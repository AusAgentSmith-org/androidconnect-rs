use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use eframe::egui;
use egui::{
    Color32, ColorImage, Event, Frame, Key, Layout, Margin, PointerButton as EguiButton, Pos2,
    Rect, RichText, Sense, TextureHandle, TextureOptions, Vec2,
};
use log::error;
use qrcode::{Color as QrColor, QrCode};

use androidconnect_protocol::{
    FileTransferChunk, FileTransferComplete, FileTransferStart, InputEvent, Modifiers, Payload,
    PointerButton, PointerEvent, PointerPhase, SystemAction, TextInput, TransferDirection,
    TransferStatus,
    qr::{QrPairingPayload, encode_qr_payload},
};

use crate::network;
use crate::panels::{
    self, Panel, files::FilesState, notifications::NotificationsState, phone::PhoneState,
};
use crate::status::{ConnectionState, DesktopStatus};
use crate::streaming::{RgbaFrame, map_window_to_frame};

const SHELL_WINDOW_TITLE: &str = "AndroidConnect";

pub struct App {
    frame_rx: mpsc::Receiver<RgbaFrame>,
    command_tx: mpsc::SyncSender<network::DesktopCommand>,
    status_rx: mpsc::Receiver<network::NetworkStatus>,
    event_rx: mpsc::Receiver<network::DesktopEvent>,
    status: DesktopStatus,
    desktop_id: String,
    desktop_name: String,
    active_panel: Panel,
    frame_texture: Option<TextureHandle>,
    frame_w: u32,
    frame_h: u32,
    last_mirror_rect: Option<Rect>,
    left_pressed: bool,
    pair_qr_cache: Option<(String, TextureHandle)>,
    notifications: NotificationsState,
    phone_state: PhoneState,
    files_state: FilesState,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        frame_rx: mpsc::Receiver<RgbaFrame>,
        command_tx: mpsc::SyncSender<network::DesktopCommand>,
        status_rx: mpsc::Receiver<network::NetworkStatus>,
        event_rx: mpsc::Receiver<network::DesktopEvent>,
        bind: String,
        pairing_code: String,
        desktop_id: String,
        desktop_name: String,
    ) -> Self {
        let status = DesktopStatus::new(bind, pairing_code);
        let mut files_state = FilesState::default();
        files_state.current_path = "/sdcard".to_owned();
        Self {
            frame_rx,
            command_tx,
            status_rx,
            event_rx,
            status,
            desktop_id,
            desktop_name,
            active_panel: Panel::Mirror,
            frame_texture: None,
            frame_w: 0,
            frame_h: 0,
            last_mirror_rect: None,
            left_pressed: false,
            pair_qr_cache: None,
            notifications: NotificationsState::default(),
            phone_state: PhoneState::default(),
            files_state,
        }
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

    fn drain_status(&mut self) {
        while let Ok(status) = self.status_rx.try_recv() {
            self.status.apply(status);
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                network::DesktopEvent::NotificationPosted(n) => self.notifications.posted(n),
                network::DesktopEvent::NotificationRemoved(r) => self.notifications.removed(r),
                network::DesktopEvent::FileBrowseResponse(r) => self.files_state.record_response(r),
                network::DesktopEvent::SessionLost => {
                    self.notifications.clear();
                    self.phone_state = PhoneState::default();
                    self.files_state = FilesState::default();
                    self.files_state.current_path = "/sdcard".to_owned();
                    self.frame_texture = None;
                    self.frame_w = 0;
                    self.frame_h = 0;
                    self.last_mirror_rect = None;
                }
            }
        }
    }

    fn drain_frames(&mut self, ctx: &egui::Context) {
        let mut latest: Option<RgbaFrame> = None;
        while let Ok(frame) = self.frame_rx.try_recv() {
            latest = Some(frame);
        }
        let Some(frame) = latest else {
            return;
        };
        self.frame_w = frame.width;
        self.frame_h = frame.height;
        let pixels: Vec<Color32> = frame
            .data
            .chunks_exact(4)
            .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
            .collect();
        let image = ColorImage {
            size: [frame.width as usize, frame.height as usize],
            pixels,
            source_size: Vec2::new(frame.width as f32, frame.height as f32),
        };
        match &mut self.frame_texture {
            Some(handle) => handle.set(image, TextureOptions::LINEAR),
            None => {
                self.frame_texture =
                    Some(ctx.load_texture("androidconnect-mirror", image, TextureOptions::LINEAR));
            }
        }
    }

    fn drain_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            if let Some(path) = f.path {
                let command_tx = self.command_tx.clone();
                std::thread::spawn(move || {
                    if let Err(e) = send_file_to_android(command_tx, path) {
                        error!("send dropped file failed: {e:#}");
                    }
                });
            }
        }
    }

    fn process_pending_downloads(&mut self) {
        // Files panel records (remote_path, local_dest); request the stream so the
        // network thread can ferry it back. For MVP we just kick off a FileBrowseRequest
        // marker — the actual download surface uses the existing file-transfer flow on
        // the Android side which is out of scope for this commit.
        let drained: Vec<_> = self.files_state.pending_downloads.drain(..).collect();
        for (remote, dest) in drained {
            log::info!("download request: {remote} -> {}", dest.display());
            // Future: emit a file pull request. For now leave a log breadcrumb.
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _eframe: &mut eframe::Frame) {
        self.drain_status();
        self.drain_events();
        self.drain_frames(ctx);
        self.drain_dropped_files(ctx);
        self.process_pending_downloads();

        let title = self.status.window_title();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        let connected = matches!(self.status.connection, ConnectionState::Connected)
            && self.status.input_authenticated;

        draw_top_bar(ctx, &self.status);
        draw_nav_rail(ctx, &mut self.active_panel);
        draw_status_bar(ctx, &self.status);

        let pair_texture = if !connected {
            self.refresh_pair_qr(ctx)
        } else {
            None
        };

        let mut emitted: Vec<Payload> = Vec::new();
        let mut mirror_rect: Option<Rect> = None;

        egui::CentralPanel::default().show(ctx, |ui| {
            if !connected {
                panels::pair::draw(ui, &self.status, pair_texture.as_ref());
                return;
            }
            match self.active_panel {
                Panel::Mirror => {
                    mirror_rect = panels::mirror::draw(
                        ui,
                        panels::mirror::DrawInput {
                            texture: self.frame_texture.as_ref(),
                            frame_w: self.frame_w,
                            frame_h: self.frame_h,
                        },
                    );
                }
                Panel::Notifications => {
                    emitted
                        .extend(panels::notifications::draw(ui, &mut self.notifications).payloads);
                }
                Panel::Messages => panels::messages::draw(ui),
                Panel::Files => {
                    emitted.extend(panels::files::draw(ui, &mut self.files_state).payloads);
                }
                Panel::Phone => {
                    emitted.extend(
                        panels::phone::draw(ui, &self.status, &mut self.phone_state).payloads,
                    );
                }
            }
        });

        for payload in emitted {
            self.send_utility(payload);
        }
        if mirror_rect.is_some() {
            self.last_mirror_rect = mirror_rect;
        }

        if connected && self.active_panel == Panel::Mirror {
            self.forward_input_events(ctx);
        }

        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn draw_top_bar(ctx: &egui::Context, status: &DesktopStatus) {
    egui::TopBottomPanel::top("topbar")
        .frame(Frame::new().inner_margin(Margin::same(8)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("AndroidConnect").heading());
                ui.separator();
                let subject = status
                    .device_name
                    .clone()
                    .or_else(|| status.peer.clone())
                    .unwrap_or_else(|| "no device".to_owned());
                ui.label(subject);
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    let (color, label) = match status.connection {
                        ConnectionState::Listening => (Color32::GRAY, "listening"),
                        ConnectionState::Connected if status.input_authenticated => {
                            (Color32::from_rgb(80, 200, 120), "paired")
                        }
                        ConnectionState::Connected => (Color32::YELLOW, "pairing"),
                        ConnectionState::Error => (Color32::RED, "error"),
                    };
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, color);
                    ui.label(label);
                });
            });
        });
}

fn draw_nav_rail(ctx: &egui::Context, active: &mut Panel) {
    egui::SidePanel::left("nav")
        .resizable(false)
        .exact_width(160.0)
        .show(ctx, |ui| {
            ui.add_space(8.0);
            for panel in Panel::ALL {
                let selected = panel == *active;
                let label = RichText::new(panel.label()).strong();
                if ui.selectable_label(selected, label).clicked() {
                    *active = panel;
                }
            }
        });
}

fn draw_status_bar(ctx: &egui::Context, status: &DesktopStatus) {
    egui::TopBottomPanel::bottom("statusbar")
        .frame(Frame::new().inner_margin(Margin::same(6)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let battery = status
                    .battery_status
                    .clone()
                    .unwrap_or_else(|| "—".to_owned());
                ui.label(format!("🔋 {battery}"));
                ui.separator();
                ui.label(format!(
                    "📶 {}",
                    status.wifi_summary.as_deref().unwrap_or("—")
                ));
                ui.separator();
                ui.label(format!(
                    "🅱 {}",
                    match status.bluetooth_enabled {
                        Some(true) => "on",
                        Some(false) => "off",
                        None => "—",
                    }
                ));
                ui.separator();
                ui.label(format!(
                    "🌙 {}",
                    match status.dnd_mode {
                        Some(androidconnect_protocol::DndMode::Off) | None => "—",
                        Some(androidconnect_protocol::DndMode::Priority) => "priority",
                        Some(androidconnect_protocol::DndMode::Alarms) => "alarms",
                        Some(androidconnect_protocol::DndMode::TotalSilence) => "silence",
                    }
                ));
                ui.separator();
                ui.label(format!(
                    "🔊 {}",
                    status
                        .volume_percent
                        .map(|p| format!("{p}%"))
                        .unwrap_or_else(|| "—".to_owned())
                ));
                ui.separator();
                if let Some(nonce) = status.last_pong_nonce {
                    ui.label(format!("♥ #{nonce}"));
                } else {
                    ui.label("♥ —");
                }
            });
        });
}

impl App {
    fn refresh_pair_qr(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        let expires_ms = now_ms.saturating_add(5 * 60 * 1000);

        let payload = QrPairingPayload::new(
            vec![self.status.bind.clone()],
            self.status.pairing_code.clone(),
            self.desktop_id.clone(),
            self.desktop_name.clone(),
            now_ms,
            expires_ms,
        );
        let uri = encode_qr_payload(&payload).ok()?;

        let cache_key = format!(
            "v={}|addrs={:?}|tok={}|id={}",
            payload.protocol_version, payload.addresses, payload.pairing_token, payload.desktop_id,
        );
        if self
            .pair_qr_cache
            .as_ref()
            .is_some_and(|(k, _)| k == &cache_key)
        {
            return self.pair_qr_cache.as_ref().map(|(_, h)| h.clone());
        }

        let image = render_qr_image(&uri)?;
        let handle = ctx.load_texture("androidconnect-pair-qr", image, TextureOptions::NEAREST);
        self.pair_qr_cache = Some((cache_key, handle.clone()));
        Some(handle)
    }

    fn forward_input_events(&mut self, ctx: &egui::Context) {
        let Some(mirror_rect) = self.last_mirror_rect else {
            return;
        };
        let frame_w = self.frame_w;
        let frame_h = self.frame_h;
        if frame_w == 0 || frame_h == 0 {
            return;
        }
        let window_w = mirror_rect.width().max(1.0) as u32;
        let window_h = mirror_rect.height().max(1.0) as u32;

        ctx.input(|i| {
            for event in &i.events {
                match event {
                    Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        ..
                    } => {
                        let Some(btn) = map_pointer_button(*button) else {
                            continue;
                        };
                        if btn == PointerButton::Left {
                            self.left_pressed = *pressed;
                        }
                        let clamp = !*pressed && btn == PointerButton::Left;
                        if let Some((x, y)) = map_into_panel(
                            *pos,
                            mirror_rect,
                            window_w,
                            window_h,
                            frame_w,
                            frame_h,
                            clamp,
                        ) {
                            self.send_input(InputEvent::Pointer(PointerEvent {
                                pointer_id: 0,
                                x,
                                y,
                                phase: if *pressed {
                                    PointerPhase::Down
                                } else {
                                    PointerPhase::Up
                                },
                                button: btn,
                                delta_x: 0,
                                delta_y: 0,
                            }));
                        }
                    }
                    Event::PointerMoved(pos) => {
                        if !self.left_pressed {
                            continue;
                        }
                        if let Some((x, y)) = map_into_panel(
                            *pos,
                            mirror_rect,
                            window_w,
                            window_h,
                            frame_w,
                            frame_h,
                            true,
                        ) {
                            self.send_input(InputEvent::Pointer(PointerEvent {
                                pointer_id: 0,
                                x,
                                y,
                                phase: PointerPhase::Move,
                                button: PointerButton::Left,
                                delta_x: 0,
                                delta_y: 0,
                            }));
                        }
                    }
                    Event::PointerGone => {
                        if self.left_pressed
                            && let Some(pos) = i.pointer.latest_pos()
                            && let Some((x, y)) = map_into_panel(
                                pos,
                                mirror_rect,
                                window_w,
                                window_h,
                                frame_w,
                                frame_h,
                                true,
                            )
                        {
                            self.send_input(InputEvent::Pointer(PointerEvent {
                                pointer_id: 0,
                                x,
                                y,
                                phase: PointerPhase::Up,
                                button: PointerButton::Left,
                                delta_x: 0,
                                delta_y: 0,
                            }));
                        }
                        self.left_pressed = false;
                    }
                    Event::MouseWheel { delta, .. } => {
                        if let Some(pos) = i.pointer.latest_pos()
                            && let Some((x, y)) = map_into_panel(
                                pos,
                                mirror_rect,
                                window_w,
                                window_h,
                                frame_w,
                                frame_h,
                                false,
                            )
                        {
                            let dx = (delta.x * 120.0).round() as i32;
                            let dy = (delta.y * 120.0).round() as i32;
                            if dx == 0 && dy == 0 {
                                continue;
                            }
                            self.send_input(InputEvent::Pointer(PointerEvent {
                                pointer_id: 0,
                                x,
                                y,
                                phase: PointerPhase::Wheel,
                                button: PointerButton::None,
                                delta_x: dx,
                                delta_y: dy,
                            }));
                        }
                    }
                    Event::Text(text) => {
                        if text.is_empty() {
                            continue;
                        }
                        self.send_input(InputEvent::Text(TextInput { text: text.clone() }));
                    }
                    Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let mods = Modifiers {
                            shift: modifiers.shift,
                            ctrl: modifiers.ctrl,
                            alt: modifiers.alt,
                            meta: modifiers.mac_cmd || modifiers.command,
                        };
                        if let Some(action) = system_action_for_egui_key(*key, mods) {
                            self.send_input(InputEvent::System(action));
                            continue;
                        }
                        if let Some(text) = synthetic_text_for_key(*key) {
                            self.send_input(InputEvent::Text(TextInput { text }));
                        }
                    }
                    _ => {}
                }
            }
        });
    }
}

fn map_pointer_button(button: EguiButton) -> Option<PointerButton> {
    match button {
        EguiButton::Primary => Some(PointerButton::Left),
        EguiButton::Secondary => Some(PointerButton::Right),
        EguiButton::Middle => Some(PointerButton::Middle),
        EguiButton::Extra1 => Some(PointerButton::Back),
        EguiButton::Extra2 => Some(PointerButton::Forward),
    }
}

fn map_into_panel(
    pos: Pos2,
    rect: Rect,
    window_w: u32,
    window_h: u32,
    frame_w: u32,
    frame_h: u32,
    clamp: bool,
) -> Option<(i32, i32)> {
    if !rect.contains(pos) && !clamp {
        return None;
    }
    let lx = (pos.x - rect.min.x) as f64;
    let ly = (pos.y - rect.min.y) as f64;
    map_window_to_frame(lx, ly, window_w, window_h, frame_w, frame_h, clamp)
}

fn system_action_for_egui_key(key: Key, modifiers: Modifiers) -> Option<SystemAction> {
    if modifiers.ctrl && modifiers.alt && !modifiers.meta {
        return match key {
            Key::B => Some(SystemAction::Back),
            Key::H => Some(SystemAction::Home),
            Key::R => Some(SystemAction::Recents),
            Key::L => Some(SystemAction::LockScreen),
            _ => None,
        };
    }
    match key {
        Key::Escape => Some(SystemAction::Back),
        _ => None,
    }
}

fn synthetic_text_for_key(key: Key) -> Option<String> {
    match key {
        Key::Backspace => Some("\u{8}".to_owned()),
        Key::Delete => Some("\u{7f}".to_owned()),
        Key::Enter => Some("\n".to_owned()),
        Key::Tab => Some("\t".to_owned()),
        _ => None,
    }
}

#[allow(dead_code)]
pub fn send_file_to_android(
    command_tx: mpsc::SyncSender<network::DesktopCommand>,
    path: PathBuf,
) -> Result<()> {
    let mut file = File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("only individual files can be sent");
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .map(sanitize_file_name)
        .unwrap_or_else(|| "file".to_owned());
    let transfer_id = format!("desktop-{}-{}", now_micros(), std::process::id());
    command_tx.send(network::DesktopCommand::Utility(
        Payload::FileTransferStart(FileTransferStart {
            transfer_id: transfer_id.clone(),
            direction: TransferDirection::DesktopToAndroid,
            file_name,
            mime_type: None,
            size_bytes: Some(metadata.len()),
            target_path: None,
        }),
    ))?;

    let mut offset = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        command_tx.send(network::DesktopCommand::Utility(
            Payload::FileTransferChunk(FileTransferChunk {
                transfer_id: transfer_id.clone(),
                offset,
                data: buffer[..read].to_vec(),
            }),
        ))?;
        offset += read as u64;
    }

    command_tx.send(network::DesktopCommand::Utility(
        Payload::FileTransferComplete(FileTransferComplete {
            transfer_id,
            status: TransferStatus::Completed,
            message: None,
        }),
    ))?;
    Ok(())
}

#[allow(dead_code)]
fn sanitize_file_name(path: &str) -> String {
    let clean: String = path
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '\0' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect();
    let clean = clean.trim_matches('.').trim();
    if clean.is_empty() {
        "file".to_owned()
    } else {
        clean.to_owned()
    }
}

#[allow(dead_code)]
fn now_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or_default()
}

pub fn shell_window_title() -> &'static str {
    SHELL_WINDOW_TITLE
}

fn render_qr_image(uri: &str) -> Option<ColorImage> {
    const QUIET_MODULES: usize = 4;
    const MIN_MODULE_PX: usize = 8;

    let code = QrCode::new(uri.as_bytes()).ok()?;
    let modules = code.width();
    let colors = code.to_colors();
    debug_assert_eq!(colors.len(), modules * modules);
    let total_modules = modules + QUIET_MODULES * 2;
    let size_px = total_modules * MIN_MODULE_PX;

    let dark = Color32::BLACK;
    let light = Color32::WHITE;
    let mut pixels = vec![light; size_px * size_px];

    for my in 0..modules {
        for mx in 0..modules {
            if colors[my * modules + mx] != QrColor::Dark {
                continue;
            }
            let x0 = (mx + QUIET_MODULES) * MIN_MODULE_PX;
            let y0 = (my + QUIET_MODULES) * MIN_MODULE_PX;
            for py in 0..MIN_MODULE_PX {
                let row_start = (y0 + py) * size_px + x0;
                for _ in 0..MIN_MODULE_PX {
                    pixels[row_start] = dark;
                }
                // unrolled fill — clobber MIN_MODULE_PX consecutive cells starting at row_start
                for px in 0..MIN_MODULE_PX {
                    pixels[row_start + px] = dark;
                }
            }
        }
    }

    Some(ColorImage {
        size: [size_px, size_px],
        pixels,
        source_size: Vec2::new(size_px as f32, size_px as f32),
    })
}
