use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use fluent_core::ThemeProvider as _;
use fluent_primitives::{Button, ButtonAppearance, Divider, Label, LabelSize, Switch, TextInput};
use gpui::{
    App, Bounds, ClickEvent, Context, Entity, FontWeight, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent,
    SharedString, VideoTextureId, Window, canvas, div, prelude::*, px,
};
use qrcode::{Color as QrColor, QrCode};

use androidconnect_protocol::{
    AudioControl, AudioControlCommand, DndMode, InputEvent, MediaControl, MediaControlAction,
    MediaPlaybackState, MessageDirection, Payload, PointerButton, PointerEvent, PointerPhase,
    qr::{QrPairingPayload, encode_qr_payload},
};

use crate::network;
use crate::panels::{
    MessagesState, Panel, files::FilesState, notifications::NotificationsState, phone::PhoneState,
};
use crate::status::{ConnectionState, DesktopStatus, format_pairing_code};
use crate::streaming::{RgbaFrame, map_window_to_frame};

pub struct AppModel {
    command_tx: mpsc::SyncSender<network::DesktopCommand>,
    pub status: DesktopStatus,
    pair_addresses: Vec<String>,
    desktop_id: String,
    desktop_name: String,
    active_panel: Panel,

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

    // Mirror input forwarding
    mirror_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    left_pressed: bool,

    // Panel states
    notifications: NotificationsState,
    phone_state: PhoneState,
    files_state: FilesState,
    messages_state: MessagesState,

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
        files_state.current_path = "/".to_owned();
        let sms_composer = cx.new(|_| TextInput::new().placeholder("Type a message…"));
        let quick_reply_input = cx.new(|_| TextInput::new().placeholder("Quick reply…"));
        let file_action_input = cx.new(|_| TextInput::new());
        Self {
            command_tx,
            status,
            pair_addresses,
            desktop_id,
            desktop_name,
            active_panel: Panel::Mirror,
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
            mirror_bounds: Arc::new(Mutex::new(None)),
            left_pressed: false,
            notifications: NotificationsState::default(),
            phone_state: PhoneState::default(),
            files_state,
            messages_state: MessagesState::default(),
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
        self.status.apply(status);
        if self.status.bind != prev_bind {
            self.pair_addresses = advertised_pair_addresses(&self.status.bind);
            self.qr_cache_key.clear();
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
                self.messages_state.apply_send_response(resp);
            }
            network::DesktopEvent::SessionLost => {
                self.notifications.clear();
                self.phone_state = PhoneState::default();
                self.files_state = FilesState::default();
                self.files_state.current_path = "/".to_owned();
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

    fn refresh_pair_qr(&mut self, window: &mut Window) {
        if self.pair_addresses.is_empty() {
            return;
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        let expires_ms = now_ms.saturating_add(5 * 60 * 1000);
        let payload = QrPairingPayload::new(
            self.pair_addresses.clone(),
            self.status.pairing_code.clone(),
            self.desktop_id.clone(),
            self.desktop_name.clone(),
            now_ms,
            expires_ms,
        );
        let cache_key = format!(
            "v={}|addrs={:?}|tok={}|id={}",
            payload.protocol_version, payload.addresses, payload.pairing_token, payload.desktop_id,
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
    }

    fn connected(&self) -> bool {
        matches!(self.status.connection, ConnectionState::Connected)
            && self.status.input_authenticated
    }

    // ── Panel renderers ────────────────────────────────────────────────────

    fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let subject = self
            .status
            .device_name
            .clone()
            .or_else(|| self.status.peer.clone())
            .unwrap_or_else(|| "no device".to_owned());
        let (state_color, state_label) = match self.status.connection {
            ConnectionState::Listening => (gpui::rgb(0x888888u32), "listening"),
            ConnectionState::Connected if self.status.input_authenticated => {
                (gpui::rgb(0x50c878u32), "paired")
            }
            ConnectionState::Connected => (gpui::rgb(0xffdd57u32), "pairing"),
            ConnectionState::Error => (gpui::rgb(0xff4444u32), "error"),
        };
        div()
            .h(px(48.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .bg(colors.neutral)
            .child(
                div()
                    .text_color(colors.on_neutral)
                    .text_size(px(16.0))
                    .child("AndroidConnect"),
            )
            .child(
                div()
                    .w(px(1.0))
                    .h(px(24.0))
                    .bg(colors.stroke_neutral_subtle),
            )
            .child(div().text_color(colors.on_subtle).child(subject))
            .child(div().flex_1())
            .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(state_color))
            .child(
                div()
                    .text_color(colors.on_subtle)
                    .text_size(px(12.0))
                    .child(state_label),
            )
    }

    fn render_nav_rail(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let entity = cx.entity();
        let mut rail = div()
            .w(px(160.0))
            .h_full()
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(2.0))
            .p(px(8.0));
        for panel in Panel::ALL {
            let is_active = panel == self.active_panel;
            let entity2 = entity.clone();
            rail = rail.child(
                div()
                    .id(("nav-", panel as usize))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(4.0))
                    .bg(if is_active {
                        colors.subtle_selected
                    } else {
                        colors.surface
                    })
                    .text_color(if is_active {
                        colors.accent
                    } else {
                        colors.on_neutral
                    })
                    .cursor_pointer()
                    .on_click(move |_: &ClickEvent, _win, app: &mut App| {
                        entity2.update(app, |m, cx| {
                            m.active_panel = panel;
                            cx.notify();
                        });
                    })
                    .child(panel.label()),
            );
        }
        rail
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
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
            None | Some(DndMode::Off) => "—",
            Some(DndMode::Priority) => "priority",
            Some(DndMode::Alarms) => "alarms",
            Some(DndMode::TotalSilence) => "silence",
        };
        let vol = self
            .status
            .volume_percent
            .map(|p| format!("{p}%"))
            .unwrap_or_else(|| "—".to_owned());
        let heartbeat = self
            .status
            .last_pong_nonce
            .map(|n| format!("#{n}"))
            .unwrap_or_else(|| "—".to_owned());

        div()
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .bg(colors.surface_dim)
            .text_color(colors.on_subtle)
            .text_size(px(11.0))
            .child(format!("🔋 {battery}"))
            .child(format!("📶 {wifi}"))
            .child(format!("BT {bt}"))
            .child(format!("DND {dnd}"))
            .child(format!("🔊 {vol}"))
            .child(format!("♥ {heartbeat}"))
    }

    fn render_mirror_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let video_id = self.video_texture;
        let frame_data = self.frame_data.clone();
        let mirror_bounds_shared = Arc::clone(&self.mirror_bounds);
        let mirror_bounds_shared2 = Arc::clone(&self.mirror_bounds);
        let mirror_bounds_canvas = Arc::clone(&self.mirror_bounds);
        let frame_w = self.frame_w;
        let frame_h = self.frame_h;

        if video_id.is_none() || frame_data.is_none() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .bg(gpui::black())
                .child(
                    div()
                        .text_color(colors.on_subtle)
                        .text_size(px(20.0))
                        .child("No video yet"),
                )
                .child(
                    div()
                        .text_color(colors.on_subtle_disabled)
                        .text_size(px(13.0))
                        .child("Mirror starts as soon as your phone shares its screen."),
                )
                .into_any_element();
        }

        div()
            .size_full()
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
                move |this, event: &MouseMoveEvent, _, _| {
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
                        (frame_data, video_id)
                    },
                    |bounds, state, window, _cx| {
                        let (Some(data), Some(id)) = state else {
                            return;
                        };
                        let _ = window.paint_video_frame(id, bounds, &data);
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    fn render_pair_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let pairing_code_fmt = format_pairing_code(&self.status.pairing_code);
        let addresses = self.pair_addresses.clone();
        let bind = self.status.bind.clone();
        let connection = self.status.connection;
        let last_error = self.status.last_error.clone();
        let qr_data = self.qr_data.clone();
        let qr_id = self.qr_texture;
        let qr_size_px = px(self.qr_size.max(256) as f32);

        div()
            .id("pair-scroll")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(16.0))
            .p(px(40.0))
            .bg(colors.surface)
            .child(
                div()
                    .text_color(colors.on_neutral)
                    .text_size(px(28.0))
                    .child("Pair a device"),
            )
            .child(
                div()
                    .text_color(colors.on_subtle)
                    .text_size(px(14.0))
                    .child("Open AndroidConnect on your phone and scan the QR, or enter the code manually."),
            )
            .child(
                div()
                    .w(qr_size_px)
                    .h(qr_size_px)
                    .bg(gpui::white())
                    .rounded(px(4.0))
                    .child(if qr_data.is_some() && qr_id.is_some() {
                        canvas(
                            move |_bounds, _window, _cx| (qr_data, qr_id),
                            |bounds, state, window, _cx| {
                                let (Some(data), Some(id)) = state else {
                                    return;
                                };
                                let _ = window.paint_video_frame(id, bounds, &data);
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
                            .text_size(px(13.0))
                            .child("QR unavailable")
                            .into_any_element()
                    }),
            )
            .child(
                div()
                    .text_color(colors.on_neutral)
                    .text_size(px(16.0))
                    .child("Manual pairing"),
            )
            .child(
                div()
                    .text_color(colors.on_subtle)
                    .text_size(px(13.0))
                    .child(if addresses.is_empty() {
                        format!("Listening on: {bind}")
                    } else {
                        format!("Address: {}", addresses.join(", "))
                    }),
            )
            .child(
                div()
                    .text_color(colors.on_neutral)
                    .child(format!("Pairing code: {pairing_code_fmt}")),
            )
            .child(if !matches!(connection, ConnectionState::Listening) {
                div()
                    .text_color(colors.on_subtle_disabled)
                    .text_size(px(13.0))
                    .italic()
                    .child("Waiting for handshake…")
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .child(if let Some(err) = last_error {
                div()
                    .text_color(gpui::rgb(0xff4444u32))
                    .text_size(px(13.0))
                    .child(err)
                    .into_any_element()
            } else {
                div().into_any_element()
            })
    }

    fn render_phone_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let entity = cx.entity();

        // If DeviceStatus mirroring is unavailable, surface a single full-panel placeholder
        // instead of the quick-settings UI.
        let device_status_feature = self
            .status
            .feature(androidconnect_protocol::UtilityFeature::DeviceStatus)
            .cloned();
        if let Some(status) = device_status_feature
            && let Some(placeholder) = feature_placeholder(
                cx,
                &status,
                "Device status mirroring is unavailable.",
                "phone-feature",
            )
        {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .p(px(16.0))
                .bg(colors.surface)
                .child(Label::new("Phone").size(LabelSize::Subtitle))
                .child(Divider::horizontal())
                .child(placeholder);
        }

        // Reconcile pending values
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

        // Volume row
        let entity_vol_down = entity.clone();
        let entity_vol_up = entity.clone();
        let entity_dnd = [
            entity.clone(),
            entity.clone(),
            entity.clone(),
            entity.clone(),
        ];
        let entity_bt = entity.clone();
        let entity_media_prev = entity.clone();
        let entity_media_play = entity.clone();
        let entity_media_next = entity.clone();

        div()
            .size_full()
            .p(px(20.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .bg(colors.surface)
            .child(if let Some(info) = media_info {
                let has_prev = info
                    .supported_actions
                    .contains(&MediaControlAction::Previous);
                let has_next = info.supported_actions.contains(&MediaControlAction::Next);
                let has_play_pause = info
                    .supported_actions
                    .contains(&MediaControlAction::PlayPause);
                let has_play = info.supported_actions.contains(&MediaControlAction::Play);
                let has_pause = info.supported_actions.contains(&MediaControlAction::Pause);

                let play_action = if has_play_pause {
                    Some(MediaControlAction::PlayPause)
                } else if matches!(
                    info.playback_state,
                    MediaPlaybackState::Playing | MediaPlaybackState::Buffering
                ) && has_pause
                {
                    Some(MediaControlAction::Pause)
                } else if has_play {
                    Some(MediaControlAction::Play)
                } else {
                    None
                };

                let play_label = if matches!(
                    info.playback_state,
                    MediaPlaybackState::Playing | MediaPlaybackState::Buffering
                ) {
                    "⏸"
                } else {
                    "▶"
                };

                let title_str = info.title.clone().unwrap_or_else(|| "Unknown".to_owned());
                let artist_str = info.artist.clone().unwrap_or_default();
                let app_str = info.app_name.clone().unwrap_or_default();

                let mut controls = div().flex().flex_row().gap(px(4.0));
                if has_prev {
                    let ep = entity_media_prev.clone();
                    controls = controls.child(Button::new("media-prev").label("⏮").on_click(
                        move |_: &ClickEvent, _, app: &mut App| {
                            ep.update(app, |m, cx| {
                                m.send_utility(Payload::MediaControl(MediaControl {
                                    action: MediaControlAction::Previous,
                                }));
                                cx.notify();
                            });
                        },
                    ));
                }
                if let Some(action) = play_action {
                    let ep = entity_media_play.clone();
                    controls =
                        controls.child(Button::new("media-play").label(play_label).on_click(
                            move |_: &ClickEvent, _, app: &mut App| {
                                ep.update(app, |m, cx| {
                                    m.send_utility(Payload::MediaControl(MediaControl { action }));
                                    cx.notify();
                                });
                            },
                        ));
                }
                if has_next {
                    let en = entity_media_next.clone();
                    controls = controls.child(Button::new("media-next").label("⏭").on_click(
                        move |_: &ClickEvent, _, app: &mut App| {
                            en.update(app, |m, cx| {
                                m.send_utility(Payload::MediaControl(MediaControl {
                                    action: MediaControlAction::Next,
                                }));
                                cx.notify();
                            });
                        },
                    ));
                }

                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(Label::new("Now Playing").size(LabelSize::Subtitle))
                    .child(Divider::horizontal())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(div().font_weight(FontWeight::BOLD).child(title_str))
                            .child(div().text_color(colors.on_subtle).child(artist_str))
                            .child(
                                div()
                                    .text_color(colors.on_subtle_disabled)
                                    .text_size(px(11.0))
                                    .child(app_str),
                            ),
                    )
                    .child(controls)
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .child(Label::new("Quick settings").size(LabelSize::Subtitle))
            .child(Divider::horizontal())
            // Volume
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().text_color(colors.on_neutral).child("Media volume"))
                    .child(if vol_pending {
                        div()
                            .w(px(12.0))
                            .h(px(12.0))
                            .rounded_full()
                            .bg(colors.accent)
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_color(colors.on_subtle)
                            .child(format!("{volume}%")),
                    )
                    .child(Button::new("vol-down").label("−10").on_click(
                        move |_: &ClickEvent, _, app: &mut App| {
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
                                    command: AudioControlCommand::SetVolume { percent: new_vol },
                                }));
                                cx.notify();
                            });
                        },
                    ))
                    .child(Button::new("vol-up").label("+10").on_click(
                        move |_: &ClickEvent, _, app: &mut App| {
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
                                    command: AudioControlCommand::SetVolume { percent: new_vol },
                                }));
                                cx.notify();
                            });
                        },
                    )),
            )
            // DND
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().text_color(colors.on_neutral).child("Do not disturb"))
                    .child(if dnd_pending {
                        div()
                            .w(px(12.0))
                            .h(px(12.0))
                            .rounded_full()
                            .bg(colors.accent)
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    }),
            )
            .child({
                let mut row = div().flex().flex_row().gap(px(4.0));
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
                    row = row.child(
                        Button::new(("dnd-", i))
                            .label(dnd_label(m))
                            .appearance(if is_sel {
                                ButtonAppearance::Accent
                            } else {
                                ButtonAppearance::default()
                            })
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
                row
            })
            // Bluetooth
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().text_color(colors.on_neutral).child("Bluetooth"))
                    .child(if bt_pending {
                        div()
                            .w(px(12.0))
                            .h(px(12.0))
                            .rounded_full()
                            .bg(colors.accent)
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    })
                    .child(div().flex_1())
                    .child(
                        Switch::new("bt-toggle")
                            .on(bt_enabled.unwrap_or(false))
                            .on_click(move |new_val, _: &ClickEvent, _, app: &mut App| {
                                entity_bt.update(app, |m, cx| {
                                    m.phone_state.pending_bluetooth = Some(new_val);
                                    m.phone_state.pending_bluetooth_since = Some(Instant::now());
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
            .child(if let Some(err) = error_msg {
                div()
                    .text_color(gpui::rgb(0xff4444u32))
                    .text_size(px(13.0))
                    .child(err)
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .child(Divider::horizontal())
            .child(
                div()
                    .text_color(colors.on_subtle_disabled)
                    .text_size(px(11.0))
                    .italic()
                    .child("Quick settings reflect the latest DeviceStatus from the phone."),
            )
    }

    fn render_notifications_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let entity = cx.entity();
        let count = self.notifications.item_count();
        let feature_status = self
            .status
            .feature(androidconnect_protocol::UtilityFeature::Notifications)
            .cloned();

        let mut panel = div()
            .id("notif-scroll")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(12.0))
            .bg(colors.surface)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(Label::new(format!("{count} notifications")).size(LabelSize::Subtitle))
                    .child(div().flex_1())
                    .child({
                        let e = entity.clone();
                        Button::new("hide-sensitive")
                            .label(if self.notifications.hide_sensitive {
                                "Showing filtered"
                            } else {
                                "Hide sensitive"
                            })
                            .appearance(ButtonAppearance::Subtle)
                            .on_click(move |_: &ClickEvent, _, app: &mut App| {
                                e.update(app, |m, cx| {
                                    m.notifications.hide_sensitive =
                                        !m.notifications.hide_sensitive;
                                    cx.notify();
                                });
                            })
                    }),
            )
            .child(Divider::horizontal());

        if let Some(status) = feature_status.as_ref()
            && let Some(placeholder) = feature_placeholder(
                cx,
                status,
                "Enable Notification access on the phone.",
                "notif-feature",
            )
        {
            panel = panel.child(placeholder);
        }

        if count == 0 {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(40.0))
                    .text_color(colors.on_subtle_disabled)
                    .italic()
                    .child("No active notifications"),
            );
        } else {
            for (id, notif) in self.notifications.iter_items() {
                let id = id.clone();
                let notif = notif.clone();
                let suppressed = self.notifications.is_suppressed(&notif.app_package);
                let hide = self.notifications.hide_sensitive && notif.sensitive;
                let e = entity.clone();
                let app_package = notif.app_package.clone();
                let notif_id = notif.notification_id.clone();

                let mut card = div()
                    .p(px(10.0))
                    .rounded(px(4.0))
                    .bg(colors.neutral)
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_color(colors.on_neutral)
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(notif.app_name.clone()),
                            )
                            .child(if suppressed {
                                div()
                                    .text_color(colors.on_subtle_disabled)
                                    .text_size(px(11.0))
                                    .italic()
                                    .child("suppressed")
                                    .into_any_element()
                            } else {
                                div().into_any_element()
                            })
                            .child(div().flex_1())
                            .child(
                                Button::new(SharedString::from(format!("notif-menu-{id}")))
                                    .label("⋮")
                                    .appearance(ButtonAppearance::Subtle)
                                    .on_click(move |_: &ClickEvent, _, app: &mut App| {
                                        let pkg = app_package.clone();
                                        e.update(app, |m, cx| {
                                            let enabled = m.notifications.is_suppressed(&pkg);
                                            m.notifications.toggle_suppress(&pkg);
                                            m.send_utility(Payload::NotificationFilterUpdate(
                                                androidconnect_protocol::NotificationFilterUpdate {
                                                    package_name: pkg.clone(),
                                                    enabled,
                                                },
                                            ));
                                            cx.notify();
                                        });
                                    }),
                            ),
                    );

                if let Some(title) = &notif.title {
                    let display = if hide {
                        "[sensitive content hidden]".to_owned()
                    } else {
                        title.clone()
                    };
                    card = card.child(
                        div()
                            .text_color(colors.on_neutral)
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(13.0))
                            .child(display),
                    );
                }
                if let Some(text) = &notif.text
                    && !hide
                    && !text.is_empty()
                {
                    card = card.child(
                        div()
                            .text_color(colors.on_subtle)
                            .text_size(px(12.0))
                            .child(text.clone()),
                    );
                }

                // Action buttons + inline quick-reply for reply-capable actions.
                if !notif.actions.is_empty() {
                    let mut actions_row = div().flex().flex_row().gap(px(4.0));
                    for action in &notif.actions {
                        let nid = notif_id.clone();
                        let aid = action.action_id.clone();
                        let title = action.title.clone();
                        let allows_reply = action.allows_reply;
                        let e2 = entity.clone();
                        actions_row = actions_row.child(
                            Button::new(SharedString::from(format!("action-{nid}-{aid}")))
                                .label(title.clone())
                                .appearance(ButtonAppearance::Subtle)
                                .on_click(move |_: &ClickEvent, _, app: &mut App| {
                                    let nid2 = nid.clone();
                                    let aid2 = aid.clone();
                                    e2.update(app, |m, cx| {
                                        if allows_reply {
                                            // Toggle the inline reply input for this action.
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
                    card = card.child(actions_row);

                    // Render the inline quick-reply composer if this notification is the active
                    // reply target. We need to remember which action_id to submit on Send.
                    if let Some((open_nid, open_aid)) = &self.quick_reply.open_on
                        && *open_nid == notif.notification_id
                    {
                        let nid_send = notif.notification_id.clone();
                        let aid_send = open_aid.clone();
                        let nid_cancel = notif.notification_id.clone();
                        let aid_cancel = open_aid.clone();
                        let e_send = entity.clone();
                        let e_cancel = entity.clone();
                        card = card.child(
                            div()
                                .mt(px(4.0))
                                .flex()
                                .flex_row()
                                .gap(px(4.0))
                                .items_center()
                                .child(div().flex_1().child(self.quick_reply_input.clone()))
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "qr-send-{nid_send}-{aid_send}"
                                    )))
                                    .label("Send")
                                    .appearance(ButtonAppearance::Accent)
                                    .on_click(
                                        move |_: &ClickEvent, _, app: &mut App| {
                                            let nid = nid_send.clone();
                                            let aid = aid_send.clone();
                                            e_send.update(app, |m, cx| {
                                                let text =
                                                    m.quick_reply_input.read(cx).text().to_string();
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
                                        },
                                    ),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "qr-cancel-{nid_cancel}-{aid_cancel}"
                                    )))
                                    .label("Cancel")
                                    .appearance(ButtonAppearance::Subtle)
                                    .on_click(
                                        move |_: &ClickEvent, _, app: &mut App| {
                                            e_cancel.update(app, |m, cx| {
                                                m.quick_reply.open_on = None;
                                                cx.notify();
                                            });
                                        },
                                    ),
                                ),
                        );
                    }
                }

                panel = panel.child(card);
            }
        }
        panel
    }

    fn render_files_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let entity = cx.entity();

        // Trigger initial browse if needed
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

        let e_refresh = entity.clone();
        let e_back = entity.clone();
        let e_new_folder = entity.clone();
        let path_for_refresh = current_path.clone();

        let mut panel = div()
            .id("files-scroll")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(12.0))
            .bg(colors.surface)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(Label::new("Files").size(LabelSize::Subtitle))
                    .child(div().flex_1())
                    .child(
                        Button::new("files-back")
                            .label("← Back")
                            .appearance(ButtonAppearance::Subtle)
                            .on_click(move |_: &ClickEvent, _, app: &mut App| {
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
                            .label("↻ Refresh")
                            .appearance(ButtonAppearance::Subtle)
                            .on_click(move |_: &ClickEvent, _, app: &mut App| {
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
                            .label("+ Folder")
                            .appearance(ButtonAppearance::Subtle)
                            .on_click(move |_: &ClickEvent, _, app: &mut App| {
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
                    ),
            )
            .child(Divider::horizontal())
            .child(
                div()
                    .text_color(colors.on_subtle)
                    .text_size(px(12.0))
                    .child(format!("Path: {current_path}")),
            );

        // Inline action dialog (create folder / rename)
        if let Some(kind) = self.file_action.kind {
            let title = match kind {
                FileActionKind::CreateFolder => "New folder",
                FileActionKind::Rename => "Rename",
            };
            let target = self.file_action.target_path.clone().unwrap_or_default();
            let e_submit = entity.clone();
            let e_cancel = entity.clone();
            panel = panel.child(
                div()
                    .p(px(8.0))
                    .rounded(px(4.0))
                    .bg(colors.neutral)
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_color(colors.on_neutral)
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(12.0))
                            .child(format!("{title}: {target}")),
                    )
                    .child(self.file_action_input.clone())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(4.0))
                            .child(
                                Button::new("file-action-submit")
                                    .label("Submit")
                                    .appearance(ButtonAppearance::Accent)
                                    .on_click(move |_: &ClickEvent, _, app: &mut App| {
                                        e_submit.update(app, |m, cx| {
                                            let value =
                                                m.file_action_input.read(cx).text().to_string();
                                            let trimmed = value.trim().to_owned();
                                            if trimmed.is_empty() {
                                                return;
                                            }
                                            let Some(kind) = m.file_action.kind else {
                                                return;
                                            };
                                            let Some(target) = m.file_action.target_path.clone()
                                            else {
                                                return;
                                            };
                                            let parent_path = m.files_state.current_path.clone();
                                            let payload = match kind {
                                                FileActionKind::CreateFolder => {
                                                    Payload::FileMutation(
                                                        androidconnect_protocol::FileMutation {
                                                            request_id: format!(
                                                                "mut-{}",
                                                                new_request_token()
                                                            ),
                                                            mutation:
                                                                androidconnect_protocol::FileMutationKind::CreateFolder,
                                                            path: target.clone(),
                                                            new_path: Some(trimmed),
                                                        },
                                                    )
                                                }
                                                FileActionKind::Rename => {
                                                    let new_path = if let Some(slash_idx) =
                                                        target.rfind('/')
                                                    {
                                                        format!(
                                                            "{}/{trimmed}",
                                                            &target[..slash_idx]
                                                        )
                                                    } else {
                                                        trimmed
                                                    };
                                                    Payload::FileMutation(
                                                        androidconnect_protocol::FileMutation {
                                                            request_id: format!(
                                                                "mut-{}",
                                                                new_request_token()
                                                            ),
                                                            mutation:
                                                                androidconnect_protocol::FileMutationKind::Rename,
                                                            path: target.clone(),
                                                            new_path: Some(new_path),
                                                        },
                                                    )
                                                }
                                            };
                                            m.send_utility(payload);
                                            // Re-browse the current path so the user sees the new state.
                                            let refresh =
                                                m.files_state.request_browse(parent_path);
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
                                    .on_click(move |_: &ClickEvent, _, app: &mut App| {
                                        e_cancel.update(app, |m, cx| {
                                            m.file_action = FileActionDialog::default();
                                            cx.notify();
                                        });
                                    }),
                            ),
                    ),
            );
        }

        if pending {
            panel = panel.child(
                div()
                    .text_color(colors.on_subtle_disabled)
                    .italic()
                    .child("Loading…"),
            );
        } else if let Some(resp) = response {
            if let Some(placeholder) = feature_placeholder(
                cx,
                &resp.status,
                "Cannot browse this folder.",
                "files-status",
            ) {
                panel = panel.child(placeholder);
            }
            for entry in &resp.entries {
                let icon = match entry.entry_type {
                    androidconnect_protocol::FileEntryType::Directory => "📁",
                    androidconnect_protocol::FileEntryType::File => "📄",
                    androidconnect_protocol::FileEntryType::Media => "🖼",
                };
                let size_label = entry
                    .size_bytes
                    .map(crate::status::format_bytes_short)
                    .unwrap_or_else(|| "—".to_owned());
                let label_text =
                    format!("{icon}  {:<48} {}", truncate(&entry.name, 46), size_label);
                let entry_path = entry.path.clone();
                let entry_type = entry.entry_type;
                let e_nav = entity.clone();
                let e_del = entity.clone();
                let e_rename = entity.clone();
                let e_download = entity.clone();
                let e_path_del = entry.path.clone();
                let e_path_rename = entry.path.clone();
                let e_name_rename = entry.name.clone();
                let e_path_download = entry.path.clone();

                let mut row = div().flex().flex_row().items_center().gap(px(4.0)).child(
                    div()
                        .id(SharedString::from(format!("fentry-{entry_path}")))
                        .flex_1()
                        .text_color(colors.on_neutral)
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .on_click(move |_: &ClickEvent, _, app: &mut App| {
                            if !matches!(
                                entry_type,
                                androidconnect_protocol::FileEntryType::Directory
                            ) {
                                return;
                            }
                            let p = entry_path.clone();
                            e_nav.update(app, |m, cx| {
                                let payload = m.files_state.navigate_to(p);
                                m.send_utility(payload);
                                cx.notify();
                            });
                        })
                        .child(label_text),
                );

                if !matches!(
                    entry_type,
                    androidconnect_protocol::FileEntryType::Directory
                ) {
                    row = row.child(
                        Button::new(SharedString::from(format!("dl-{}", entry.path)))
                            .label("⬇")
                            .appearance(ButtonAppearance::Subtle)
                            .on_click(move |_: &ClickEvent, _, app: &mut App| {
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
                            .label("✎")
                            .appearance(ButtonAppearance::Subtle)
                            .on_click(move |_: &ClickEvent, _, app: &mut App| {
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
                            .label("✕")
                            .appearance(ButtonAppearance::Subtle)
                            .on_click(move |_: &ClickEvent, _, app: &mut App| {
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

                panel = panel.child(row);
            }
        } else {
            panel = panel.child(
                div()
                    .text_color(colors.on_subtle_disabled)
                    .italic()
                    .child("No data for this path."),
            );
        }
        panel
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
                path,
            },
        ));
    }

    fn render_messages_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors.clone();
        let entity = cx.entity();
        let state = self.messages_state.clone();
        let composer = self.sms_composer.clone();

        // Left column: thread list
        let mut thread_list = div()
            .id("sms-thread-list")
            .w(px(220.0))
            .h_full()
            .overflow_y_scroll()
            .bg(colors.surface_dim)
            .flex()
            .flex_col()
            .p(px(8.0))
            .gap(px(2.0))
            .child(Label::new("Threads").size(LabelSize::Subtitle));

        if let Some(status) = &state.thread_status
            && let Some(placeholder) =
                feature_placeholder(cx, status, "SMS access required.", "sms-status")
        {
            thread_list = thread_list.child(placeholder);
        }

        if state.threads.is_empty() {
            thread_list = thread_list.child(
                div()
                    .text_color(colors.on_subtle_disabled)
                    .italic()
                    .py(px(8.0))
                    .child("No threads yet."),
            );
        } else {
            for thread in &state.threads {
                let is_active = state.active_thread.as_deref() == Some(thread.thread_id.as_str());
                let thread_id = thread.thread_id.clone();
                let e = entity.clone();
                let snippet = thread
                    .last_message
                    .clone()
                    .unwrap_or_else(|| "(no preview)".to_owned());
                let unread_badge: gpui::AnyElement = if thread.unread_count > 0 {
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(8.0))
                        .bg(colors.accent)
                        .text_color(colors.on_accent)
                        .text_size(px(10.0))
                        .child(format!("{}", thread.unread_count))
                        .into_any_element()
                } else {
                    div().into_any_element()
                };
                thread_list = thread_list.child(
                    div()
                        .id(SharedString::from(format!("thread-{thread_id}")))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(4.0))
                        .bg(if is_active {
                            colors.subtle_selected
                        } else {
                            colors.surface_dim
                        })
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .on_click(move |_: &ClickEvent, _, app: &mut App| {
                            let id = thread_id.clone();
                            e.update(app, |m, cx| {
                                if let Some(payload) = m.messages_state.open_thread(id) {
                                    m.send_utility(payload);
                                }
                                cx.notify();
                            });
                        })
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_color(colors.on_neutral)
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child(truncate(&thread.display_name, 22).to_owned()),
                                )
                                .child(unread_badge),
                        )
                        .child(
                            div()
                                .text_color(colors.on_subtle)
                                .text_size(px(11.0))
                                .child(truncate(&snippet, 30).to_owned()),
                        ),
                );
            }
        }

        // Right column: active conversation
        let conversation: gpui::AnyElement = if let Some(active_id) = &state.active_thread {
            let messages = state
                .thread_messages
                .get(active_id)
                .cloned()
                .unwrap_or_default();
            let display_name = state
                .threads
                .iter()
                .find(|t| &t.thread_id == active_id)
                .map(|t| t.display_name.clone())
                .unwrap_or_else(|| active_id.clone());
            let detail_status = state.thread_detail_status.get(active_id).cloned();
            let pending_load = state.pending_thread_open.contains_key(active_id);
            let e_send = entity.clone();
            let e_close = entity.clone();
            let composer_for_send = composer.clone();
            let send_pending = state.pending_send.is_some();
            let send_error = state.send_error.clone();

            let mut conv = div()
                .flex_1()
                .h_full()
                .flex()
                .flex_col()
                .bg(colors.surface)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .p(px(10.0))
                        .child(Label::new(display_name).size(LabelSize::Subtitle))
                        .child(div().flex_1())
                        .child(
                            Button::new("close-thread")
                                .label("Close")
                                .appearance(ButtonAppearance::Subtle)
                                .on_click(move |_: &ClickEvent, _, app: &mut App| {
                                    e_close.update(app, |m, cx| {
                                        m.messages_state.close_thread();
                                        cx.notify();
                                    });
                                }),
                        ),
                )
                .child(Divider::horizontal());

            let mut history = div()
                .id("sms-history")
                .flex_1()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .p(px(10.0));

            if let Some(status) = detail_status
                && let Some(placeholder) = feature_placeholder(
                    cx,
                    &status,
                    "Cannot read this thread.",
                    "sms-thread-status",
                )
            {
                history = history.child(placeholder);
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
                for entry in &messages {
                    let outbound = matches!(entry.direction, MessageDirection::Outbound);
                    let bubble = div()
                        .max_w(px(420.0))
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
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
                        .text_size(px(12.0))
                        .child(entry.body.clone());
                    let mut row = div().flex().flex_row().gap(px(4.0));
                    if outbound {
                        row = row.child(div().flex_1()).child(bubble);
                    } else {
                        row = row.child(bubble).child(div().flex_1());
                    }
                    history = history.child(row);
                }
            }

            conv = conv.child(history).child(Divider::horizontal());

            // Composer
            let composer_row = div()
                .flex()
                .flex_row()
                .gap(px(6.0))
                .items_center()
                .p(px(10.0))
                .child(div().flex_1().child(composer_for_send))
                .child(
                    Button::new("sms-send")
                        .label(if send_pending { "Sending…" } else { "Send" })
                        .appearance(ButtonAppearance::Accent)
                        .disabled(send_pending)
                        .on_click(move |_: &ClickEvent, _, app: &mut App| {
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
                conv = conv.child(
                    div()
                        .px(px(10.0))
                        .pb(px(8.0))
                        .text_color(gpui::rgb(0xff5555u32))
                        .text_size(px(11.0))
                        .child(err),
                );
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
                .child("Select a thread to view the conversation.")
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(colors.surface)
            .child(thread_list)
            .child(Divider::vertical())
            .child(conversation)
    }
}

/// Returns a small placeholder card describing a non-Available `FeatureStatus`. Returns `None`
/// when the feature is `Available` (caller should render normal content).
fn feature_placeholder(
    cx: &mut Context<AppModel>,
    status: &androidconnect_protocol::FeatureStatus,
    fallback_message: &str,
    id_prefix: &'static str,
) -> Option<gpui::AnyElement> {
    use androidconnect_protocol::FeatureState;
    let colors = cx.theme().colors.clone();
    let message = status.message.clone();
    let body = if message.is_empty() {
        fallback_message.to_owned()
    } else {
        message
    };
    let heading = match status.state {
        FeatureState::Available => return None,
        FeatureState::PermissionRequired => "Permission required",
        FeatureState::Disabled => "Feature disabled",
        FeatureState::Unsupported => "Not supported on this device",
        FeatureState::Error => "Error",
    };
    Some(
        div()
            .id(SharedString::from(format!("{id_prefix}-placeholder")))
            .p(px(12.0))
            .rounded(px(4.0))
            .bg(colors.neutral)
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_color(colors.on_neutral)
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(13.0))
                    .child(heading),
            )
            .child(
                div()
                    .text_color(colors.on_subtle)
                    .text_size(px(12.0))
                    .child(body),
            )
            .into_any_element(),
    )
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
        if !connected {
            self.refresh_pair_qr(window);
        }

        let top_bar = self.render_top_bar(cx);
        let nav_rail = self.render_nav_rail(cx);
        let status_bar = self.render_status_bar(cx);

        let content = if !connected {
            self.render_pair_panel(cx).into_any_element()
        } else {
            match self.active_panel {
                Panel::Mirror => self.render_mirror_panel(cx).into_any_element(),
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
            .child(top_bar)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .min_h_0()
                    .child(nav_rail)
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
}
