use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arboard::Clipboard;
use fluent_app::TitleBar;
use fluent_core::{ThemeProvider as _, gradient_from_hue, tint};
use fluent_primitives::{
    AppDot, Avatar, Button, ButtonAppearance, ButtonSize, Card, ConnectionBadge,
    ConnectionBadgeState, Divider, FluentTextExt as _, Icon, IconButton, IconSize, Label,
    LabelSize, SectionHeader, Skeleton, SkeletonRow, Switch, TextInput, ToggleButton,
};
use gpui::{
    Animation, AnimationExt, App, Bounds, ClickEvent, Context, Entity, FontWeight, Hsla,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render,
    ScrollDelta, ScrollWheelEvent, SharedString, VideoTextureId, Window, backdrop_blur, canvas,
    div, hsla, linear_color_stop, linear_gradient, prelude::*, pulsating_between, px, relative,
};
use qrcode::{Color as QrColor, QrCode};

use androidconnect_protocol::{
    AudioControl, AudioControlCommand, DndMode, FeatureState, FeatureStatus, FileEntry,
    FileEntryType, InputEvent, MediaControl, MediaControlAction, MediaPlaybackState,
    MessageDirection, MessageSendResult, Payload, PointerButton, PointerEvent, PointerPhase,
    StorageBreakdown, UtilityFeature,
    qr::{QrPairingPayload, encode_qr_payload},
};

use crate::network;
use crate::panels::{
    ActivityIcon, ActivityLog, MessagesState, Panel,
    files::{FilesState, FilesViewMode},
    notifications::NotificationsState,
    phone::PhoneState,
};
use crate::status::{ConnectionState, DesktopStatus, format_pairing_code};
use crate::streaming::{RgbaFrame, letterbox_bounds, map_window_to_frame};

// Fixed Layout Dimensions
//
// These values size stable app chrome, anchored overlays, and illustrative
// assets. They are intentionally not theme spacing/radius tokens because
// changing them with density would alter layout contracts rather than rhythm.
const SIDEBAR_WIDTH: f32 = 220.0;
const TOPBAR_HEIGHT: f32 = 48.0;
const STATUSBAR_HEIGHT: f32 = 28.0;
const DEVICE_POPOVER_WIDTH: f32 = 320.0;
const OVERVIEW_MEDIA_CARD_WIDTH: f32 = 360.0;
const MIRROR_CHROME_OFFSET: f32 = 20.0;
const PAIR_QR_SIDE: f32 = 208.0;
const PAIR_CODE_CARD_WIDTH: f32 = 280.0;
const FILE_TILE_WIDTH: f32 = 140.0;
const FILE_TILE_MAX_WIDTH: f32 = 180.0;
const FAKE_PHONE_WIDTH: f32 = 96.0;
const FAKE_PHONE_HEIGHT: f32 = 180.0;

pub struct AppModel {
    command_tx: mpsc::SyncSender<network::DesktopCommand>,
    pub status: DesktopStatus,
    pair_addresses: Vec<String>,
    desktop_id: String,
    desktop_name: String,
    active_panel: Panel,
    device_popover_open: bool,
    pairing_requested: bool,
    title_bar: Entity<TitleBar>,

    // Video mirror
    video_texture: Option<VideoTextureId>,
    video_alloc_w: u32,
    video_alloc_h: u32,
    frame_data: Option<Arc<Vec<u8>>>,
    frame_w: u32,
    frame_h: u32,

    // QR code
    qr_texture: Option<VideoTextureId>,
    qr_alloc_size: u32,
    qr_data: Option<Arc<Vec<u8>>>,
    qr_size: u32,
    qr_cache_key: String,
    qr_expires_at_ms: u64,
    pair_code_copied_at: Option<Instant>,

    // Mirror input forwarding
    mirror_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    left_pressed: bool,
    last_mirror_mouse_move: Option<Instant>,

    // Panel states
    notifications: NotificationsState,
    phone_state: PhoneState,
    files_state: FilesState,
    messages_state: MessagesState,
    activity: ActivityLog,
    logged_initial_pair: bool,
    reconnecting: bool,

    // Per-panel text inputs (FluentGUI TextInputs are Entities and must be pre-created)
    sms_composer: Entity<TextInput>,
    quick_reply: QuickReplyState,
    quick_reply_input: Entity<TextInput>,
    file_action_input: Entity<TextInput>,
    file_action: FileActionDialog,

    /// Map of `source_path` (Android-side path the user asked to download) → desktop destination.
    /// The reader thread reads this when a `FileTransferStart` arrives and moves the completed
    /// file to the chosen destination.
    pub download_destinations: Arc<Mutex<HashMap<String, PathBuf>>>,
}

#[derive(Debug, Default, Clone)]
pub struct QuickReplyState {
    /// (notification_id, action_id) currently expanded for inline reply.
    pub open_on: Option<(String, String)>,
}

#[derive(Debug, Default, Clone)]
pub struct FileActionDialog {
    pub kind: Option<FileActionKind>,
    pub target_path: Option<String>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FileActionKind {
    CreateFolder,
    Rename,
}

impl AppModel {
    pub fn new(
        cx: &mut Context<Self>,
        command_tx: mpsc::SyncSender<network::DesktopCommand>,
        download_destinations: Arc<Mutex<HashMap<String, PathBuf>>>,
        bind: String,
        pairing_code: String,
        desktop_id: String,
        desktop_name: String,
    ) -> Self {
        let status = DesktopStatus::new(bind, pairing_code);
        let pair_addresses = advertised_pair_addresses(&status.bind);
        let mut files_state = FilesState::default();
        files_state.current_path = "/sdcard".to_owned();
        let sms_composer = cx.new(|_| TextInput::new().placeholder("Type a message…"));
        let quick_reply_input = cx.new(|_| TextInput::new().placeholder("Quick reply…"));
        let file_action_input = cx.new(|_| TextInput::new());
        let title_bar = cx.new(|cx| TitleBar::new(cx, "AndroidConnect").icon("icons/logo.svg"));

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this
                    .update(cx, |m, cx| {
                        let should_tick = matches!(m.active_panel, Panel::Mirror)
                            && m.last_mirror_mouse_move
                                .is_some_and(|last| last.elapsed() < Duration::from_millis(3500));
                        if should_tick {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update(cx, |m, cx| {
                        if (!m.connected() && m.qr_expires_at_ms > 0)
                            || m.pair_code_copied_at.is_some_and(|copied_at| {
                                copied_at.elapsed() < Duration::from_secs(2)
                            })
                        {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            command_tx,
            status,
            pair_addresses,
            desktop_id,
            desktop_name,
            active_panel: Panel::Overview,
            device_popover_open: false,
            pairing_requested: false,
            title_bar,
            video_texture: None,
            video_alloc_w: 0,
            video_alloc_h: 0,
            frame_data: None,
            frame_w: 0,
            frame_h: 0,
            qr_texture: None,
            qr_alloc_size: 0,
            qr_data: None,
            qr_size: 0,
            qr_cache_key: String::new(),
            qr_expires_at_ms: 0,
            pair_code_copied_at: None,
            mirror_bounds: Arc::new(Mutex::new(None)),
            left_pressed: false,
            last_mirror_mouse_move: None,
            notifications: NotificationsState::default(),
            phone_state: PhoneState::default(),
            files_state,
            messages_state: MessagesState::default(),
            activity: ActivityLog::default(),
            logged_initial_pair: false,
            reconnecting: false,
            sms_composer,
            quick_reply: QuickReplyState::default(),
            quick_reply_input,
            file_action_input,
            file_action: FileActionDialog::default(),
            download_destinations,
        }
    }

    pub fn push_frame(&mut self, frame: RgbaFrame, cx: &mut Context<Self>) {
        if frame.width != self.video_alloc_w || frame.height != self.video_alloc_h {
            self.video_texture = None;
            self.video_alloc_w = 0;
            self.video_alloc_h = 0;
        }
        self.frame_w = frame.width;
        self.frame_h = frame.height;
        self.frame_data = Some(Arc::new(frame.data));
        cx.notify();
    }

    pub fn push_status(&mut self, status: network::NetworkStatus, cx: &mut Context<Self>) {
        let prev_bind = self.status.bind.clone();
        let was_authenticated = self.status.input_authenticated;
        self.status.apply(status);
        if self.status.bind != prev_bind {
            self.pair_addresses = advertised_pair_addresses(&self.status.bind);
            self.qr_cache_key.clear();
        }
        if !was_authenticated && self.status.input_authenticated {
            self.reconnecting = false;
            if !self.logged_initial_pair {
                let name = self
                    .status
                    .device_name
                    .clone()
                    .or_else(|| self.status.peer.clone())
                    .unwrap_or_else(|| "device".to_owned());
                self.activity
                    .push(ActivityIcon::Pair, format!("Paired with {name}"));
                self.logged_initial_pair = true;
            }
        }
        cx.notify();
    }

    pub fn push_event(&mut self, event: network::DesktopEvent, cx: &mut Context<Self>) {
        match event {
            network::DesktopEvent::NotificationPosted(n) => self.notifications.posted(n),
            network::DesktopEvent::NotificationRemoved(r) => self.notifications.removed(r),
            network::DesktopEvent::FileBrowseResponse(r) => self.files_state.record_response(r),
            network::DesktopEvent::MessageThreadList(list) => {
                self.messages_state.apply_thread_list(list);
            }
            network::DesktopEvent::MessageEvent(ev) => {
                self.messages_state.apply_event(ev);
            }
            network::DesktopEvent::MessageThreadDetail(detail) => {
                self.messages_state.apply_thread_detail(detail);
            }
            network::DesktopEvent::MessageSendResponse(resp) => {
                let succeeded = matches!(
                    resp.result,
                    MessageSendResult::Queued | MessageSendResult::Sent
                );
                self.messages_state.apply_send_response(resp);
                if succeeded {
                    self.activity.push(ActivityIcon::Send, "Sent reply via SMS");
                }
            }
            network::DesktopEvent::SessionLost => {
                self.reconnecting = true;
                self.notifications.clear();
                self.phone_state = PhoneState::default();
                self.files_state = FilesState::default();
                self.files_state.current_path = "/sdcard".to_owned();
                self.messages_state = MessagesState::default();
                self.quick_reply = QuickReplyState::default();
                self.file_action = FileActionDialog::default();
                self.frame_data = None;
                self.frame_w = 0;
                self.frame_h = 0;
                self.video_texture = None;
                self.video_alloc_w = 0;
                self.video_alloc_h = 0;
                if let Ok(mut g) = self.mirror_bounds.lock() {
                    *g = None;
                }
                self.left_pressed = false;
                self.last_mirror_mouse_move = None;
                self.device_popover_open = false;
                self.pairing_requested = false;
                self.logged_initial_pair = false;
            }
        }
        cx.notify();
    }

    fn send_input(&self, event: InputEvent) {
        let _ = self
            .command_tx
            .try_send(network::DesktopCommand::Input(event));
    }

    fn send_utility(&self, payload: Payload) {
        let _ = self
            .command_tx
            .try_send(network::DesktopCommand::Utility(payload));
    }

    fn send_refresh_ping(&self) {
        let _ = self.command_tx.try_send(network::DesktopCommand::PingNow);
    }

    fn set_active_panel(&mut self, panel: Panel) {
        let entering_mirror =
            !matches!(self.active_panel, Panel::Mirror) && matches!(panel, Panel::Mirror);
        self.active_panel = panel;
        self.device_popover_open = false;
        self.pairing_requested = false;
        if entering_mirror {
            self.last_mirror_mouse_move = Some(Instant::now());
        }
    }

    fn request_pairing(&mut self) {
        self.active_panel = Panel::Overview;
        self.device_popover_open = false;
        self.pairing_requested = true;
    }

    fn show_permission_steps(&mut self) {
        self.request_pairing();
        self.activity
            .push(ActivityIcon::Pair, "Opened phone permission steps");
    }

    fn refresh_pair_qr(&mut self, window: &mut Window) {
        if self.pair_addresses.is_empty() {
            return;
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        let expires_ms = if self.qr_expires_at_ms > now_ms {
            self.qr_expires_at_ms
        } else {
            now_ms.saturating_add(5 * 60 * 1000)
        };
        let payload = QrPairingPayload::new(
            self.pair_addresses.clone(),
            self.status.pairing_code.clone(),
            self.desktop_id.clone(),
            self.desktop_name.clone(),
            now_ms,
            expires_ms,
        );
        let cache_key = format!(
            "v={}|addrs={:?}|tok={}|id={}|exp={}",
            payload.protocol_version,
            payload.addresses,
            payload.pairing_token,
            payload.desktop_id,
            expires_ms,
        );
        if !self.qr_cache_key.is_empty() && self.qr_cache_key == cache_key {
            return;
        }
        let Some(uri) = encode_qr_payload(&payload).ok() else {
            return;
        };
        let Some((pixels, size_px)) = build_qr_pixels(&uri) else {
            return;
        };
        if self.qr_texture.is_none() || self.qr_alloc_size != size_px {
            if let Some(id) = self.qr_texture.take() {
                window.free_video_texture(id);
            }
            self.qr_texture = window.alloc_video_texture(size_px, size_px).ok();
            self.qr_alloc_size = size_px;
        }
        self.qr_data = Some(Arc::new(pixels));
        self.qr_size = size_px;
        self.qr_cache_key = cache_key;
        self.qr_expires_at_ms = expires_ms;
    }

    fn connected(&self) -> bool {
        matches!(self.status.connection, ConnectionState::Connected)
            && self.status.input_authenticated
    }

    fn badge_state(&self) -> ConnectionBadgeState {
        match self.status.connection {
            ConnectionState::Listening => ConnectionBadgeState::Disconnected,
            ConnectionState::Connected if self.status.input_authenticated => {
                ConnectionBadgeState::Paired
            }
            ConnectionState::Connected => ConnectionBadgeState::Pairing,
            ConnectionState::Error => ConnectionBadgeState::Error,
        }
    }

    // ── Chrome ────────────────────────────────────────────────────────────

    fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;
        let radii = cx.theme().radii;
        let entity = cx.entity();
        let refresh_entity = entity.clone();
        let settings_entity = entity.clone();
        let device_entity = entity.clone();

        let subject = self
            .status
            .device_name
            .clone()
            .or_else(|| self.status.peer.clone())
            .unwrap_or_else(|| "No device".to_owned());
        let initials = device_initials(&subject);

        let mut bar = div()
            .h(px(TOPBAR_HEIGHT))
            .px(px(spacing.xl))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing.lg))
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.stroke_neutral_subtle)
            .child(
                div()
                    .id("topbar-device-chip")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(spacing.md))
                    .pr(px(10.0))
                    .pl(px(spacing.sm))
                    .py(px(spacing.sm))
                    .rounded(px(radii.pill))
                    .border_1()
                    .border_color(colors.stroke_neutral)
                    .bg(colors.neutral)
                    .cursor_pointer()
                    .hover(move |s| s.bg(colors.neutral_hover))
                    .on_click(move |_, _, app| {
                        device_entity.update(app, |m, cx| {
                            m.device_popover_open = !m.device_popover_open;
                            cx.notify();
                        });
                    })
                    .child(Avatar::initials(initials).size(24.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(typography.body.size))
                                    .text_color(colors.on_neutral)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(subject.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(colors.on_subtle)
                                    .child(self.status.bind.clone()),
                            ),
                    ),
            )
            .child(ConnectionBadge::new("topbar-conn", self.badge_state()))
            .child(div().flex_1())
            .child(action_icon_button(
                "topbar-theme",
                "moon",
                cx,
                |_, _, app| {
                    fluent_core::Theme::toggle(app);
                },
            ))
            .child(action_icon_button(
                "topbar-refresh",
                "refresh",
                cx,
                move |_, _, app| {
                    refresh_entity.update(app, |m, cx| {
                        m.send_refresh_ping();
                        cx.notify();
                    });
                },
            ))
            .child(action_icon_button(
                "topbar-settings",
                "settings",
                cx,
                move |_, _, app| {
                    settings_entity.update(app, |m, cx| {
                        m.set_active_panel(Panel::Settings);
                        cx.notify();
                    });
                },
            ));

        if self.device_popover_open {
            bar = bar.child(self.render_device_popover(cx));
        }

        bar
    }

    fn render_device_popover(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;
        let radii = cx.theme().radii;
        let entity = cx.entity();
        let pair_entity = entity.clone();
        let refresh_entity = entity.clone();
        let subject = self
            .status
            .device_name
            .clone()
            .or_else(|| self.status.peer.clone())
            .unwrap_or_else(|| "No device paired".to_owned());
        let peer = self
            .status
            .peer
            .clone()
            .unwrap_or_else(|| "No peer connected".to_owned());

        div()
            .absolute()
            .top(px(TOPBAR_HEIGHT + spacing.sm))
            .left(px(spacing.xl))
            .w(px(DEVICE_POPOVER_WIDTH))
            .p(px(spacing.lg))
            .flex()
            .flex_col()
            .gap(px(spacing.md))
            .rounded(px(radii.lg))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.stroke_neutral)
            .shadow_md()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(spacing.md))
                    .child(Avatar::initials(device_initials(&subject)).size(36.0))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(spacing.xs))
                            .child(
                                div()
                                    .text_color(colors.on_neutral)
                                    .text_size(px(typography.body.size))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(subject),
                            )
                            .child(
                                div()
                                    .text_color(colors.on_subtle)
                                    .text_size(px(typography.caption.size))
                                    .child(peer),
                            ),
                    )
                    .child(ConnectionBadge::new(
                        "device-popover-conn",
                        self.badge_state(),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(spacing.xs))
                    .text_color(colors.on_subtle)
                    .text_size(px(typography.caption.size))
                    .child(div().child(format!("Listening on {}", self.status.bind)))
                    .child(div().child(if self.status.input_authenticated {
                        "Current pairing remains active"
                    } else {
                        "Waiting for pairing"
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(spacing.sm))
                    .child(
                        Button::new("device-popover-pair")
                            .label("Pair new device")
                            .appearance(ButtonAppearance::Accent)
                            .size(ButtonSize::Compact)
                            .on_click(move |_, _, app| {
                                pair_entity.update(app, |m, cx| {
                                    m.request_pairing();
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        Button::new("device-popover-refresh")
                            .label("Refresh status")
                            .appearance(ButtonAppearance::Subtle)
                            .size(ButtonSize::Compact)
                            .on_click(move |_, _, app| {
                                refresh_entity.update(app, |m, cx| {
                                    m.device_popover_open = false;
                                    m.send_refresh_ping();
                                    cx.notify();
                                });
                            }),
                    ),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;
        let radii = cx.theme().radii;
        let entity = cx.entity();

        let mut rail = div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .bg(colors.panel_bg)
            .border_r_1()
            .border_color(colors.stroke_neutral_subtle)
            .flex()
            .flex_col();

        rail = rail.child(section_header("Workspace", cx));

        for panel in Panel::ALL {
            let is_active = panel == self.active_panel;
            let entity2 = entity.clone();
            let unread = if matches!(panel, Panel::Messages) {
                self.messages_state
                    .threads
                    .iter()
                    .map(|t| t.unread_count)
                    .sum::<u32>()
            } else {
                0
            };

            let label_color = if is_active {
                colors.on_neutral
            } else {
                colors.on_subtle
            };

            let mut item = div()
                .id(("nav-", panel as usize))
                .h(px(36.0))
                .flex()
                .flex_row()
                .items_center()
                .cursor_pointer()
                .bg(if is_active {
                    colors.neutral_selected
                } else {
                    gpui::transparent_black()
                })
                .hover(move |s| {
                    if is_active {
                        s
                    } else {
                        s.bg(colors.subtle_hover)
                    }
                })
                .on_click(move |_: &ClickEvent, _win, app: &mut App| {
                    entity2.update(app, |m, cx| {
                        m.set_active_panel(panel);
                        cx.notify();
                    });
                });

            // 3px accent indicator bar on the left
            item = item.child(div().w(px(3.0)).h(px(20.0)).ml(px(0.0)).bg(if is_active {
                colors.accent
            } else {
                gpui::transparent_black()
            }));

            // Icon
            item = item.child(
                div()
                    .pl(px(13.0))
                    .pr(px(10.0))
                    .text_color(label_color)
                    .child(Icon::new(panel.icon()).size(IconSize::Sm)),
            );

            // Label
            item = item.child(
                div()
                    .flex_1()
                    .text_size(px(typography.body.size))
                    .text_color(label_color)
                    .font_weight(if is_active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(panel.label()),
            );

            // Unread badge
            if unread > 0 {
                item = item.child(
                    div()
                        .mr(px(spacing.lg))
                        .min_w(px(18.0))
                        .h(px(18.0))
                        .px(px(6.0))
                        .rounded(px(radii.pill))
                        .bg(colors.status_error)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.0))
                        .text_color(gpui::white())
                        .font_weight(FontWeight::BOLD)
                        .tabular_nums()
                        .child(format!("{unread}")),
                );
            }

            rail = rail.child(item);
        }

        rail = rail.child(section_header("Device", cx)).child(
            div()
                .id("nav-pair")
                .h(px(36.0))
                .px(px(spacing.xl))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .cursor_pointer()
                .hover(move |s| s.bg(colors.subtle_hover))
                .on_click({
                    let e = entity.clone();
                    move |_: &ClickEvent, _, app: &mut App| {
                        e.update(app, |m, cx| {
                            m.request_pairing();
                            cx.notify();
                        });
                    }
                })
                .text_color(colors.on_subtle)
                .text_size(px(typography.body.size))
                .child(Icon::new("qr").size(IconSize::Sm))
                .child("Pair new device"),
        );

        rail = rail.child(div().flex_1()).child(
            div()
                .px(px(spacing.xl))
                .py(px(10.0))
                .flex()
                .flex_col()
                .gap(px(spacing.xs))
                .border_t_1()
                .border_color(colors.stroke_neutral_subtle)
                .child(
                    div()
                        .text_size(px(typography.caption.size))
                        .text_color(colors.on_subtle_disabled)
                        .child("AndroidConnect 0.4.2-beta"),
                )
                .child(
                    div()
                        .text_size(px(typography.caption.size))
                        .text_color(colors.on_subtle_disabled)
                        .child("Built on FluentGUI"),
                ),
        );

        rail
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;

        let mut bar = div()
            .h(px(STATUSBAR_HEIGHT))
            .px(px(spacing.lg))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing.xl))
            .bg(colors.surface_dim)
            .border_t_1()
            .border_color(colors.stroke_neutral_subtle)
            .text_size(px(typography.caption.size))
            .text_color(colors.on_subtle);

        if !self.connected() {
            bar = bar
                .child(div().child("Waiting for device…"))
                .child(div().flex_1())
                .child(div().child(format!("listening on {}", self.status.bind)));
            return bar;
        }

        let battery = self
            .status
            .battery_status
            .clone()
            .unwrap_or_else(|| "—".to_owned());
        let wifi = self
            .status
            .wifi_summary
            .as_deref()
            .unwrap_or("—")
            .to_owned();
        let bt = match self.status.bluetooth_enabled {
            Some(true) => "on",
            Some(false) => "off",
            None => "—",
        };
        let dnd = match self.status.dnd_mode {
            None | Some(DndMode::Off) => "off",
            Some(DndMode::Priority) => "priority",
            Some(DndMode::Alarms) => "alarms",
            Some(DndMode::TotalSilence) => "silence",
        };
        let vol = self
            .status
            .volume_percent
            .map(|p| format!("{p}%"))
            .unwrap_or_else(|| "—".to_owned());

        bar = bar
            .child(stat_chip("battery", &battery, cx))
            .child(stat_chip("wifi", &wifi, cx))
            .child(stat_chip("bluetooth", bt, cx))
            .child(stat_chip("moon", dnd, cx))
            .child(stat_chip("vol", &vol, cx))
            .child(div().flex_1())
            .child(heartbeat_dot(cx));
        bar
    }

    // ── Panels ────────────────────────────────────────────────────────────

    fn render_overview_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;
        let entity = cx.entity();

        if !self.connected() {
            // Disconnected hero
            let e_pair = entity.clone();
            return div()
                .id("overview-disc")
                .size_full()
                .overflow_y_scroll()
                .p(px(40.0))
                .bg(colors.surface)
                .child(
                    Card::new()
                        .padding(24.0)
                        .gap(16.0)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(spacing.xxl))
                                .items_center()
                                .child(
                                    div()
                                        .size(px(96.0))
                                        .rounded(px(20.0))
                                        .bg(colors.surface_dim)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(colors.on_neutral_accent)
                                        .child(Icon::new("qr").size(IconSize::Lg)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap(px(spacing.md))
                                        .child(Label::new("Connect your Android phone").size(LabelSize::Title))
                                        .child(
                                            div()
                                                .text_color(colors.on_subtle)
                                                .text_size(px(typography.body.size))
                                                .child("Pair AndroidConnect on your phone to mirror its screen, see notifications, and exchange messages here.")
                                        )
                                        .child(
                                            div()
                                                .mt(px(spacing.md))
                                                .flex()
                                                .flex_row()
                                                .gap(px(spacing.md))
                                                .child(
                                                    Button::new("overview-pair")
                                                        .label("Pair a device")
                                                        .appearance(ButtonAppearance::Accent)
                                                        .on_click(move |_, _, app| {
                                                            e_pair.update(app, |m, cx| {
                                                                m.request_pairing();
                                                                cx.notify();
                                                            });
                                                        }),
                                                )
                                        ),
                                ),
                        )
                )
                .into_any_element();
        }

        // Connected — full overview
        let device_name = self
            .status
            .device_name
            .clone()
            .unwrap_or_else(|| "Phone".to_owned());
        let greeting = local_greeting();
        let today = today_string();
        let battery_pct = parse_battery_percent(self.status.battery_status.as_deref());

        let entity_quick = entity.clone();

        let mut root = div()
            .id("overview-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(colors.surface)
            .p(px(spacing.xxl))
            .flex()
            .flex_col()
            .gap(px(spacing.xl));

        // Row 1: Greeting
        root = root.child(
            div()
                .flex()
                .flex_row()
                .items_end()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(spacing.xs))
                        .child(Label::new(greeting).size(LabelSize::Display))
                        .child(
                            div()
                                .text_color(colors.on_subtle)
                                .text_size(px(typography.body.size))
                                .child(format!("Connected to {device_name} · last sync just now")),
                        ),
                )
                .child(
                    div()
                        .text_color(colors.on_subtle)
                        .text_size(px(typography.body.size))
                        .child(today),
                ),
        );

        // Row 2: Device hero card
        let mut hero_row = div().flex().flex_row().gap(px(spacing.xl));
        hero_row = hero_row.child(div().flex_1().child(device_summary_card(
            &device_name,
            self.badge_state(),
            battery_pct,
            self.status.wifi_summary.as_deref(),
            self.status.bluetooth_enabled,
            cx,
        )));
        if let Some(media) = self.status.media_info.clone() {
            hero_row = hero_row.child(
                div()
                    .w(px(OVERVIEW_MEDIA_CARD_WIDTH))
                    .child(now_playing_card(&media, entity.clone(), cx)),
            );
        }
        root = root.child(hero_row);

        // Row 3: Quick actions
        root = root.child(
            div()
                .flex()
                .flex_row()
                .gap(px(spacing.lg))
                .child(quick_action(
                    "qa-mirror",
                    "mirror",
                    "Open mirror",
                    "Stream the screen",
                    {
                        let e = entity_quick.clone();
                        move |_, _, app| {
                            e.update(app, |m, cx| {
                                m.set_active_panel(Panel::Mirror);
                                cx.notify();
                            });
                        }
                    },
                    cx,
                ))
                .child(quick_action(
                    "qa-msg",
                    "chat",
                    "New message",
                    "Send an SMS",
                    {
                        let e = entity_quick.clone();
                        move |_, _, app| {
                            e.update(app, |m, cx| {
                                m.set_active_panel(Panel::Messages);
                                cx.notify();
                            });
                        }
                    },
                    cx,
                ))
                .child(quick_action(
                    "qa-files",
                    "upload",
                    "Send to phone",
                    "Push a file",
                    {
                        let e = entity_quick.clone();
                        move |_, _, app| {
                            e.update(app, |m, cx| {
                                m.set_active_panel(Panel::Files);
                                cx.notify();
                            });
                        }
                    },
                    cx,
                ))
                .child(quick_action(
                    "qa-phone",
                    "phone",
                    "Phone controls",
                    "Volume, DND, Bluetooth",
                    {
                        let e = entity_quick.clone();
                        move |_, _, app| {
                            e.update(app, |m, cx| {
                                m.set_active_panel(Panel::Phone);
                                cx.notify();
                            });
                        }
                    },
                    cx,
                )),
        );

        // Row 4: Recent grids
        let e_notifs = entity.clone();
        let e_msgs = entity.clone();
        root = root.child(
            div()
                .flex()
                .flex_row()
                .gap(px(spacing.xl))
                .child(
                    div().flex_1().child(
                        Card::new()
                            .padding(16.0)
                            .child(
                                SectionHeader::new("Recent notifications").action(
                                    Button::new("ov-see-notifs")
                                        .label("See all")
                                        .appearance(ButtonAppearance::Subtle)
                                        .size(ButtonSize::Compact)
                                        .on_click(move |_, _, app| {
                                            e_notifs.update(app, |m, cx| {
                                                m.set_active_panel(Panel::Notifications);
                                                cx.notify();
                                            });
                                        }),
                                ),
                            )
                            .children(recent_notifications(self, 4, cx)),
                    ),
                )
                .child(
                    div().flex_1().child(
                        Card::new()
                            .padding(16.0)
                            .child(
                                SectionHeader::new("Recent messages").action(
                                    Button::new("ov-see-msgs")
                                        .label("Open inbox")
                                        .appearance(ButtonAppearance::Subtle)
                                        .size(ButtonSize::Compact)
                                        .on_click(move |_, _, app| {
                                            e_msgs.update(app, |m, cx| {
                                                m.set_active_panel(Panel::Messages);
                                                cx.notify();
                                            });
                                        }),
                                ),
                            )
                            .children(recent_messages(self, 4, cx)),
                    ),
                ),
        );

        // Row 5: Activity card
        root = root.child(
            Card::new()
                .padding(16.0)
                .child(SectionHeader::new("Recent activity"))
                .children(activity_rows(self, 8, cx)),
        );

        root = root.child(storage_card(self, entity.clone(), cx));

        root.into_any_element()
    }

    fn render_mirror_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;
        let radii = cx.theme().radii;
        let entity = cx.entity();
        let video_id = self.video_texture;
        let frame_data = self.frame_data.clone();
        let mirror_bounds_shared = Arc::clone(&self.mirror_bounds);
        let mirror_bounds_shared2 = Arc::clone(&self.mirror_bounds);
        let mirror_bounds_canvas = Arc::clone(&self.mirror_bounds);
        let frame_w = self.frame_w;
        let frame_h = self.frame_h;
        let chrome_opacity = mirror_chrome_opacity(self.last_mirror_mouse_move);
        let mirror_settings_entity = entity.clone();
        let mirror_refresh_entity = entity.clone();

        if !self.connected() {
            return mirror_disconnected_panel(entity, cx).into_any_element();
        }

        if video_id.is_none() || frame_data.is_none() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(spacing.lg))
                .bg(gpui::black())
                .child(
                    div()
                        .size(px(64.0))
                        .rounded(px(2.0 * radii.lg))
                        .bg(colors.neutral)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(colors.on_subtle)
                        .child(Icon::new("mirror").size(IconSize::Lg)),
                )
                .child(
                    div()
                        .text_color(colors.on_subtle)
                        .text_size(px(typography.body.size))
                        .child("Waiting for first frame…"),
                )
                .into_any_element();
        }

        div()
            .size_full()
            .relative()
            .bg(gpui::black())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    this.left_pressed = true;
                    let bounds = mirror_bounds_shared.lock().ok().and_then(|g| *g);
                    if let Some(b) = bounds {
                        let (x, y) = local_pos(event.position, b);
                        let window_w = f32::from(b.size.width) as u32;
                        let window_h = f32::from(b.size.height) as u32;
                        if let Some((fx, fy)) =
                            map_window_to_frame(x, y, window_w, window_h, frame_w, frame_h, false)
                        {
                            this.send_input(InputEvent::Pointer(PointerEvent {
                                pointer_id: 0,
                                x: fx,
                                y: fy,
                                phase: PointerPhase::Down,
                                button: PointerButton::Left,
                                delta_x: 0,
                                delta_y: 0,
                            }));
                        }
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, _, _| {
                    this.left_pressed = false;
                    let bounds = mirror_bounds_shared2.lock().ok().and_then(|g| *g);
                    if let Some(b) = bounds {
                        let (x, y) = local_pos(event.position, b);
                        let window_w = f32::from(b.size.width) as u32;
                        let window_h = f32::from(b.size.height) as u32;
                        if let Some((fx, fy)) =
                            map_window_to_frame(x, y, window_w, window_h, frame_w, frame_h, true)
                        {
                            this.send_input(InputEvent::Pointer(PointerEvent {
                                pointer_id: 0,
                                x: fx,
                                y: fy,
                                phase: PointerPhase::Up,
                                button: PointerButton::Left,
                                delta_x: 0,
                                delta_y: 0,
                            }));
                        }
                    }
                }),
            )
            .on_mouse_move(cx.listener({
                let mirror_bounds_m = Arc::clone(&self.mirror_bounds);
                move |this, event: &MouseMoveEvent, _, cx| {
                    this.last_mirror_mouse_move = Some(Instant::now());
                    cx.notify();
                    if !this.left_pressed {
                        return;
                    }
                    let bounds = mirror_bounds_m.lock().ok().and_then(|g| *g);
                    if let Some(b) = bounds {
                        let (x, y) = local_pos(event.position, b);
                        let window_w = f32::from(b.size.width) as u32;
                        let window_h = f32::from(b.size.height) as u32;
                        if let Some((fx, fy)) =
                            map_window_to_frame(x, y, window_w, window_h, frame_w, frame_h, true)
                        {
                            this.send_input(InputEvent::Pointer(PointerEvent {
                                pointer_id: 0,
                                x: fx,
                                y: fy,
                                phase: PointerPhase::Move,
                                button: PointerButton::Left,
                                delta_x: 0,
                                delta_y: 0,
                            }));
                        }
                    }
                }
            }))
            .on_scroll_wheel(cx.listener({
                let mirror_bounds_s = Arc::clone(&self.mirror_bounds);
                move |this, event: &ScrollWheelEvent, _, _| {
                    let bounds = mirror_bounds_s.lock().ok().and_then(|g| *g);
                    if let Some(b) = bounds {
                        let (x, y) = local_pos(event.position, b);
                        let window_w = f32::from(b.size.width) as u32;
                        let window_h = f32::from(b.size.height) as u32;
                        if let Some((fx, fy)) =
                            map_window_to_frame(x, y, window_w, window_h, frame_w, frame_h, false)
                        {
                            let (dx, dy) = match event.delta {
                                ScrollDelta::Pixels(p) => {
                                    (f32::from(p.x) as i32, f32::from(p.y) as i32)
                                }
                                ScrollDelta::Lines(p) => {
                                    ((p.x * 120.0).round() as i32, (p.y * 120.0).round() as i32)
                                }
                            };
                            if dx != 0 || dy != 0 {
                                this.send_input(InputEvent::Pointer(PointerEvent {
                                    pointer_id: 0,
                                    x: fx,
                                    y: fy,
                                    phase: PointerPhase::Wheel,
                                    button: PointerButton::None,
                                    delta_x: dx,
                                    delta_y: dy,
                                }));
                            }
                        }
                    }
                }
            }))
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        if let Ok(mut g) = mirror_bounds_canvas.lock() {
                            *g = Some(bounds);
                        }
                        (frame_data, video_id, frame_w, frame_h)
                    },
                    |bounds, state, window, _cx| {
                        let (Some(data), Some(id), fw, fh) = state else {
                            return;
                        };
                        let video_bounds = letterbox_bounds(bounds, fw, fh);
                        let _ = window.paint_video_frame(id, video_bounds, &data);
                    },
                )
                .size_full(),
            )
            // Floating control rail — top-right, icon buttons backed by
            // backdrop-blurred glass.
            .child(
                div()
                    .absolute()
                    .top(px(MIRROR_CHROME_OFFSET))
                    .right(px(MIRROR_CHROME_OFFSET))
                    .opacity(chrome_opacity)
                    .flex()
                    .flex_col()
                    .gap(px(spacing.md))
                    .p(px(spacing.sm))
                    .rounded(px(10.0))
                    // Backdrop blur background — samples the video frame
                    // beneath, blurs it 16px, tints it ~10% black.
                    .bg(backdrop_blur(16.0, mirror_glass_fill()))
                    .border_1()
                    .border_color(mirror_glass_stroke())
                    .child(mirror_chip_icon(
                        "mirror-settings",
                        "settings",
                        move |_, _, app| {
                            mirror_settings_entity.update(app, |m, cx| {
                                m.set_active_panel(Panel::Settings);
                                cx.notify();
                            });
                        },
                        cx,
                    ))
                    .child(mirror_chip_icon(
                        "mirror-refresh",
                        "refresh",
                        move |_, _, app| {
                            mirror_refresh_entity.update(app, |m, cx| {
                                m.send_refresh_ping();
                                cx.notify();
                            });
                        },
                        cx,
                    )),
            )
            // Stream-info pill — bottom-left.
            .child(
                div()
                    .absolute()
                    .bottom(px(MIRROR_CHROME_OFFSET))
                    .left(px(MIRROR_CHROME_OFFSET))
                    .opacity(chrome_opacity)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(spacing.lg))
                    .py(px(6.0))
                    .rounded(px(radii.pill))
                    .bg(backdrop_blur(16.0, mirror_glass_fill()))
                    .border_1()
                    .border_color(mirror_glass_stroke())
                    .text_color(gpui::white())
                    .text_size(px(typography.caption.size))
                    .child(
                        div()
                            .size(px(6.0))
                            .rounded(px(radii.pill))
                            .bg(colors.status_success),
                    )
                    .child(
                        div()
                            .tabular_nums()
                            .child(format!("{}×{}", frame_w, frame_h)),
                    ),
            )
            .into_any_element()
    }

    fn render_settings_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;

        div()
            .id("settings-panel")
            .size_full()
            .bg(colors.surface)
            .p(px(spacing.xxl))
            .child(
                Card::new()
                    .padding(20.0)
                    .gap(spacing.md)
                    .child(Label::new("Settings").size(LabelSize::Display))
                    .child(
                        div()
                            .text_color(colors.on_subtle)
                            .text_size(px(typography.body.size))
                            .child("Settings are coming soon."),
                    ),
            )
            .into_any_element()
    }

    fn render_pair_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;
        let radii = cx.theme().radii;
        let pairing_code_fmt = format_pairing_code(&self.status.pairing_code);
        let addresses = self.pair_addresses.clone();
        let bind = self.status.bind.clone();
        let last_error = self.status.last_error.clone();
        let qr_data = self.qr_data.clone();
        let qr_id = self.qr_texture;
        let qr_side = px(PAIR_QR_SIDE);
        let qr_native = self.qr_size;
        let countdown = pair_countdown_label(self.qr_expires_at_ms);
        let copy_icon = if self
            .pair_code_copied_at
            .is_some_and(|copied_at| copied_at.elapsed() < Duration::from_millis(1500))
        {
            "icons/check.svg"
        } else {
            "icons/copy.svg"
        };
        let copy_entity = cx.entity();
        let copy_code = self.status.pairing_code.clone();

        // Left column: instructions
        let mut left = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(Label::new("Pair a new device").size(LabelSize::Display))
            .child(
                div()
                    .text_color(colors.on_subtle)
                    .text_size(px(typography.body.size))
                    .child("Install AndroidConnect on your phone, then scan the QR or enter the pairing code below."),
            );

        for (idx, (title, body)) in [
            ("Install on phone", "Search 'AndroidConnect' in Play Store, or scan the QR with your camera."),
            ("Open the app", "Tap 'Connect to PC' and grant the requested permissions."),
            ("Scan the QR", "Or enter the pairing code shown here. Both devices must be on the same Wi-Fi network."),
        ].iter().enumerate() {
            left = left.child(numbered_step(idx + 1, title, body, cx));
        }

        left = left
            .child(div().h(px(spacing.md)))
            .child(Divider::horizontal())
            .child(
                div()
                    .text_color(colors.on_neutral)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(typography.body.size))
                    .child("Listening on"),
            );

        let addrs_for_display = if addresses.is_empty() {
            vec![bind.clone()]
        } else {
            addresses.clone()
        };
        let mut addr_row = div().flex().flex_row().flex_wrap().gap(px(6.0));
        for a in addrs_for_display {
            addr_row = addr_row.child(
                div()
                    .px(px(spacing.md))
                    .py(px(spacing.xs))
                    .rounded(px(radii.md))
                    .bg(colors.neutral)
                    .border_1()
                    .border_color(colors.stroke_neutral_subtle)
                    .text_color(colors.on_subtle)
                    .text_size(px(typography.caption.size))
                    .font_family("monospace")
                    .child(a),
            );
        }
        left = left.child(addr_row);

        // Right column: QR + code
        let qr_card = Card::new().padding(20.0).gap(12.0).child(
            div()
                .w(qr_side)
                .h(qr_side)
                .flex_none()
                .p(px(spacing.lg))
                .bg(gpui::white())
                .rounded(px(radii.md))
                .child(if qr_data.is_some() && qr_id.is_some() {
                    canvas(
                        move |_, _, _| (qr_data, qr_id, qr_native),
                        |bounds, state, window, _| {
                            let (Some(data), Some(id), native) = state else {
                                return;
                            };
                            let painted = letterbox_bounds(bounds, native, native);
                            let _ = window.paint_video_frame(id, painted, &data);
                        },
                    )
                    .size_full()
                    .into_any_element()
                } else {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(colors.on_subtle_disabled)
                        .text_size(px(typography.caption.size))
                        .child("QR unavailable")
                        .into_any_element()
                }),
        );

        let code_chip = div()
            .px(px(18.0))
            .py(px(10.0))
            .bg(colors.neutral)
            .border_1()
            .border_color(colors.stroke_neutral_subtle)
            .rounded(px(radii.md))
            .text_color(colors.on_neutral)
            .text_size(px(28.0))
            .font_family("monospace")
            .font_weight(FontWeight::SEMIBOLD)
            .tabular_nums()
            .child(pairing_code_fmt);

        let code_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing.sm))
            .child(code_chip)
            .child(
                IconButton::new("pair-copy-code", copy_icon)
                    .size(ButtonSize::Normal)
                    .appearance(ButtonAppearance::Subtle)
                    .on_click(move |_, _, app| {
                        let code = copy_code.clone();
                        copy_entity.update(app, |m, cx| {
                            if Clipboard::new()
                                .and_then(|mut clipboard| clipboard.set_text(code))
                                .is_ok()
                            {
                                m.pair_code_copied_at = Some(Instant::now());
                            }
                            cx.notify();
                        });
                    }),
            );

        let right = div()
            .w(px(PAIR_CODE_CARD_WIDTH))
            .flex()
            .flex_col()
            .gap(px(spacing.xl))
            .items_center()
            .child(qr_card)
            .child(
                div()
                    .text_color(colors.on_subtle)
                    .text_size(px(typography.caption.size))
                    .child("or enter the code manually"),
            )
            .child(code_row)
            .child(
                div()
                    .text_color(colors.on_subtle)
                    .text_size(px(typography.caption.size))
                    .tabular_nums()
                    .child(countdown),
            )
            .child(if let Some(err) = last_error {
                div()
                    .px(px(spacing.lg))
                    .py(px(spacing.md))
                    .rounded(px(radii.md))
                    .bg(colors.status_error_bg)
                    .border_1()
                    .border_color(colors.status_error_border)
                    .text_color(colors.status_error)
                    .text_size(px(typography.caption.size))
                    .child(err)
                    .into_any_element()
            } else {
                div().into_any_element()
            });

        div()
            .id("pair-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(colors.surface)
            .p(px(40.0))
            .flex()
            .flex_row()
            .gap(px(2.0 * spacing.xl))
            .items_start()
            .child(left)
            .child(right)
    }

    fn render_phone_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let spacing = cx.theme().spacing;
        let entity = cx.entity();

        if !self.connected() {
            let (title, body) = if self.reconnecting {
                (
                    "Reconnecting…",
                    "Your phone disconnected. Waiting to reconnect automatically.",
                )
            } else {
                (
                    "Quick settings & media",
                    "Pair a device to control volume, Do Not Disturb, Bluetooth, and now-playing media from your desktop.",
                )
            };
            return div()
                .size_full()
                .bg(colors.surface)
                .child(disconnected_placeholder("phone", title, body, cx))
                .into_any_element();
        }

        let device_status_feature = self.status.feature(UtilityFeature::DeviceStatus).cloned();
        if let Some(status) = device_status_feature.as_ref()
            && let Some(banner) = permission_banner(
                status,
                "Device status mirroring is unavailable.",
                onboarding_action(entity.clone()),
                cx,
            )
        {
            return div()
                .size_full()
                .bg(colors.surface)
                .p(px(spacing.xxl))
                .flex()
                .flex_col()
                .gap(px(spacing.xl))
                .child(Label::new("Phone").size(LabelSize::Title))
                .child(banner)
                .into_any_element();
        }

        self.phone_state.reconcile(&self.status);
        self.phone_state.expire_pending();

        let volume = self
            .phone_state
            .pending_volume
            .or(self.status.volume_percent)
            .unwrap_or(0);
        let vol_pending = self.phone_state.pending_volume.is_some();
        let dnd_current = self.phone_state.pending_dnd.or(self.status.dnd_mode);
        let dnd_pending = self.phone_state.pending_dnd.is_some();
        let bt_enabled = self
            .phone_state
            .pending_bluetooth
            .or(self.status.bluetooth_enabled);
        let bt_pending = self.phone_state.pending_bluetooth.is_some();
        let error_msg = self.phone_state.error.clone();
        let media_info = self.status.media_info.clone();
        let battery_pct = parse_battery_percent(self.status.battery_status.as_deref());

        let entity_vol_down = entity.clone();
        let entity_vol_up = entity.clone();
        let entity_dnd = [
            entity.clone(),
            entity.clone(),
            entity.clone(),
            entity.clone(),
        ];
        let entity_bt = entity.clone();

        let mut grid = div().flex().flex_col().gap(px(spacing.xl));

        if let Some(media) = media_info {
            grid = grid.child(now_playing_card(&media, entity.clone(), cx));
        }

        let mut row1 = div().flex().flex_row().gap(px(spacing.xl));
        row1 = row1
            .child(div().flex_1().child({
                // Volume card
                let mut c = Card::new().padding(12.0).gap(10.0).child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(spacing.md))
                        .child(Icon::new("vol").size(IconSize::Md))
                        .child(
                            div()
                                .flex_1()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.on_neutral)
                                .child("Media volume"),
                        )
                        .child(if vol_pending {
                            pending_dot(cx).into_any_element()
                        } else {
                            div().into_any_element()
                        })
                        .child(
                            div()
                                .text_color(colors.on_subtle)
                                .tabular_nums()
                                .child(format!("{volume}%")),
                        ),
                );
                c = c.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(spacing.md))
                        .child(
                            Button::new("vol-down")
                                .label("−10")
                                .appearance(ButtonAppearance::Subtle)
                                .size(ButtonSize::Compact)
                                .on_click(move |_: &ClickEvent, _, app: &mut App| {
                                    entity_vol_down.update(app, |m, cx| {
                                        let new_vol = m
                                            .phone_state
                                            .pending_volume
                                            .or(m.status.volume_percent)
                                            .unwrap_or(0)
                                            .saturating_sub(10);
                                        m.phone_state.pending_volume = Some(new_vol);
                                        m.phone_state.pending_volume_since = Some(Instant::now());
                                        m.phone_state.error = None;
                                        m.send_utility(Payload::AudioControl(AudioControl {
                                            command: AudioControlCommand::SetVolume {
                                                percent: new_vol,
                                            },
                                        }));
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            Button::new("vol-up")
                                .label("+10")
                                .appearance(ButtonAppearance::Subtle)
                                .size(ButtonSize::Compact)
                                .on_click(move |_: &ClickEvent, _, app: &mut App| {
                                    entity_vol_up.update(app, |m, cx| {
                                        let new_vol = m
                                            .phone_state
                                            .pending_volume
                                            .or(m.status.volume_percent)
                                            .unwrap_or(0)
                                            .saturating_add(10)
                                            .min(100);
                                        m.phone_state.pending_volume = Some(new_vol);
                                        m.phone_state.pending_volume_since = Some(Instant::now());
                                        m.phone_state.error = None;
                                        m.send_utility(Payload::AudioControl(AudioControl {
                                            command: AudioControlCommand::SetVolume {
                                                percent: new_vol,
                                            },
                                        }));
                                        cx.notify();
                                    });
                                }),
                        ),
                );
                c
            }))
            .child(div().flex_1().child({
                // Bluetooth card
                Card::new().padding(12.0).gap(10.0).child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(spacing.md))
                        .child(Icon::new("bluetooth").size(IconSize::Md))
                        .child(
                            div()
                                .flex_1()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.on_neutral)
                                .child("Bluetooth"),
                        )
                        .child(if bt_pending {
                            pending_dot(cx).into_any_element()
                        } else {
                            div().into_any_element()
                        })
                        .child(
                            Switch::new("bt-toggle")
                                .on(bt_enabled.unwrap_or(false))
                                .on_click(move |new_val, _: &ClickEvent, _, app: &mut App| {
                                    entity_bt.update(app, |m, cx| {
                                        m.phone_state.pending_bluetooth = Some(new_val);
                                        m.phone_state.pending_bluetooth_since =
                                            Some(Instant::now());
                                        m.phone_state.error = None;
                                        m.send_utility(Payload::AudioControl(AudioControl {
                                            command: AudioControlCommand::SetBluetooth {
                                                enabled: new_val,
                                            },
                                        }));
                                        cx.notify();
                                    });
                                }),
                        ),
                )
            }));
        grid = grid.child(row1);

        // DND card (full width)
        let mut dnd_card = Card::new().padding(12.0).gap(10.0).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(spacing.md))
                .child(Icon::new("moon").size(IconSize::Md))
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.on_neutral)
                        .child("Do Not Disturb"),
                )
                .child(if dnd_pending {
                    pending_dot(cx).into_any_element()
                } else {
                    div().into_any_element()
                }),
        );
        let mut dnd_row = div().flex().flex_row().gap(px(spacing.sm));
        for (i, mode) in [
            DndMode::Off,
            DndMode::Priority,
            DndMode::Alarms,
            DndMode::TotalSilence,
        ]
        .iter()
        .enumerate()
        {
            let is_sel = dnd_current == Some(*mode);
            let e = entity_dnd[i].clone();
            let m = *mode;
            dnd_row = dnd_row.child(
                Button::new(("dnd-", i))
                    .label(dnd_label(m))
                    .appearance(if is_sel {
                        ButtonAppearance::Accent
                    } else {
                        ButtonAppearance::Subtle
                    })
                    .size(ButtonSize::Compact)
                    .on_click(move |_: &ClickEvent, _, app: &mut App| {
                        if is_sel {
                            return;
                        }
                        e.update(app, |model, cx| {
                            model.phone_state.pending_dnd = Some(m);
                            model.phone_state.pending_dnd_since = Some(Instant::now());
                            model.phone_state.error = None;
                            model.send_utility(Payload::AudioControl(AudioControl {
                                command: AudioControlCommand::SetDnd { mode: m },
                            }));
                            cx.notify();
                        });
                    }),
            );
        }
        dnd_card = dnd_card.child(dnd_row);
        grid = grid.child(dnd_card);

        // Battery + Connectivity row
        let battery_card = Card::new().padding(12.0).gap(10.0).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(spacing.md))
                .child(Icon::new("battery").size(IconSize::Md))
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.on_neutral)
                        .child("Battery"),
                )
                .child(
                    div()
                        .text_size(px(28.0))
                        .font_weight(FontWeight::BOLD)
                        .child(
                            self.status
                                .battery_status
                                .clone()
                                .unwrap_or_else(|| "—".to_owned()),
                        ),
                ),
        );

        let battery_with_bar = if let Some(p) = battery_pct {
            battery_card.child(
                div()
                    .w_full()
                    .h(px(spacing.md))
                    .rounded(px(6.0))
                    .bg(colors.surface_dim)
                    .border_1()
                    .border_color(colors.stroke_neutral_subtle)
                    .child(
                        div()
                            .w(px(PAIR_QR_SIDE * (p as f32) / 100.0))
                            .h_full()
                            .rounded(px(6.0))
                            .bg(if p > 20 {
                                colors.status_success
                            } else {
                                colors.status_warning
                            }),
                    ),
            )
        } else {
            battery_card
        };

        let conn_card = Card::new()
            .padding(12.0)
            .gap(8.0)
            .child(SectionHeader::new("Connectivity"));
        let wifi_label = self
            .status
            .wifi_summary
            .clone()
            .unwrap_or_else(|| "—".to_owned());
        let bt_label = match self.status.bluetooth_enabled {
            Some(true) => "On".to_owned(),
            Some(false) => "Off".to_owned(),
            None => "—".to_owned(),
        };
        let conn_card = conn_card
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(spacing.md))
                    .child(Icon::new("wifi").size(IconSize::Md))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.on_neutral)
                            .child("Wi-Fi"),
                    )
                    .child(div().text_color(colors.on_subtle).child(wifi_label)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(spacing.md))
                    .child(Icon::new("bluetooth").size(IconSize::Md))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.on_neutral)
                            .child("Bluetooth"),
                    )
                    .child(div().text_color(colors.on_subtle).child(bt_label)),
            );

        grid = grid.child(
            div()
                .flex()
                .flex_row()
                .gap(px(spacing.xl))
                .child(div().flex_1().child(battery_with_bar))
                .child(div().flex_1().child(conn_card)),
        );

        if let Some(err) = error_msg {
            grid = grid.child(error_banner(&err, cx));
        }

        div()
            .id("phone-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(colors.surface)
            .p(px(spacing.xxl))
            .child(grid)
            .into_any_element()
    }

    fn render_notifications_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;
        let radii = cx.theme().radii;
        let entity = cx.entity();
        let count = self.notifications.item_count();
        let feature_status = self.status.feature(UtilityFeature::Notifications).cloned();

        if !self.connected() {
            let (title, body) = if self.reconnecting {
                (
                    "Reconnecting…",
                    "Your phone disconnected. Waiting to reconnect automatically.",
                )
            } else {
                (
                    "Your phone's notifications, here",
                    "Once paired, alerts from your phone show up here so you can read and reply without picking it up.",
                )
            };
            return div()
                .size_full()
                .bg(colors.surface)
                .child(disconnected_placeholder("bell", title, body, cx))
                .into_any_element();
        }

        let e_hide = entity.clone();
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing.lg))
            .px(px(20.0))
            .py(px(spacing.lg))
            .border_b_1()
            .border_color(colors.stroke_neutral_subtle)
            .child(Label::new("Notifications").size(LabelSize::Subtitle))
            .child(
                div()
                    .min_w(px(20.0))
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(radii.pill))
                    .bg(colors.neutral)
                    .border_1()
                    .border_color(colors.stroke_neutral_subtle)
                    .text_color(colors.on_subtle)
                    .text_size(px(typography.caption.size))
                    .tabular_nums()
                    .child(format!("{count}")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_color(colors.on_subtle)
                            .text_size(px(typography.body.size))
                            .child("Hide sensitive"),
                    )
                    .child(
                        Switch::new("hide-sensitive-switch")
                            .on(self.notifications.hide_sensitive)
                            .on_click(move |new_val, _, _, app| {
                                e_hide.update(app, |m, cx| {
                                    m.notifications.hide_sensitive = new_val;
                                    cx.notify();
                                });
                            }),
                    ),
            );

        let mut body = div()
            .id("notif-scroll")
            .flex_1()
            .overflow_y_scroll()
            .p(px(20.0))
            .flex()
            .flex_col()
            .gap(px(spacing.md));

        if let Some(status) = feature_status.as_ref()
            && let Some(banner) = permission_banner(
                status,
                "Enable Notification access on the phone to see alerts here.",
                onboarding_action(entity.clone()),
                cx,
            )
        {
            body = body.child(banner);
        }

        if count == 0 && self.notifications.show_initial_skeleton() {
            for idx in 0..5 {
                body = body.child(SkeletonRow::new(format!("notif-loading-{idx}")));
            }
        } else if count == 0 {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .py(px(60.0))
                    .flex_col()
                    .gap(px(spacing.md))
                    .text_color(colors.on_subtle_disabled)
                    .child(Icon::new("check").size(IconSize::Lg))
                    .child(div().italic().child("You're all caught up.")),
            );
        } else {
            let items: Vec<_> = self
                .notifications
                .iter_items()
                .map(|(id, n)| (id.clone(), n.clone()))
                .collect();
            let now_ms = current_time_ms();
            let (now_items, earlier_items): (Vec<_>, Vec<_>) =
                items.into_iter().partition(|(_, notif)| {
                    now_ms.saturating_sub(notif.timestamp_unix_ms) <= 30 * 60 * 1000
                });
            for (group_label, group_items) in [("Now", now_items), ("Earlier", earlier_items)] {
                if group_items.is_empty() {
                    continue;
                }
                body = body.child(Label::eyebrow(group_label));
                for (id, notif) in group_items {
                    let suppressed = self.notifications.is_suppressed(&notif.app_package);
                    let hide = self.notifications.hide_sensitive && notif.sensitive;
                    let app_package = notif.app_package.clone();
                    let notif_id = notif.notification_id.clone();
                    let meta_time = relative_timestamp(notif.timestamp_unix_ms);

                    let mut meta = div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(typography.caption.size))
                        .text_color(colors.on_subtle)
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(notif.app_name.clone()),
                        )
                        .child(div().child("·"))
                        .child(div().child(meta_time));
                    if hide {
                        meta = meta.child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(radii.pill))
                                .bg(colors.status_warning_bg)
                                .text_color(colors.status_warning)
                                .text_size(px(10.0))
                                .child("sensitive"),
                        );
                    }
                    if suppressed {
                        meta = meta.child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(radii.pill))
                                .bg(colors.neutral)
                                .text_color(colors.on_subtle_disabled)
                                .text_size(px(10.0))
                                .child("muted"),
                        );
                    }

                    let title_text = if hide {
                        "Content hidden".to_owned()
                    } else {
                        notif.title.clone().unwrap_or_default()
                    };

                    let mut body_col = div().flex_1().flex().flex_col().gap(px(3.0)).child(meta);
                    if !title_text.is_empty() {
                        body_col = body_col.child(
                            div()
                                .text_color(colors.on_neutral)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(typography.body.size))
                                .child(title_text),
                        );
                    }
                    if let Some(text) = &notif.text
                        && !hide
                        && !text.is_empty()
                    {
                        body_col = body_col.child(
                            div()
                                .text_color(colors.on_subtle)
                                .text_size(px(typography.body.size))
                                .child(text.clone()),
                        );
                    }

                    // Action buttons
                    if !notif.actions.is_empty() && !hide {
                        let mut actions_row = div()
                            .mt(px(spacing.sm))
                            .flex()
                            .flex_row()
                            .gap(px(spacing.sm));
                        for action in &notif.actions {
                            let nid = notif_id.clone();
                            let aid = action.action_id.clone();
                            let title = action.title.clone();
                            let allows_reply = action.allows_reply;
                            let e2 = entity.clone();
                            actions_row = actions_row.child(
                                Button::new(SharedString::from(format!("action-{nid}-{aid}")))
                                    .label(title)
                                    .appearance(ButtonAppearance::Subtle)
                                    .size(ButtonSize::Compact)
                                    .on_click(move |_: &ClickEvent, _, app: &mut App| {
                                        let nid2 = nid.clone();
                                        let aid2 = aid.clone();
                                        e2.update(app, |m, cx| {
                                            if allows_reply {
                                                let key = (nid2.clone(), aid2.clone());
                                                if m.quick_reply.open_on.as_ref() == Some(&key) {
                                                    m.quick_reply.open_on = None;
                                                } else {
                                                    m.quick_reply.open_on = Some(key);
                                                    m.quick_reply_input.update(cx, |t, cx| {
                                                        t.set_value("", cx);
                                                    });
                                                }
                                                cx.notify();
                                                return;
                                            }
                                            m.send_utility(Payload::NotificationAction(
                                                androidconnect_protocol::NotificationAction {
                                                    notification_id: nid2,
                                                    action_id: aid2,
                                                    reply_text: None,
                                                },
                                            ));
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        body_col = body_col.child(actions_row);

                        if let Some((open_nid, open_aid)) = &self.quick_reply.open_on
                            && *open_nid == notif.notification_id
                        {
                            let nid_send = notif.notification_id.clone();
                            let aid_send = open_aid.clone();
                            let e_send = entity.clone();
                            body_col =
                                body_col.child(
                                    div()
                                        .mt(px(spacing.sm))
                                        .flex()
                                        .flex_row()
                                        .gap(px(spacing.sm))
                                        .items_center()
                                        .child(div().flex_1().child(self.quick_reply_input.clone()))
                                        .child(
                                            Button::new(SharedString::from(format!(
                                                "qr-send-{nid_send}"
                                            )))
                                            .label("Send")
                                            .appearance(ButtonAppearance::Accent)
                                            .size(ButtonSize::Compact)
                                            .on_click(move |_: &ClickEvent, _, app: &mut App| {
                                                let nid = nid_send.clone();
                                                let aid = aid_send.clone();
                                                e_send.update(app, |m, cx| {
                                                    let text = m
                                                        .quick_reply_input
                                                        .read(cx)
                                                        .text()
                                                        .to_string();
                                                    if text.trim().is_empty() {
                                                        return;
                                                    }
                                                    m.send_utility(Payload::NotificationAction(
                                                    androidconnect_protocol::NotificationAction {
                                                        notification_id: nid,
                                                        action_id: aid,
                                                        reply_text: Some(text),
                                                    },
                                                ));
                                                    m.quick_reply.open_on = None;
                                                    m.quick_reply_input.update(cx, |t, cx| {
                                                        t.set_value("", cx);
                                                    });
                                                    cx.notify();
                                                });
                                            }),
                                        ),
                                );
                        }
                    }

                    let pkg_for_mute = app_package.clone();
                    let e_mute = entity.clone();
                    let nid_for_dismiss = notif_id.clone();
                    let e_dismiss = entity.clone();

                    let actions_col = div()
                        .flex()
                        .flex_col()
                        .gap(px(spacing.sm))
                        .child(
                            Button::new(SharedString::from(format!("notif-mute-{}", id)))
                                .label(if suppressed { "Unmute" } else { "Mute" })
                                .appearance(ButtonAppearance::Subtle)
                                .size(ButtonSize::Compact)
                                .on_click(move |_, _, app| {
                                    let pkg = pkg_for_mute.clone();
                                    e_mute.update(app, |m, cx| {
                                        let enabled = m.notifications.is_suppressed(&pkg);
                                        m.notifications.toggle_suppress(&pkg);
                                        m.send_utility(Payload::NotificationFilterUpdate(
                                            androidconnect_protocol::NotificationFilterUpdate {
                                                package_name: pkg,
                                                enabled,
                                            },
                                        ));
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("notif-close-{}", id)))
                                .label("✕")
                                .appearance(ButtonAppearance::Subtle)
                                .size(ButtonSize::Compact)
                                .on_click(move |_, _, app| {
                                    let nid = nid_for_dismiss.clone();
                                    e_dismiss.update(app, |m, cx| {
                                        m.notifications.removed(
                                            androidconnect_protocol::NotificationRemoved {
                                                notification_id: nid,
                                            },
                                        );
                                        cx.notify();
                                    });
                                }),
                        );

                    let hover_bg = colors.neutral_hover;
                    let card = Card::new().padding(0.0).child(
                        div()
                            .id(SharedString::from(format!("notif-card-{id}")))
                            .flex()
                            .flex_row()
                            .gap(px(spacing.lg))
                            .items_start()
                            .p(px(spacing.lg))
                            .rounded(px(radii.md))
                            .hover(move |s| s.bg(hover_bg))
                            .child(
                                AppDot::new(notif.app_name.clone())
                                    .hue(brand_hue(&notif.app_package, &notif.app_name))
                                    .size(28.0),
                            )
                            .child(body_col)
                            .child(actions_col),
                    );

                    body = body.child(card);
                }
            }
        }

        div()
            .size_full()
            .bg(colors.surface)
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_files_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;
        let radii = cx.theme().radii;
        let entity = cx.entity();

        if !self.connected() {
            let (title, body) = if self.reconnecting {
                (
                    "Reconnecting…",
                    "Your phone disconnected. Waiting to reconnect automatically.",
                )
            } else {
                (
                    "Browse your phone's storage",
                    "Drag and drop files between your phone and PC once paired.",
                )
            };
            return div()
                .size_full()
                .bg(colors.surface)
                .child(disconnected_placeholder("folder", title, body, cx))
                .into_any_element();
        }

        if let Some(p) = self.files_state.ensure_initial_browse() {
            self.send_utility(p);
        }

        let current_path = self.files_state.current_path.clone();
        let pending = self.files_state.pending_request.is_some();
        let response = self
            .files_state
            .responses
            .get(&self.files_state.current_path)
            .cloned();
        let selected_entry = response.as_ref().and_then(|resp| {
            self.files_state.selected_path.as_ref().and_then(|path| {
                resp.entries
                    .iter()
                    .find(|entry| &entry.path == path)
                    .cloned()
            })
        });

        let e_refresh = entity.clone();
        let e_back = entity.clone();
        let e_new_folder = entity.clone();
        let path_for_refresh = current_path.clone();
        let view_mode = self.files_state.view_mode;
        let breadcrumb = files_breadcrumb(&current_path, entity.clone(), cx);
        let e_list = entity.clone();
        let e_grid = entity.clone();

        let toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing.md))
            .px(px(20.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(colors.stroke_neutral_subtle)
            .child(
                Button::new("files-back")
                    .icon("icons/back.svg")
                    .label("Back")
                    .appearance(ButtonAppearance::Subtle)
                    .size(ButtonSize::Compact)
                    .disabled(current_path == "/")
                    .on_click(move |_, _, app| {
                        e_back.update(app, |m, cx| {
                            if let Some(p) = m.files_state.navigate_back() {
                                m.send_utility(p);
                            }
                            cx.notify();
                        });
                    }),
            )
            .child(
                Button::new("files-refresh")
                    .icon("icons/refresh.svg")
                    .label("Refresh")
                    .appearance(ButtonAppearance::Subtle)
                    .size(ButtonSize::Compact)
                    .on_click(move |_, _, app| {
                        let path = path_for_refresh.clone();
                        e_refresh.update(app, |m, cx| {
                            let p = m.files_state.request_browse(path);
                            m.send_utility(p);
                            cx.notify();
                        });
                    }),
            )
            .child(
                Button::new("files-new-folder")
                    .label("New folder")
                    .appearance(ButtonAppearance::Subtle)
                    .size(ButtonSize::Compact)
                    .on_click(move |_, _, app| {
                        e_new_folder.update(app, |m, cx| {
                            m.file_action = FileActionDialog {
                                kind: Some(FileActionKind::CreateFolder),
                                target_path: Some(m.files_state.current_path.clone()),
                            };
                            m.file_action_input.update(cx, |t, cx| {
                                t.set_value("", cx);
                                t.set_placeholder("New folder name");
                                cx.notify();
                            });
                            cx.notify();
                        });
                    }),
            )
            .child(breadcrumb)
            .child(div().flex_1())
            .child(
                ToggleButton::new("files-view-list")
                    .label("List")
                    .selected(matches!(view_mode, FilesViewMode::List))
                    .size(ButtonSize::Compact)
                    .on_click(move |_, _, _, app| {
                        e_list.update(app, |m, cx| {
                            m.files_state.view_mode = FilesViewMode::List;
                            cx.notify();
                        });
                    }),
            )
            .child(
                ToggleButton::new("files-view-grid")
                    .label("Grid")
                    .selected(matches!(view_mode, FilesViewMode::Grid))
                    .size(ButtonSize::Compact)
                    .on_click(move |_, _, _, app| {
                        e_grid.update(app, |m, cx| {
                            m.files_state.view_mode = FilesViewMode::Grid;
                            cx.notify();
                        });
                    }),
            );

        let mut body = div()
            .id("files-scroll")
            .flex_1()
            .overflow_y_scroll()
            .p(px(spacing.lg))
            .flex()
            .flex_col()
            .gap(px(spacing.xs));

        // Inline action dialog
        if let Some(kind) = self.file_action.kind {
            let title = match kind {
                FileActionKind::CreateFolder => "New folder",
                FileActionKind::Rename => "Rename",
            };
            let target = self.file_action.target_path.clone().unwrap_or_default();
            let e_submit = entity.clone();
            let e_cancel = entity.clone();
            body = body.child(
                Card::new().padding(12.0).gap(8.0).child(
                    div()
                        .text_color(colors.on_neutral)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{title}: {target}")),
                ).child(self.file_action_input.clone()).child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(6.0))
                        .child(
                            Button::new("file-action-submit")
                                .label("Submit")
                                .appearance(ButtonAppearance::Accent)
                                .size(ButtonSize::Compact)
                                .on_click(move |_, _, app| {
                                    e_submit.update(app, |m, cx| {
                                        let value = m.file_action_input.read(cx).text().to_string();
                                        let trimmed = value.trim().to_owned();
                                        if trimmed.is_empty() {
                                            return;
                                        }
                                        let Some(kind) = m.file_action.kind else { return };
                                        let Some(target) = m.file_action.target_path.clone() else {
                                            return;
                                        };
                                        let parent_path = m.files_state.current_path.clone();
                                        let payload = match kind {
                                            FileActionKind::CreateFolder => Payload::FileMutation(
                                                androidconnect_protocol::FileMutation {
                                                    request_id: format!("mut-{}", new_request_token()),
                                                    mutation:
                                                        androidconnect_protocol::FileMutationKind::CreateFolder,
                                                    path: target.clone(),
                                                    new_path: Some(trimmed),
                                                },
                                            ),
                                            FileActionKind::Rename => {
                                                let new_path = if let Some(slash_idx) = target.rfind('/') {
                                                    format!("{}/{trimmed}", &target[..slash_idx])
                                                } else {
                                                    trimmed
                                                };
                                                Payload::FileMutation(
                                                    androidconnect_protocol::FileMutation {
                                                        request_id: format!("mut-{}", new_request_token()),
                                                        mutation:
                                                            androidconnect_protocol::FileMutationKind::Rename,
                                                        path: target.clone(),
                                                        new_path: Some(new_path),
                                                    },
                                                )
                                            }
                                        };
                                        m.send_utility(payload);
                                        let refresh = m.files_state.request_browse(parent_path);
                                        m.send_utility(refresh);
                                        m.file_action = FileActionDialog::default();
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            Button::new("file-action-cancel")
                                .label("Cancel")
                                .appearance(ButtonAppearance::Subtle)
                                .size(ButtonSize::Compact)
                                .on_click(move |_, _, app| {
                                    e_cancel.update(app, |m, cx| {
                                        m.file_action = FileActionDialog::default();
                                        cx.notify();
                                    });
                                }),
                        ),
                ),
            );
        }

        if pending && response.is_none() {
            for idx in 0..8 {
                body = body.child(file_skeleton_row(idx, cx));
            }
        } else if let Some(resp) = response {
            if let Some(banner) = permission_banner(
                &resp.status,
                "Cannot browse this folder.",
                onboarding_action(entity.clone()),
                cx,
            ) {
                body = body.child(banner);
            }

            if matches!(view_mode, FilesViewMode::Grid) {
                let mut grid = div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(spacing.lg))
                    .p(px(spacing.xl));
                for entry in &resp.entries {
                    let icon = file_icon_name(entry);
                    let icon_color = filetype_colour(entry, &colors);
                    let meta_label = file_entry_meta(entry);
                    let entry_path = entry.path.clone();
                    let entry_type = entry.entry_type;
                    let entry_name = entry.name.clone();
                    let selected = self.files_state.selected_path.as_deref() == Some(&entry.path);
                    let e_select = entity.clone();
                    let tile_bg = if selected {
                        tint(colors.accent, 0.12)
                    } else {
                        colors.neutral
                    };
                    let tile_border = if selected {
                        colors.accent
                    } else {
                        colors.stroke_neutral_subtle
                    };
                    let thumb_bg = if file_is_image(entry) {
                        gradient_from_hue(hue_for_filename(&entry.name))
                    } else {
                        colors.surface_dim.into()
                    };

                    grid = grid.child(
                        div()
                            .id(SharedString::from(format!("file-tile-{entry_path}")))
                            .flex()
                            .flex_col()
                            .flex_basis(px(FILE_TILE_WIDTH))
                            .max_w(px(FILE_TILE_MAX_WIDTH))
                            .gap(px(spacing.md))
                            .p(px(spacing.lg))
                            .rounded(px(radii.md))
                            .border_1()
                            .border_color(tile_border)
                            .bg(tile_bg)
                            .cursor_pointer()
                            .hover(move |s| s.bg(colors.neutral_hover))
                            .on_click(move |_, _, app| {
                                let path = entry_path.clone();
                                let is_dir = matches!(entry_type, FileEntryType::Directory);
                                e_select.update(app, |m, cx| {
                                    let now = Instant::now();
                                    let is_double = m.files_state.last_click_path.as_deref()
                                        == Some(path.as_str())
                                        && m.files_state.last_click_time.is_some_and(|last| {
                                            now.duration_since(last) <= Duration::from_millis(350)
                                        });
                                    m.files_state.last_click_path = Some(path.clone());
                                    m.files_state.last_click_time = Some(now);
                                    m.files_state.selected_path = Some(path.clone());
                                    if is_double {
                                        if is_dir {
                                            let payload = m.files_state.navigate_to(path);
                                            m.send_utility(payload);
                                        } else {
                                            m.request_download(path);
                                        }
                                    }
                                    cx.notify();
                                });
                            })
                            .child(
                                div()
                                    .w_full()
                                    .h(px(116.0))
                                    .rounded(px(radii.lg))
                                    .bg(thumb_bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(icon_color)
                                    .child(Icon::new(icon).size(IconSize::Lg)),
                            )
                            .child(
                                div()
                                    .text_color(colors.on_neutral)
                                    .text_size(px(typography.caption.size))
                                    .overflow_hidden()
                                    .child(truncate(&entry_name, 18)),
                            )
                            .child(
                                div()
                                    .text_color(colors.on_subtle)
                                    .text_size(px(10.0))
                                    .child(meta_label),
                            ),
                    );
                }
                body = body.child(grid);
            } else {
                for entry in &resp.entries {
                    let icon = file_icon_name(entry);
                    let icon_color = filetype_colour(entry, &colors);
                    let size_label = file_entry_meta(entry);
                    let selected = self.files_state.selected_path.as_deref() == Some(&entry.path);
                    let entry_path = entry.path.clone();
                    let entry_type = entry.entry_type;
                    let entry_name = entry.name.clone();
                    let e_nav = entity.clone();
                    let e_del = entity.clone();
                    let e_rename = entity.clone();
                    let e_download = entity.clone();
                    let e_path_del = entry.path.clone();
                    let e_path_rename = entry.path.clone();
                    let e_name_rename = entry.name.clone();
                    let e_path_download = entry.path.clone();

                    let row_bg = if selected {
                        tint(colors.accent, 0.12)
                    } else {
                        gpui::transparent_black()
                    };

                    let mut row = div()
                        .id(SharedString::from(format!("fentry-{entry_path}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(spacing.lg))
                        .py(px(6.0))
                        .rounded(px(radii.md))
                        .cursor_pointer()
                        .bg(row_bg)
                        .hover(move |s| s.bg(colors.subtle_hover))
                        .on_click(move |_, _, app| {
                            let p = entry_path.clone();
                            e_nav.update(app, |m, cx| {
                                let now = Instant::now();
                                let is_dir = matches!(entry_type, FileEntryType::Directory);
                                let is_double = m.files_state.last_click_path.as_deref()
                                    == Some(p.as_str())
                                    && m.files_state.last_click_time.is_some_and(|last| {
                                        now.duration_since(last) <= Duration::from_millis(350)
                                    });
                                m.files_state.last_click_path = Some(p.clone());
                                m.files_state.last_click_time = Some(now);
                                m.files_state.selected_path = Some(p.clone());
                                if is_double {
                                    if is_dir {
                                        let payload = m.files_state.navigate_to(p);
                                        m.send_utility(payload);
                                    } else {
                                        m.request_download(p);
                                    }
                                }
                                cx.notify();
                            });
                        })
                        .child(
                            div()
                                .text_color(icon_color)
                                .child(Icon::new(icon).size(IconSize::Sm)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_color(colors.on_neutral)
                                .font_weight(if matches!(entry_type, FileEntryType::Directory) {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_size(px(typography.body.size))
                                .child(entry_name),
                        )
                        .child(
                            div()
                                .w(px(80.0))
                                .text_color(colors.on_subtle)
                                .text_size(px(typography.caption.size))
                                .child(size_label),
                        );

                    if !matches!(entry_type, FileEntryType::Directory) {
                        row = row.child(
                            Button::new(SharedString::from(format!("dl-{}", entry.path)))
                                .label("Save to PC")
                                .appearance(ButtonAppearance::Subtle)
                                .size(ButtonSize::Compact)
                                .on_click(move |_, _, app| {
                                    let path = e_path_download.clone();
                                    e_download.update(app, |m, cx| {
                                        m.request_download(path);
                                        cx.notify();
                                    });
                                }),
                        );
                    }

                    row = row
                        .child(
                            Button::new(SharedString::from(format!("rn-{}", entry.path)))
                                .label("Rename")
                                .appearance(ButtonAppearance::Subtle)
                                .size(ButtonSize::Compact)
                                .on_click(move |_, _, app| {
                                    let path = e_path_rename.clone();
                                    let name = e_name_rename.clone();
                                    e_rename.update(app, |m, cx| {
                                        m.file_action = FileActionDialog {
                                            kind: Some(FileActionKind::Rename),
                                            target_path: Some(path),
                                        };
                                        m.file_action_input.update(cx, |t, cx| {
                                            t.set_value(name, cx);
                                            t.set_placeholder("New name");
                                            cx.notify();
                                        });
                                        cx.notify();
                                    });
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("del-{}", entry.path)))
                                .label("Delete")
                                .appearance(ButtonAppearance::Subtle)
                                .size(ButtonSize::Compact)
                                .on_click(move |_, _, app| {
                                    let path = e_path_del.clone();
                                    e_del.update(app, |m, cx| {
                                        let parent_path = m.files_state.current_path.clone();
                                        m.send_utility(Payload::FileMutation(
                                        androidconnect_protocol::FileMutation {
                                            request_id: format!("mut-{}", new_request_token()),
                                            mutation:
                                                androidconnect_protocol::FileMutationKind::Delete,
                                            path,
                                            new_path: None,
                                        },
                                    ));
                                        let refresh = m.files_state.request_browse(parent_path);
                                        m.send_utility(refresh);
                                        cx.notify();
                                    });
                                }),
                        );

                    body = body.child(row);
                }
            }
        } else {
            body = body.child(
                div()
                    .text_color(colors.on_subtle_disabled)
                    .italic()
                    .py(px(20.0))
                    .child("No data for this path."),
            );
        }

        let mut panel = div()
            .size_full()
            .bg(colors.surface)
            .flex()
            .flex_col()
            .child(toolbar)
            .child(body);

        if let Some(entry) = selected_entry {
            panel = panel.child(files_selection_footer(&entry, cx));
        }

        panel.into_any_element()
    }

    fn render_messages_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let typography = cx.theme().typography;
        let spacing = cx.theme().spacing;
        let radii = cx.theme().radii;
        let entity = cx.entity();

        if !self.connected() {
            let (title, body) = if self.reconnecting {
                (
                    "Reconnecting…",
                    "Your phone disconnected. Waiting to reconnect automatically.",
                )
            } else {
                (
                    "Your text threads, on your desktop",
                    "Send and receive SMS / RCS messages through your paired phone.",
                )
            };
            return div()
                .size_full()
                .bg(colors.surface)
                .child(disconnected_placeholder("chat", title, body, cx))
                .into_any_element();
        }

        // Clone only what we need from MessagesState to avoid copying all thread_messages.
        let threads = self.messages_state.threads.clone();
        let thread_status = self.messages_state.thread_status.clone();
        let active_thread = self.messages_state.active_thread.clone();
        let pending_thread_open = self.messages_state.pending_thread_open.clone();
        let pending_send = self.messages_state.pending_send.clone();
        let send_error = self.messages_state.send_error.clone();
        let messages: Vec<_> = active_thread
            .as_ref()
            .and_then(|id| self.messages_state.thread_messages.get(id))
            .cloned()
            .unwrap_or_default();
        let detail_status = active_thread
            .as_ref()
            .and_then(|id| self.messages_state.thread_detail_status.get(id))
            .cloned();
        let active_display_name = active_thread
            .as_ref()
            .and_then(|id| threads.iter().find(|t| &t.thread_id == id))
            .map(|t| t.display_name.clone())
            .unwrap_or_else(|| active_thread.clone().unwrap_or_default());

        let composer = self.sms_composer.clone();
        let submit_entity = entity.clone();
        self.sms_composer.update(cx, |input, _| {
            input.set_on_submit(move |text, app| {
                let body = text.to_string();
                submit_entity.update(app, |m, cx| {
                    if let Some(payload) = m.messages_state.build_send(body) {
                        m.send_utility(payload);
                        cx.notify();
                        true
                    } else {
                        false
                    }
                })
            });
        });

        // Thread list (left)
        let mut thread_list = div()
            .id("sms-thread-list")
            .w(px(280.0))
            .h_full()
            .bg(colors.panel_bg)
            .border_r_1()
            .border_color(colors.stroke_neutral_subtle)
            .flex()
            .flex_col();

        thread_list = thread_list.child(
            div()
                .px(px(spacing.lg))
                .py(px(spacing.lg))
                .border_b_1()
                .border_color(colors.stroke_neutral_subtle)
                .child(Label::new("Messages").size(LabelSize::Subtitle)),
        );

        let mut list = div()
            .id("sms-list-scroll")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p(px(6.0))
            .gap(px(spacing.xs));

        if let Some(status) = &thread_status
            && let Some(banner) = permission_banner(
                status,
                "SMS access required — enable in onboarding.",
                onboarding_action(entity.clone()),
                cx,
            )
        {
            list = list.child(banner);
        }

        if threads.is_empty() && thread_status.is_none() {
            list = list.child(
                div()
                    .text_color(colors.on_subtle_disabled)
                    .text_size(px(typography.caption.size))
                    .italic()
                    .px(px(spacing.lg))
                    .py(px(spacing.sm))
                    .child("Syncing messages from your phone…"),
            );
            for idx in 0..6 {
                list = list.child(SkeletonRow::new(format!("msg-thread-loading-{idx}")));
            }
        } else if threads.is_empty() {
            list = list.child(
                div()
                    .text_color(colors.on_subtle_disabled)
                    .italic()
                    .py(px(20.0))
                    .px(px(spacing.lg))
                    .child("No threads yet."),
            );
        } else {
            for thread in &threads {
                let is_active = active_thread.as_deref() == Some(thread.thread_id.as_str());
                let thread_id = thread.thread_id.clone();
                let e = entity.clone();
                let snippet = thread
                    .last_message
                    .clone()
                    .unwrap_or_else(|| "(no preview)".to_owned());
                let unread_badge: gpui::AnyElement = if thread.unread_count > 0 {
                    div()
                        .min_w(px(20.0))
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(radii.pill))
                        .bg(colors.accent)
                        .text_color(colors.on_accent)
                        .text_size(px(10.0))
                        .font_weight(FontWeight::BOLD)
                        .tabular_nums()
                        .child(format!("{}", thread.unread_count))
                        .into_any_element()
                } else {
                    div().into_any_element()
                };
                let initials = device_initials(&thread.display_name);
                let display_name = thread.display_name.clone();

                list = list.child(
                    div()
                        .id(SharedString::from(format!("thread-{thread_id}")))
                        .flex()
                        .flex_row()
                        .gap(px(10.0))
                        .px(px(spacing.lg))
                        .py(px(10.0))
                        .rounded(px(radii.md))
                        .bg(if is_active {
                            colors.neutral_selected
                        } else {
                            gpui::transparent_black()
                        })
                        .hover(move |s| {
                            if is_active {
                                s
                            } else {
                                s.bg(colors.subtle_hover)
                            }
                        })
                        .cursor_pointer()
                        .on_click(move |_, _, app| {
                            let id = thread_id.clone();
                            e.update(app, |m, cx| {
                                if let Some(payload) = m.messages_state.open_thread(id) {
                                    m.send_utility(payload);
                                }
                                cx.notify();
                            });
                        })
                        .child(Avatar::initials(initials).size(36.0))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(spacing.xs))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .flex_1()
                                                .text_color(colors.on_neutral)
                                                .text_size(px(typography.body.size))
                                                .font_weight(if thread.unread_count > 0 {
                                                    FontWeight::BOLD
                                                } else {
                                                    FontWeight::SEMIBOLD
                                                })
                                                .child(truncate(&display_name, 22).to_owned()),
                                        )
                                        .child(unread_badge),
                                )
                                .child(
                                    div()
                                        .text_color(colors.on_subtle)
                                        .text_size(px(typography.caption.size))
                                        .child(truncate(&snippet, 36).to_owned()),
                                ),
                        ),
                );
            }
        }

        thread_list = thread_list.child(list);

        // Right column: active conversation
        let conversation: gpui::AnyElement = if let Some(active_id) = &active_thread {
            let pending_load = pending_thread_open.contains_key(active_id.as_str());
            let e_send = entity.clone();
            let e_close = entity.clone();
            let composer_for_send = composer.clone();
            let send_pending = pending_send.is_some();
            let display_name = active_display_name.clone();

            let mut conv = div().flex_1().h_full().flex().flex_col().bg(colors.surface);

            // Header
            conv = conv.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(20.0))
                    .py(px(spacing.lg))
                    .border_b_1()
                    .border_color(colors.stroke_neutral_subtle)
                    .child(Avatar::initials(device_initials(&display_name)).size(32.0))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_color(colors.on_neutral)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(display_name),
                            )
                            .child(
                                div()
                                    .text_color(colors.on_subtle)
                                    .text_size(px(typography.caption.size))
                                    .child("SMS"),
                            ),
                    )
                    .child(
                        Button::new("close-thread")
                            .label("Close")
                            .appearance(ButtonAppearance::Subtle)
                            .size(ButtonSize::Compact)
                            .on_click(move |_, _, app| {
                                e_close.update(app, |m, cx| {
                                    m.messages_state.close_thread();
                                    cx.notify();
                                });
                            }),
                    ),
            );

            let mut history = div()
                .id("sms-history")
                .flex_1()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(20.0))
                .py(px(spacing.xl));

            if let Some(status) = detail_status
                && let Some(banner) = permission_banner(
                    &status,
                    "Cannot read this thread.",
                    onboarding_action(entity.clone()),
                    cx,
                )
            {
                history = history.child(banner);
            }

            if pending_load && messages.is_empty() {
                history = history.child(
                    div()
                        .text_color(colors.on_subtle_disabled)
                        .italic()
                        .child("Loading conversation…"),
                );
            } else if messages.is_empty() {
                history = history.child(
                    div()
                        .text_color(colors.on_subtle_disabled)
                        .italic()
                        .child("No messages in this thread yet."),
                );
            } else {
                let mut last_ts = None;
                for entry in &messages {
                    if last_ts
                        .is_none_or(|prev| entry.timestamp_unix_ms.abs_diff(prev) > 30 * 60 * 1000)
                    {
                        history = history.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .py(px(spacing.md))
                                .text_color(colors.on_subtle_disabled)
                                .text_size(px(typography.caption.size))
                                .child(format_full_time(entry.timestamp_unix_ms)),
                        );
                    }
                    last_ts = Some(entry.timestamp_unix_ms);
                    let outbound = matches!(entry.direction, MessageDirection::Outbound);
                    let bubble = div()
                        .max_w(px(420.0))
                        .px(px(spacing.lg))
                        .py(px(spacing.md))
                        .rounded(px(14.0))
                        .bg(if outbound {
                            colors.accent
                        } else {
                            colors.neutral
                        })
                        .text_color(if outbound {
                            colors.on_accent
                        } else {
                            colors.on_neutral
                        })
                        .text_size(px(typography.body.size))
                        .child(entry.body.clone());
                    let mut row = div().flex().flex_row().gap(px(spacing.sm));
                    if outbound {
                        row = row.child(div().flex_1()).child(bubble);
                    } else {
                        row = row.child(bubble).child(div().flex_1());
                    }
                    history = history.child(row);
                }
            }

            conv = conv.child(history);

            // Composer
            let composer_row = div()
                .flex()
                .flex_row()
                .gap(px(spacing.md))
                .items_center()
                .px(px(20.0))
                .py(px(spacing.lg))
                .border_t_1()
                .border_color(colors.stroke_neutral_subtle)
                .child(div().flex_1().child(composer_for_send))
                .child(
                    Button::new("sms-send")
                        .icon("icons/send.svg")
                        .label(if send_pending { "Sending…" } else { "Send" })
                        .appearance(ButtonAppearance::Accent)
                        .disabled(send_pending)
                        .on_click(move |_, _, app| {
                            e_send.update(app, |m, cx| {
                                let body = m.sms_composer.read(cx).text().to_string();
                                if let Some(payload) = m.messages_state.build_send(body) {
                                    m.send_utility(payload);
                                    m.sms_composer.update(cx, |t, cx| {
                                        t.set_value("", cx);
                                    });
                                }
                                cx.notify();
                            });
                        }),
                );
            conv = conv.child(composer_row);
            if let Some(err) = send_error {
                conv = conv.child(error_banner(&err, cx));
            }
            conv.into_any_element()
        } else {
            div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.on_subtle_disabled)
                .italic()
                .bg(colors.surface)
                .child("Pick a conversation to view the thread.")
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(colors.surface)
            .child(thread_list)
            .child(conversation)
            .into_any_element()
    }

    /// Open a native save-dialog for `path` and emit a `FileTransferRequest`. The incoming
    /// transfer machinery in `network.rs` writes to its default download directory; the desktop
    /// then moves the completed file to the user's chosen destination via
    /// `download_destinations`.
    fn request_download(&mut self, path: String) {
        let suggested = path
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(&path)
            .to_owned();
        let chosen = rfd::FileDialog::new().set_file_name(&suggested).save_file();
        let Some(dest) = chosen else { return };
        if let Ok(mut map) = self.download_destinations.lock() {
            map.insert(path.clone(), dest);
        }
        self.send_utility(Payload::FileTransferRequest(
            androidconnect_protocol::FileTransferRequest {
                request_id: format!("dl-{}", new_request_token()),
                path: path.clone(),
            },
        ));
        let filename = path.rsplit_once('/').map(|(_, n)| n).unwrap_or(&path);
        self.activity
            .push(ActivityIcon::Download, format!("Saved {filename}"));
    }
}

impl Render for AppModel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let connected = self.connected();

        // Ensure video texture is allocated
        if self.frame_w > 0 && self.frame_h > 0 && self.video_texture.is_none() {
            self.video_texture = window.alloc_video_texture(self.frame_w, self.frame_h).ok();
            self.video_alloc_w = self.frame_w;
            self.video_alloc_h = self.frame_h;
        }

        // Refresh QR when on pair screen
        if !connected || self.pairing_requested {
            self.refresh_pair_qr(window);
        }

        let top_bar = self.render_top_bar(cx);
        let sidebar = self.render_sidebar(cx);
        let status_bar = self.render_status_bar(cx);

        let content = if self.pairing_requested {
            self.render_pair_panel(cx).into_any_element()
        } else {
            match self.active_panel {
                Panel::Mirror => self.render_mirror_panel(cx).into_any_element(),
                Panel::Settings => self.render_settings_panel(cx).into_any_element(),
                _ if !connected => self.render_pair_panel(cx).into_any_element(),
                Panel::Overview => self.render_overview_panel(cx).into_any_element(),
                Panel::Notifications => self.render_notifications_panel(cx).into_any_element(),
                Panel::Messages => self.render_messages_panel(cx).into_any_element(),
                Panel::Files => self.render_files_panel(cx).into_any_element(),
                Panel::Phone => self.render_phone_panel(cx).into_any_element(),
            }
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.surface)
            .child(self.title_bar.clone())
            .child(top_bar)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .min_h_0()
                    .child(sidebar)
                    .child(div().flex_1().min_w_0().child(content)),
            )
            .child(status_bar)
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn local_pos(pos: Point<Pixels>, bounds: Bounds<Pixels>) -> (f64, f64) {
    (
        f32::from(pos.x - bounds.origin.x) as f64,
        f32::from(pos.y - bounds.origin.y) as f64,
    )
}

fn mirror_chrome_opacity(last_move: Option<Instant>) -> f32 {
    let Some(last_move) = last_move else {
        return 0.0;
    };
    let elapsed = last_move.elapsed();
    if elapsed < Duration::from_secs(3) {
        1.0
    } else if elapsed < Duration::from_millis(3200) {
        1.0 - ((elapsed - Duration::from_secs(3)).as_secs_f32() / 0.2).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn dnd_label(mode: DndMode) -> &'static str {
    match mode {
        DndMode::Off => "Off",
        DndMode::Priority => "Priority",
        DndMode::Alarms => "Alarms",
        DndMode::TotalSilence => "Silence",
    }
}

fn build_qr_pixels(uri: &str) -> Option<(Vec<u8>, u32)> {
    const QUIET_MODULES: usize = 4;
    const MIN_MODULE_PX: usize = 8;

    let code = QrCode::new(uri.as_bytes()).ok()?;
    let modules = code.width();
    let colors = code.to_colors();
    let total_modules = modules + QUIET_MODULES * 2;
    let size_px = total_modules * MIN_MODULE_PX;

    let mut pixels = vec![0xffu8; size_px * size_px * 4];
    for my in 0..modules {
        for mx in 0..modules {
            if colors[my * modules + mx] != QrColor::Dark {
                continue;
            }
            let x0 = (mx + QUIET_MODULES) * MIN_MODULE_PX;
            let y0 = (my + QUIET_MODULES) * MIN_MODULE_PX;
            for py in 0..MIN_MODULE_PX {
                for px in 0..MIN_MODULE_PX {
                    let i = ((y0 + py) * size_px + (x0 + px)) * 4;
                    pixels[i] = 0;
                    pixels[i + 1] = 0;
                    pixels[i + 2] = 0;
                    pixels[i + 3] = 0xff;
                }
            }
        }
    }
    Some((pixels, size_px as u32))
}

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let kept: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{kept}…")
    } else {
        kept
    }
}

fn new_request_token() -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or_default();
    format!("{micros}-{}", std::process::id())
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

fn pair_countdown_label(expires_at_ms: u64) -> String {
    let remaining_secs = expires_at_ms.saturating_sub(current_time_ms()) / 1000;
    let minutes = remaining_secs / 60;
    let seconds = remaining_secs % 60;
    format!("Code expires in {minutes}:{seconds:02}")
}

fn advertised_pair_addresses(bind: &str) -> Vec<String> {
    advertised_pair_addresses_from(
        bind,
        std::env::var("ANDROIDCONNECT_ADVERTISE_ADDR").ok(),
        discover_default_route_addresses(),
        std::env::var("HOSTNAME").ok(),
    )
}

fn advertised_pair_addresses_from(
    bind: &str,
    configured: Option<String>,
    discovered_ips: Vec<IpAddr>,
    host_name: Option<String>,
) -> Vec<String> {
    let mut addresses = BTreeSet::new();
    let mut wildcard_port = None;
    let configured = configured.unwrap_or_default();
    for value in configured
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        addresses.insert(value.to_owned());
    }
    if let Ok(socket) = bind.parse::<SocketAddr>() {
        if socket.ip().is_unspecified() {
            wildcard_port = Some(socket.port());
            for ip in discovered_ips {
                if !ip.is_unspecified() && !ip.is_loopback() {
                    addresses.insert(format_socket_addr(ip, socket.port()));
                }
            }
        } else {
            addresses.insert(socket.to_string());
        }
    } else if !bind.trim().is_empty() {
        addresses.insert(bind.trim().to_owned());
    }
    if addresses.is_empty()
        && let Some(port) = wildcard_port
        && let Some(host) = host_name
            .as_deref()
            .map(str::trim)
            .filter(|host| !host.is_empty())
    {
        addresses.insert(format!("{host}:{port}"));
    }
    if addresses.is_empty() && wildcard_port.is_none() && !bind.trim().is_empty() {
        addresses.insert(bind.trim().to_owned());
    }
    addresses.into_iter().collect()
}

fn discover_default_route_addresses() -> Vec<IpAddr> {
    let mut addresses = Vec::new();
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0")
        && socket.connect("8.8.8.8:80").is_ok()
        && let Ok(local) = socket.local_addr()
    {
        addresses.push(local.ip());
    }
    if let Ok(socket) = UdpSocket::bind("[::]:0")
        && socket.connect("[2001:4860:4860::8888]:80").is_ok()
        && let Ok(local) = socket.local_addr()
    {
        addresses.push(local.ip());
    }
    addresses
}

fn format_socket_addr(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(ip) => format!("{ip}:{port}"),
        IpAddr::V6(ip) => format!("[{ip}]:{port}"),
    }
}

// ── Render helpers ────────────────────────────────────────────────────────

fn section_header(title: &'static str, cx: &Context<AppModel>) -> impl IntoElement {
    let spacing = cx.theme().spacing;
    div()
        .px(px(spacing.xl))
        .pt(px(spacing.lg))
        .pb(px(6.0))
        .child(Label::eyebrow(title))
}

fn action_icon_button(
    id: &'static str,
    icon_name: &'static str,
    cx: &Context<AppModel>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let radii = cx.theme().radii;
    div()
        .id(id)
        .size(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radii.md))
        .text_color(colors.on_subtle)
        .cursor_pointer()
        .hover(move |s| s.bg(colors.subtle_hover))
        .on_click(on_click)
        .child(Icon::new(icon_name).size(IconSize::Sm))
}

fn mirror_chip_icon(
    id: &'static str,
    icon_name: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    _cx: &Context<AppModel>,
) -> impl IntoElement {
    div()
        .id(id)
        .size(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .text_color(gpui::white())
        .cursor_pointer()
        .hover(|s| s.bg(mirror_chip_hover_fill()))
        .on_click(on_click)
        .child(Icon::new(icon_name).size(IconSize::Sm))
}

// Mirror glass uses literal alpha overlays because backdrop blur chips sit
// over video frames rather than themed app surfaces.
fn mirror_glass_fill() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.25)
}

fn mirror_glass_stroke() -> Hsla {
    hsla(0.0, 0.0, 1.0, 0.08)
}

fn mirror_chip_hover_fill() -> Hsla {
    hsla(0.0, 0.0, 1.0, 0.12)
}

fn stat_chip(icon_name: &'static str, value: &str, cx: &Context<AppModel>) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let spacing = cx.theme().spacing;
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing.sm))
        .text_color(colors.on_subtle)
        .child(Icon::new(icon_name).size(IconSize::Sm))
        .child(div().tabular_nums().child(value.to_owned()))
}

fn heartbeat_dot(cx: &Context<AppModel>) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let radii = cx.theme().radii;
    let dot_color = colors.status_success;

    div()
        .size(px(6.0))
        .rounded(px(radii.pill))
        .bg(dot_color)
        .with_animation(
            "status-heartbeat-dot",
            Animation::new(Duration::from_millis(1400))
                .repeat()
                .with_easing(pulsating_between(0.35, 1.0)),
            move |this, delta| {
                let mut color = dot_color;
                color.a = delta;
                this.bg(color)
            },
        )
}

fn file_skeleton_row(idx: usize, cx: &Context<AppModel>) -> impl IntoElement {
    let spacing = cx.theme().spacing;
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(spacing.lg))
        .py(px(6.0))
        .child(Skeleton::new(("file-loading-icon", idx)).w(16.0).h(16.0))
        .child(Skeleton::new(("file-loading-line", idx)).w(220.0).h(10.0))
}

fn files_breadcrumb(
    current_path: &str,
    entity: Entity<AppModel>,
    cx: &Context<AppModel>,
) -> impl IntoElement + use<> {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing.sm))
        .text_size(px(typography.body.size));

    let root_entity = entity.clone();
    row = row.child(
        div()
            .id("files-breadcrumb-root")
            .cursor_pointer()
            .text_color(colors.on_neutral)
            .hover(move |s| s.text_color(colors.on_neutral_accent))
            .on_click(move |_, _, app| {
                root_entity.update(app, |m, cx| {
                    let payload = m.files_state.navigate_to("/".to_owned());
                    m.send_utility(payload);
                    cx.notify();
                });
            })
            .child("Phone"),
    );

    let mut prefix = String::new();
    for segment in current_path
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        prefix.push('/');
        prefix.push_str(segment);
        let target = prefix.clone();
        let e = entity.clone();
        row = row
            .child(
                div()
                    .text_color(colors.on_subtle_disabled)
                    .child(Icon::new("chevron").size(IconSize::Sm)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("files-breadcrumb-{target}")))
                    .cursor_pointer()
                    .text_color(colors.on_neutral)
                    .hover(move |s| s.text_color(colors.on_neutral_accent))
                    .on_click(move |_, _, app| {
                        let target = target.clone();
                        e.update(app, |m, cx| {
                            let payload = m.files_state.navigate_to(target);
                            m.send_utility(payload);
                            cx.notify();
                        });
                    })
                    .child(segment.to_owned()),
            );
    }

    row
}

fn files_selection_footer(entry: &FileEntry, cx: &Context<AppModel>) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let icon = file_icon_name(entry);
    let icon_color = filetype_colour(entry, &colors);
    let meta = file_entry_meta(entry);
    let modified = file_entry_modified(entry);

    div()
        .border_t_1()
        .border_color(colors.stroke_neutral_subtle)
        .bg(colors.surface_dim)
        .px(px(20.0))
        .py(px(spacing.md))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing.sm))
        .text_size(px(typography.caption.size))
        .child(
            div()
                .text_color(icon_color)
                .child(Icon::new(icon).size(IconSize::Sm)),
        )
        .child(
            div()
                .text_color(colors.on_neutral)
                .font_weight(FontWeight::SEMIBOLD)
                .child(entry.name.clone()),
        )
        .child(div().text_color(colors.on_subtle).child("·"))
        .child(div().text_color(colors.on_subtle).child(meta))
        .child(div().text_color(colors.on_subtle).child("·"))
        .child(div().text_color(colors.on_subtle).child(modified))
}

fn file_icon_name(entry: &FileEntry) -> &'static str {
    match entry.entry_type {
        FileEntryType::Directory => "folder",
        FileEntryType::Media if file_is_video(entry) => "video",
        FileEntryType::Media => "image",
        FileEntryType::File if file_is_archive(entry) => "zip",
        FileEntryType::File => "doc",
    }
}

fn filetype_colour(entry: &FileEntry, colors: &fluent_core::ColorScheme) -> Hsla {
    // File type hues are illustrative category colors, not app surface tokens.
    match entry.entry_type {
        FileEntryType::Directory => colors.on_neutral_accent,
        FileEntryType::Media if file_is_video(entry) => hsla(350.0 / 360.0, 0.6, 0.55, 1.0),
        FileEntryType::Media => hsla(80.0 / 360.0, 0.6, 0.55, 1.0),
        FileEntryType::File if file_is_archive(entry) => hsla(110.0 / 360.0, 0.45, 0.55, 1.0),
        FileEntryType::File => hsla(200.0 / 360.0, 0.45, 0.55, 1.0),
    }
}

fn file_entry_meta(entry: &FileEntry) -> String {
    if matches!(entry.entry_type, FileEntryType::Directory) {
        entry
            .size_bytes
            .map(|count| format!("{count} items"))
            .unwrap_or_else(|| "Folder".to_owned())
    } else {
        entry
            .size_bytes
            .map(crate::status::format_bytes_short)
            .unwrap_or_else(|| "—".to_owned())
    }
}

fn file_entry_modified(entry: &FileEntry) -> String {
    entry
        .modified_unix_ms
        .map(relative_timestamp)
        .unwrap_or_else(|| "—".to_owned())
}

fn file_extension(entry: &FileEntry) -> String {
    entry
        .name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn file_is_video(entry: &FileEntry) -> bool {
    let ext = file_extension(entry);
    matches!(ext.as_str(), "mp4" | "mov" | "mkv" | "webm" | "avi")
}

fn file_is_archive(entry: &FileEntry) -> bool {
    let lower = entry.name.to_ascii_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".rar")
        || lower.ends_with(".7z")
}

fn file_is_image(entry: &FileEntry) -> bool {
    let ext = file_extension(entry);
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "heic" | "avif"
    )
}

fn hue_for_filename(name: &str) -> f32 {
    let mut hash: u32 = 2166136261;
    for b in name.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    (hash % 360) as f32
}

fn brand_hue(package: &str, fallback_name: &str) -> f32 {
    match package {
        "com.whatsapp" => 145.0,
        "com.google.android.apps.messaging" => 210.0,
        "com.spotify.music" => 140.0,
        "com.discord" => 235.0,
        "com.instagram.android" => 320.0,
        "com.slack" => 30.0,
        "com.google.android.gm" => 8.0,
        "com.facebook.katana" => 220.0,
        "com.twitter.android" | "com.x.android" => 210.0,
        "org.telegram.messenger" => 205.0,
        _ => fluent_core::hue_for(fallback_name),
    }
}

fn pending_dot(cx: &Context<AppModel>) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let radii = cx.theme().radii;
    div()
        .size(px(10.0))
        .rounded(px(radii.pill))
        .bg(colors.accent)
}

fn error_banner(msg: &str, cx: &Context<AppModel>) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let radii = cx.theme().radii;
    div()
        .px(px(spacing.lg))
        .py(px(spacing.md))
        .rounded(px(radii.md))
        .bg(colors.status_error_bg)
        .border_1()
        .border_color(colors.status_error_border)
        .text_color(colors.status_error)
        .text_size(px(typography.caption.size))
        .child(msg.to_owned())
}

type BannerAction = (
    SharedString,
    Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
);

fn onboarding_action(entity: Entity<AppModel>) -> Option<BannerAction> {
    Some((
        "Show permission steps".into(),
        Box::new(move |_, _, app| {
            entity.update(app, |m, cx| {
                m.show_permission_steps();
                cx.notify();
            });
        }),
    ))
}

fn permission_banner(
    status: &FeatureStatus,
    fallback: &str,
    action: Option<BannerAction>,
    cx: &Context<AppModel>,
) -> Option<gpui::AnyElement> {
    if matches!(status.state, FeatureState::Available) {
        return None;
    }
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let heading = match status.state {
        FeatureState::Available => return None,
        FeatureState::PermissionRequired => "Permission required",
        FeatureState::Disabled => "Feature disabled",
        FeatureState::Unsupported => "Not supported on this device",
        FeatureState::Error => "Error",
    };
    let message = if status.message.is_empty() {
        fallback.to_owned()
    } else {
        status.message.clone()
    };
    let mut row = div()
        .flex()
        .flex_row()
        .gap(px(spacing.lg))
        .items_start()
        .p(px(spacing.lg))
        .border_l_2()
        .border_color(colors.status_info)
        .child(Icon::new("bell").size(IconSize::Sm))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(spacing.xs))
                .child(
                    div()
                        .text_color(colors.on_neutral)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(typography.body.size))
                        .child(heading),
                )
                .child(
                    div()
                        .text_color(colors.on_subtle)
                        .text_size(px(typography.caption.size))
                        .child(message),
                ),
        );

    if matches!(status.state, FeatureState::PermissionRequired)
        && let Some((label, on_click)) = action
    {
        row = row.child(
            Button::new(SharedString::from(format!(
                "permission-action-{}",
                heading.to_ascii_lowercase().replace(' ', "-")
            )))
            .label(label)
            .appearance(ButtonAppearance::Accent)
            .size(ButtonSize::Compact)
            .on_click(move |event, window, app| {
                on_click(event, window, app);
            }),
        );
    }

    Some(Card::new().padding(0.0).child(row).into_any_element())
}

fn disconnected_placeholder(
    icon: &'static str,
    title: &'static str,
    body: &'static str,
    cx: &Context<AppModel>,
) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(spacing.lg))
                .max_w(px(380.0))
                .text_color(colors.on_subtle)
                .child(
                    div()
                        .size(px(56.0))
                        .rounded(px(14.0))
                        .bg(colors.neutral)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(colors.on_subtle)
                        .child(Icon::new(icon).size(IconSize::Lg)),
                )
                .child(Label::new(title).size(LabelSize::Subtitle))
                .child(
                    div()
                        .text_color(colors.on_subtle)
                        .text_size(px(typography.body.size))
                        .child(body),
                ),
        )
}

fn mirror_disconnected_panel(
    entity: Entity<AppModel>,
    cx: &Context<AppModel>,
) -> impl IntoElement + use<> {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let radii = cx.theme().radii;
    let pair_entity = entity.clone();

    div()
        .size_full()
        .bg(colors.surface)
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(spacing.xxl))
                .max_w(px(640.0))
                .p(px(40.0))
                .child(
                    div()
                        .size(px(96.0))
                        .rounded(px(3.0 * radii.lg))
                        .bg(tint(colors.accent, 0.18))
                        .border_1()
                        .border_color(tint(colors.accent, 0.35))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(colors.on_neutral_accent)
                        .child(Icon::new("mirror").size(IconSize::Lg)),
                )
                .child(Label::new("Mirror your phone").size(LabelSize::Display))
                .child(
                    div()
                        .max_w(px(460.0))
                        .text_center()
                        .text_color(colors.on_subtle)
                        .text_size(px(typography.body.size))
                        .child("Once paired, your phone's screen streams here at up to 60 fps with full pointer and keyboard control."),
                )
                .child(
                    Button::new("mirror-pair-device")
                        .label("Pair a device")
                        .appearance(ButtonAppearance::Accent)
                        .on_click(move |_, _, app| {
                            pair_entity.update(app, |m, cx| {
                                m.request_pairing();
                                cx.notify();
                            });
                        }),
                )
                .child(
                    Card::new()
                        .padding(20.0)
                        .gap(spacing.lg)
                        .child(Label::eyebrow("What you can do"))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap(px(spacing.lg))
                                .child(mirror_capability_cell(
                                    "mirror",
                                    "Mirror & control",
                                    "Stream the screen at up to 60 fps",
                                    cx,
                                ))
                                .child(mirror_capability_cell(
                                    "bell",
                                    "See notifications",
                                    "Reply without unlocking your phone",
                                    cx,
                                ))
                                .child(mirror_capability_cell(
                                    "chat",
                                    "Send messages",
                                    "SMS / RCS from your keyboard",
                                    cx,
                                ))
                                .child(mirror_capability_cell(
                                    "folder",
                                    "Browse files",
                                    "Drag and drop both ways",
                                    cx,
                                )),
                        ),
                ),
        )
}

fn mirror_capability_cell(
    icon: &'static str,
    title: &'static str,
    caption: &'static str,
    cx: &Context<AppModel>,
) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;

    div()
        .w(px(284.0))
        .flex()
        .flex_row()
        .items_start()
        .gap(px(spacing.md))
        .child(
            div()
                .size(px(28.0))
                .rounded(px(6.0))
                .bg(colors.surface_dim)
                .border_1()
                .border_color(colors.stroke_neutral_subtle)
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.on_neutral_accent)
                .child(Icon::new(icon).size(IconSize::Sm)),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(spacing.xs))
                .child(
                    div()
                        .text_color(colors.on_neutral)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(typography.body.size))
                        .child(title),
                )
                .child(
                    div()
                        .text_color(colors.on_subtle)
                        .text_size(px(typography.caption.size))
                        .child(caption),
                ),
        )
}

fn quick_action(
    id: &'static str,
    icon: &'static str,
    title: &'static str,
    sub: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &Context<AppModel>,
) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let radii = cx.theme().radii;
    Card::new()
        .id(id)
        .flex_1()
        .row()
        .items_center()
        .hoverable()
        .on_click(on_click)
        .padding(14.0)
        .gap(spacing.lg)
        .child(
            div()
                .size(px(36.0))
                .rounded(px(radii.lg))
                .bg(tint(colors.accent, 0.18))
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.on_neutral_accent)
                .child(Icon::new(icon).size(IconSize::Md)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(spacing.xs))
                .child(
                    div()
                        .text_color(colors.on_neutral)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(typography.body.size))
                        .child(title),
                )
                .child(
                    div()
                        .text_color(colors.on_subtle)
                        .text_size(px(typography.caption.size))
                        .child(sub),
                ),
        )
}

fn storage_card(
    model: &AppModel,
    entity: Entity<AppModel>,
    cx: &Context<AppModel>,
) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let radii = cx.theme().radii;

    let mut card = Card::new()
        .padding(spacing.xl)
        .gap(spacing.md)
        .child(SectionHeader::new("Storage"));

    if let Some(status) = model.status.storage_status.as_ref()
        && !matches!(status.state, FeatureState::Available)
    {
        if let Some(banner) = permission_banner(
            status,
            "Enable storage access on the phone.",
            onboarding_action(entity),
            cx,
        ) {
            card = card.child(banner);
        }
        return card;
    }

    let Some(storage) = model.status.storage.as_ref() else {
        return card
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(px(spacing.sm))
                    .child(Skeleton::new("storage-used-skel").w(96.0).h(34.0))
                    .child(Skeleton::new("storage-total-skel").w(140.0).h(12.0)),
            )
            .child(
                Skeleton::new("storage-bar-skel")
                    .h(10.0)
                    .rounded(radii.pill),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(spacing.md))
                    .child(Skeleton::new("storage-legend-0").w(220.0).h(14.0))
                    .child(Skeleton::new("storage-legend-1").w(220.0).h(14.0))
                    .child(Skeleton::new("storage-legend-2").w(220.0).h(14.0))
                    .child(Skeleton::new("storage-legend-3").w(220.0).h(14.0)),
            );
    };

    card = card
        .child(
            div()
                .flex()
                .flex_row()
                .items_end()
                .gap(px(spacing.sm))
                .child(
                    div()
                        .text_color(colors.on_neutral)
                        .text_size(px(typography.display.size))
                        .line_height(px(typography.display.line_height))
                        .font_weight(FontWeight(typography.display.weight as f32))
                        .tabular_nums()
                        .child(storage_gb_value(storage.used)),
                )
                .child(
                    div()
                        .pb(px(5.0))
                        .text_color(colors.on_subtle)
                        .text_size(px(typography.caption.size))
                        .tabular_nums()
                        .child(format!("GB used of {}", storage_gb_label(storage.total))),
                ),
        )
        .child(storage_segmented_bar(storage, cx))
        .child(storage_legend(storage, cx));

    card
}

fn storage_segmented_bar(storage: &StorageBreakdown, cx: &Context<AppModel>) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let radii = cx.theme().radii;
    let total = storage.total.max(1);
    let categories = storage_categories(storage, &colors);

    let mut bar = div()
        .h(px(10.0))
        .w_full()
        .flex()
        .flex_row()
        .overflow_hidden()
        .rounded(px(radii.pill))
        .border_1()
        .border_color(colors.stroke_neutral_subtle)
        .bg(colors.surface_dim);

    for category in categories {
        let share = (category.bytes as f32 / total as f32).max(0.0);
        if share <= 0.0 {
            continue;
        }
        bar = bar.child(
            div()
                .h_full()
                .flex_basis(relative(share))
                .bg(category.background),
        );
    }

    bar
}

fn storage_legend(storage: &StorageBreakdown, cx: &Context<AppModel>) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let spacing = cx.theme().spacing;
    let radii = cx.theme().radii;
    let typography = cx.theme().typography;
    let total = storage.total.max(1);
    let mut legend = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(spacing.md))
        .pt(px(spacing.xs));

    for category in storage_categories(storage, &colors) {
        legend = legend.child(
            div()
                .w(px(260.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(spacing.sm))
                .text_size(px(typography.caption.size))
                .child(
                    div()
                        .size(px(10.0))
                        .rounded(px(radii.sm))
                        .bg(category.background),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(colors.on_subtle)
                        .child(category.label),
                )
                .child(
                    div()
                        .w(px(44.0))
                        .text_color(colors.on_neutral)
                        .text_align(gpui::TextAlign::Right)
                        .tabular_nums()
                        .child(storage_percent(category.bytes, total)),
                ),
        );
    }

    legend
}

#[derive(Clone, Copy)]
struct StorageCategory {
    label: &'static str,
    bytes: u64,
    background: gpui::Background,
}

fn storage_categories(
    storage: &StorageBreakdown,
    colors: &fluent_core::ColorScheme,
) -> [StorageCategory; 5] {
    [
        StorageCategory {
            label: "Photos & Video",
            bytes: storage.photos.saturating_add(storage.videos),
            background: gradient_from_hue(220.0),
        },
        StorageCategory {
            label: "Apps",
            bytes: storage.apps,
            background: gradient_from_hue(280.0),
        },
        StorageCategory {
            label: "Music",
            bytes: storage.music,
            background: gradient_from_hue(145.0),
        },
        StorageCategory {
            label: "System",
            bytes: storage.system.saturating_add(storage.other),
            background: colors.stroke_neutral.into(),
        },
        StorageCategory {
            label: "Free",
            bytes: storage.free,
            background: colors.stroke_neutral_subtle.into(),
        },
    ]
}

fn storage_percent(bytes: u64, total: u64) -> String {
    let pct = bytes as f64 / total.max(1) as f64 * 100.0;
    format!("{pct:.0}%")
}

fn storage_gb_value(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_000_000_000.0)
}

fn storage_gb_label(bytes: u64) -> String {
    let value = bytes as f64 / 1_000_000_000.0;
    if value >= 10.0 {
        format!("{value:.0} GB")
    } else {
        format!("{value:.1} GB")
    }
}

fn phone_illustration(cx: &Context<AppModel>) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let spacing = cx.theme().spacing;
    let radii = cx.theme().radii;
    let typography = cx.theme().typography;
    // The phone mock is an illustration; its gradients and highlights are
    // intentionally literal so they do not shift with neutral app surfaces.
    let phone_bg = linear_gradient(
        145.0,
        linear_color_stop(hsla(250.0 / 360.0, 0.38, 0.30, 1.0), 0.0),
        linear_color_stop(hsla(250.0 / 360.0, 0.34, 0.16, 1.0), 1.0),
    );
    let screen_bg = linear_gradient(
        145.0,
        linear_color_stop(hsla(210.0 / 360.0, 0.48, 0.38, 1.0), 0.0),
        linear_color_stop(hsla(255.0 / 360.0, 0.42, 0.22, 1.0), 1.0),
    );

    let mut tiles = div().flex().flex_row().flex_wrap().gap(px(spacing.sm));
    for hue in [200.0, 50.0, 130.0, 280.0, 25.0, 350.0] {
        tiles = tiles.child(
            div()
                .size(px(21.0))
                .rounded(px(radii.md))
                .bg(gradient_from_hue(hue)),
        );
    }

    div()
        .w(px(FAKE_PHONE_WIDTH))
        .h(px(FAKE_PHONE_HEIGHT))
        .flex_none()
        .rounded(px(2.0 * radii.lg))
        .p(px(spacing.md))
        .bg(phone_bg)
        .border_1()
        .border_color(colors.stroke_neutral_subtle)
        .child(
            div()
                .size_full()
                .rounded(px(radii.lg))
                .p(px(spacing.md))
                .flex()
                .flex_col()
                .gap(px(spacing.md))
                .bg(screen_bg)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(typography.caption.size))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(gpui::white())
                                .child("9:47"),
                        )
                        .child(
                            div()
                                .w(px(14.0))
                                .h(px(7.0))
                                .rounded(px(radii.sm))
                                .border_1()
                                .border_color(hsla(0.0, 0.0, 1.0, 0.7))
                                .child(
                                    div()
                                        .w(px(9.0))
                                        .h_full()
                                        .rounded(px(1.0))
                                        .bg(hsla(0.0, 0.0, 1.0, 0.72)),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(spacing.sm))
                        .child(
                            div()
                                .w(px(44.0))
                                .h(px(6.0))
                                .rounded(px(radii.pill))
                                .bg(hsla(0.0, 0.0, 1.0, 0.55)),
                        )
                        .child(
                            div()
                                .w(px(30.0))
                                .h(px(6.0))
                                .rounded(px(radii.pill))
                                .bg(hsla(0.0, 0.0, 1.0, 0.34)),
                        ),
                )
                .child(tiles),
        )
}

fn device_summary_card(
    device_name: &str,
    state: ConnectionBadgeState,
    battery_pct: Option<u8>,
    wifi: Option<&str>,
    bluetooth: Option<bool>,
    cx: &Context<AppModel>,
) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let battery_label = battery_pct
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "—".to_owned());
    let battery_bar_color = match battery_pct {
        Some(p) if p > 20 => colors.status_success,
        Some(_) => colors.status_warning,
        None => colors.stroke_neutral_subtle,
    };
    let battery_width = battery_pct.unwrap_or(0).min(100) as f32 / 100.0 * 320.0;

    let mut conn_row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing.md))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(typography.subtitle.size))
                .child(device_name.to_owned()),
        );
    conn_row = conn_row.child(ConnectionBadge::new("hero-conn", state));

    let stats_row = div()
        .flex()
        .flex_row()
        .gap(px(spacing.xxl))
        .child(stat_block("Wi-Fi", wifi.unwrap_or("—"), cx))
        .child(stat_block(
            "Bluetooth",
            match bluetooth {
                Some(true) => "On",
                Some(false) => "Off",
                None => "—",
            },
            cx,
        ))
        .child(stat_block("Charging", "—", cx));

    Card::new().padding(20.0).child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(20.0))
            .child(phone_illustration(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(spacing.xl))
                    .child(conn_row)
                    .child(
                        // Battery block
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(spacing.md))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(6.0))
                                            .text_color(colors.on_subtle)
                                            .child(Icon::new("battery").size(IconSize::Sm))
                                            .child(Label::eyebrow("Battery")),
                                    )
                                    .child(
                                        div()
                                            .text_color(colors.on_neutral)
                                            .text_size(px(typography.display.size))
                                            .font_weight(FontWeight::BOLD)
                                            .tabular_nums()
                                            .child(battery_label),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(spacing.md))
                                    .rounded(px(6.0))
                                    .bg(colors.surface_dim)
                                    .border_1()
                                    .border_color(colors.stroke_neutral_subtle)
                                    .child(
                                        div()
                                            .w(px(battery_width))
                                            .h_full()
                                            .rounded(px(6.0))
                                            .bg(battery_bar_color),
                                    ),
                            ),
                    )
                    .child(stats_row),
            ),
    )
}

fn stat_block(label: &str, value: &str, cx: &Context<AppModel>) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    div()
        .flex()
        .flex_col()
        .gap(px(spacing.xs))
        .child(Label::eyebrow(label.to_owned()))
        .child(
            div()
                .text_color(colors.on_neutral)
                .text_size(px(typography.body.size))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value.to_owned()),
        )
}

fn media_toggle_action(
    media: &crate::status::MediaInfo,
) -> Option<(MediaControlAction, &'static str)> {
    let supports = |action| media.supported_actions.contains(&action);
    let is_playing = matches!(
        media.playback_state,
        MediaPlaybackState::Playing | MediaPlaybackState::Buffering
    );

    if is_playing {
        if supports(MediaControlAction::Pause) {
            Some((MediaControlAction::Pause, "Pause"))
        } else if supports(MediaControlAction::PlayPause) {
            Some((MediaControlAction::PlayPause, "Pause"))
        } else {
            None
        }
    } else if supports(MediaControlAction::Play) {
        Some((MediaControlAction::Play, "Play"))
    } else if supports(MediaControlAction::PlayPause) {
        Some((MediaControlAction::PlayPause, "Play"))
    } else {
        None
    }
}

fn now_playing_card(
    media: &crate::status::MediaInfo,
    entity: Entity<AppModel>,
    cx: &Context<AppModel>,
) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let radii = cx.theme().radii;
    let title = media.title.clone().unwrap_or_else(|| "Unknown".to_owned());
    let artist = media.artist.clone().unwrap_or_default();
    let app_name = media.app_name.clone().unwrap_or_default();
    let control = media_toggle_action(media);

    Card::new().padding(16.0).gap(12.0).child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing.lg))
            .child(
                div()
                    .size(px(80.0))
                    .rounded(px(radii.lg))
                    .bg(colors.surface_dim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(colors.on_neutral_accent)
                    .child(Icon::new("play").size(IconSize::Lg)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(spacing.xs))
                    .child(Label::eyebrow(format!("Now playing · {app_name}")))
                    .child(
                        div()
                            .text_color(colors.on_neutral)
                            .text_size(px(typography.subtitle.size))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_color(colors.on_subtle)
                            .text_size(px(typography.body.size))
                            .child(artist),
                    ),
            )
            .child(if let Some((action, label)) = control {
                Button::new("now-play")
                    .label(label)
                    .appearance(ButtonAppearance::Accent)
                    .size(ButtonSize::Compact)
                    .on_click(move |_, _, app| {
                        entity.update(app, |m, cx| {
                            m.send_utility(Payload::MediaControl(MediaControl { action }));
                            m.activity.push(
                                ActivityIcon::Send,
                                format!("Sent media {} command", label.to_ascii_lowercase()),
                            );
                            cx.notify();
                        });
                    })
                    .into_any_element()
            } else {
                div().into_any_element()
            }),
    )
}

fn recent_notifications(
    model: &AppModel,
    limit: usize,
    cx: &Context<AppModel>,
) -> Vec<gpui::AnyElement> {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let mut out = Vec::new();
    let items: Vec<_> = model
        .notifications
        .iter_items()
        .take(limit)
        .map(|(_, n)| n.clone())
        .collect();
    if items.is_empty() {
        if model.connected() && model.notifications.is_initial_loading() {
            for idx in 0..limit {
                out.push(
                    SkeletonRow::new(format!("overview-notif-loading-{idx}"))
                        .avatar_size(24.0)
                        .into_any_element(),
                );
            }
            return out;
        }
        out.push(
            div()
                .text_color(colors.on_subtle_disabled)
                .italic()
                .py(px(6.0))
                .child("No recent notifications.")
                .into_any_element(),
        );
        return out;
    }
    for n in items {
        let title = n.title.clone().unwrap_or_default();
        let body = n.text.clone().unwrap_or_default();
        let when = relative_timestamp(n.timestamp_unix_ms);
        out.push(
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(10.0))
                .py(px(6.0))
                .child(
                    AppDot::new(n.app_name.clone())
                        .hue(brand_hue(&n.app_package, &n.app_name))
                        .size(24.0),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_size(px(typography.caption.size))
                                .text_color(colors.on_subtle)
                                .child(n.app_name.clone()),
                        )
                        .child(
                            div()
                                .text_color(colors.on_neutral)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(typography.body.size))
                                .child(truncate(&title, 48)),
                        )
                        .child(
                            div()
                                .text_color(colors.on_subtle)
                                .text_size(px(typography.caption.size))
                                .child(truncate(&body, 60)),
                        ),
                )
                .child(
                    div()
                        .text_color(colors.on_subtle_disabled)
                        .text_size(px(typography.caption.size))
                        .child(when),
                )
                .into_any_element(),
        );
    }
    out
}

fn recent_messages(
    model: &AppModel,
    limit: usize,
    cx: &Context<AppModel>,
) -> Vec<gpui::AnyElement> {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let mut out = Vec::new();
    let items: Vec<_> = model
        .messages_state
        .threads
        .iter()
        .take(limit)
        .cloned()
        .collect();
    if items.is_empty() {
        if model.connected() && model.messages_state.thread_status.is_none() {
            for idx in 0..limit {
                out.push(
                    SkeletonRow::new(format!("overview-msg-loading-{idx}"))
                        .avatar_size(28.0)
                        .into_any_element(),
                );
            }
            return out;
        }
        out.push(
            div()
                .text_color(colors.on_subtle_disabled)
                .italic()
                .py(px(6.0))
                .child("No recent messages.")
                .into_any_element(),
        );
        return out;
    }
    for t in items {
        let snippet = t
            .last_message
            .clone()
            .unwrap_or_else(|| "(no preview)".to_owned());
        let when = t
            .timestamp_unix_ms
            .map(relative_timestamp)
            .unwrap_or_default();
        out.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .py(px(6.0))
                .child(Avatar::initials(device_initials(&t.display_name)).size(28.0))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_color(colors.on_neutral)
                                .font_weight(if t.unread_count > 0 {
                                    FontWeight::BOLD
                                } else {
                                    FontWeight::SEMIBOLD
                                })
                                .text_size(px(typography.body.size))
                                .child(truncate(&t.display_name, 24)),
                        )
                        .child(
                            div()
                                .text_color(colors.on_subtle)
                                .text_size(px(typography.caption.size))
                                .child(truncate(&snippet, 40)),
                        ),
                )
                .child(
                    div()
                        .text_color(colors.on_subtle_disabled)
                        .text_size(px(typography.caption.size))
                        .child(when),
                )
                .into_any_element(),
        );
    }
    out
}

fn activity_rows(model: &AppModel, limit: usize, cx: &Context<AppModel>) -> Vec<gpui::AnyElement> {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let radii = cx.theme().radii;
    let mut out = Vec::new();
    if model.activity.is_empty() {
        out.push(
            div()
                .text_color(colors.on_subtle_disabled)
                .italic()
                .py(px(6.0))
                .child("Nothing yet — your recent file transfers, replies, and pairings will show up here.")
                .into_any_element(),
        );
        return out;
    }
    for (idx, ev) in model.activity.iter().take(limit).enumerate() {
        let activity_hover = colors.neutral_hover;
        let when = ev
            .timestamp
            .duration_since(UNIX_EPOCH)
            .map(|d| relative_timestamp(d.as_millis() as u64))
            .unwrap_or_default();
        out.push(
            div()
                .id(SharedString::from(format!("activity-row-{idx}")))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .px(px(spacing.sm))
                .py(px(6.0))
                .rounded(px(radii.md))
                .hover(move |s| s.bg(activity_hover))
                .child(
                    div()
                        .size(px(28.0))
                        .rounded(px(radii.pill))
                        .bg(colors.surface_dim)
                        .border_1()
                        .border_color(colors.stroke_neutral_subtle)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(colors.on_neutral_accent)
                        .child(Icon::new(ev.icon.icon_name()).size(IconSize::Sm)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(colors.on_neutral)
                        .text_size(px(typography.body.size))
                        .child(ev.text.clone()),
                )
                .child(
                    div()
                        .text_color(colors.on_subtle_disabled)
                        .text_size(px(typography.caption.size))
                        .child(when),
                )
                .into_any_element(),
        );
    }
    out
}

fn numbered_step(n: usize, title: &str, body: &str, cx: &Context<AppModel>) -> impl IntoElement {
    let colors = cx.theme().colors.clone();
    let typography = cx.theme().typography;
    let spacing = cx.theme().spacing;
    let radii = cx.theme().radii;
    div()
        .flex()
        .flex_row()
        .gap(px(14.0))
        .items_start()
        .child(
            div()
                .size(px(26.0))
                .rounded(px(radii.pill))
                .bg(tint(colors.accent, 0.18))
                .border_1()
                .border_color(tint(colors.accent, 0.4))
                .text_color(colors.on_neutral_accent)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(typography.caption.size))
                .font_weight(FontWeight::BOLD)
                .child(format!("{n}")),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(spacing.xs))
                .child(
                    div()
                        .text_color(colors.on_neutral)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(typography.body.size))
                        .child(title.to_owned()),
                )
                .child(
                    div()
                        .text_color(colors.on_subtle)
                        .text_size(px(typography.caption.size))
                        .child(body.to_owned()),
                ),
        )
}

// ── Small data helpers ──────────────────────────────────────────────────────

fn device_initials(name: &str) -> String {
    let mut initials = String::new();
    for w in name.split_whitespace().take(2) {
        if let Some(c) = w.chars().next() {
            initials.push(c.to_ascii_uppercase());
        }
    }
    if initials.is_empty() {
        "•".to_owned()
    } else {
        initials
    }
}

fn parse_battery_percent(s: Option<&str>) -> Option<u8> {
    let s = s?;
    let n: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    n.parse().ok()
}

fn relative_timestamp(ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(ms);
    let diff_secs = now.saturating_sub(ms) / 1000;
    if diff_secs < 60 {
        "just now".to_owned()
    } else if diff_secs < 3600 {
        format!("{}m ago", diff_secs / 60)
    } else if diff_secs < 86400 {
        format!("{}h ago", diff_secs / 3600)
    } else {
        format!("{}d ago", diff_secs / 86400)
    }
}

fn format_full_time(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86400) as i64;
    let minutes = (secs % 86400) / 60;
    let hour = minutes / 60;
    let minute = minutes % 60;
    let now_days = (current_time_ms() / 1000 / 86400) as i64;
    let time = format!("{hour:02}:{minute:02}");
    match now_days.saturating_sub(days) {
        0 => format!("Today {time}"),
        1 => format!("Yesterday {time}"),
        _ => {
            let (y, m, d) = civil_from_days(days);
            let weekday = weekday_name(y, m, d);
            let short_weekday = weekday.get(..3).unwrap_or(weekday);
            format!("{short_weekday} {d} {}", month_name(m))
        }
    }
}

fn local_greeting() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Local hour without tz lookup — assume system clock is local enough.
    let hour = ((secs % 86400) / 3600) as u8;
    let phase = if hour < 5 {
        "Good evening"
    } else if hour < 12 {
        "Good morning"
    } else if hour < 18 {
        "Good afternoon"
    } else {
        "Good evening"
    };
    phase.to_owned()
}

fn today_string() -> String {
    // Lightweight ISO-style date without `chrono`. Computes Y-M-D from epoch
    // seconds; weekday derived via Zeller's congruence.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let (y, m, d) = civil_from_days(days);
    let day_name = weekday_name(y, m, d);
    let month_name = month_name(m);
    format!("{day_name}, {month_name} {d}")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 {
        z / 146097
    } else {
        (z - 146096) / 146097
    };
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64 + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn weekday_name(y: i32, m: u32, d: u32) -> &'static str {
    let (y, m) = if m < 3 { (y - 1, m + 12) } else { (y, m) };
    let k = y % 100;
    let j = y / 100;
    let h = (d as i32 + 13 * (m as i32 + 1) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    match h {
        0 => "Saturday",
        1 => "Sunday",
        2 => "Monday",
        3 => "Tuesday",
        4 => "Wednesday",
        5 => "Thursday",
        _ => "Friday",
    }
}

fn month_name(m: u32) -> &'static str {
    match m {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_bind_advertises_discovered_addresses() {
        let addresses = advertised_pair_addresses_from(
            "0.0.0.0:48172",
            None,
            vec![
                "192.168.1.10".parse().unwrap(),
                "127.0.0.1".parse().unwrap(),
            ],
            None,
        );
        assert_eq!(addresses, vec!["192.168.1.10:48172".to_owned()]);
    }

    #[test]
    fn explicit_bind_is_advertised_verbatim() {
        let addresses = advertised_pair_addresses_from(
            "10.0.0.5:48172",
            None,
            vec!["192.168.1.10".parse().unwrap()],
            None,
        );
        assert_eq!(addresses, vec!["10.0.0.5:48172".to_owned()]);
    }

    #[test]
    fn configured_advertise_addresses_take_part_in_qr_payload() {
        let addresses = advertised_pair_addresses_from(
            "0.0.0.0:48172",
            Some("phone-visible.local:48172, 192.168.1.20:48172".to_owned()),
            Vec::new(),
            None,
        );
        assert_eq!(
            addresses,
            vec![
                "192.168.1.20:48172".to_owned(),
                "phone-visible.local:48172".to_owned()
            ]
        );
    }

    #[test]
    fn wildcard_bind_falls_back_to_hostname_without_discovery() {
        let addresses = advertised_pair_addresses_from(
            "0.0.0.0:48172",
            None,
            Vec::new(),
            Some("desktop-host".to_owned()),
        );
        assert_eq!(addresses, vec!["desktop-host:48172".to_owned()]);
    }

    #[test]
    fn parses_battery_percent_from_status_string() {
        assert_eq!(parse_battery_percent(Some("78%")), Some(78));
        assert_eq!(parse_battery_percent(Some("12% charging")), Some(12));
        assert_eq!(parse_battery_percent(None), None);
        assert_eq!(parse_battery_percent(Some("—")), None);
    }

    #[test]
    fn device_initials_takes_up_to_two_word_first_letters() {
        assert_eq!(device_initials("Pixel 8 Pro"), "P8");
        assert_eq!(device_initials("nexus"), "N");
        assert_eq!(device_initials("   "), "•");
    }
}
