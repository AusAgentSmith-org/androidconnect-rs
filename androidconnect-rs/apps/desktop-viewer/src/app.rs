use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use androidconnect_protocol::{
    FileTransferChunk, FileTransferComplete, FileTransferStart, InputEvent, Modifiers, Payload,
    PointerButton, PointerEvent, PointerPhase, SystemAction, TextInput, TransferDirection,
    TransferStatus,
};
use anyhow::{Result, bail};
use eframe::egui;
use egui::{
    Color32, ColorImage, Event, FontId, Frame, Image, Key, Layout, Margin,
    PointerButton as EguiButton, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};

use crate::network;
use crate::status::{ConnectionState, DesktopStatus};
use crate::streaming::{RgbaFrame, map_window_to_frame};

const SHELL_WINDOW_TITLE: &str = "AndroidConnect";

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Panel {
    Notifications,
    Messages,
    Files,
    Phone,
    Mirror,
}

impl Panel {
    fn label(self) -> &'static str {
        match self {
            Panel::Notifications => "Notifications",
            Panel::Messages => "Messages",
            Panel::Files => "Files",
            Panel::Phone => "Phone",
            Panel::Mirror => "Mirror",
        }
    }

    const ALL: [Panel; 5] = [
        Panel::Mirror,
        Panel::Notifications,
        Panel::Messages,
        Panel::Files,
        Panel::Phone,
    ];
}

pub struct App {
    frame_rx: mpsc::Receiver<RgbaFrame>,
    command_tx: mpsc::SyncSender<network::DesktopCommand>,
    status_rx: mpsc::Receiver<network::NetworkStatus>,
    status: DesktopStatus,
    active_panel: Panel,
    /// Latest decoded frame as an egui texture. None until a frame arrives.
    frame_texture: Option<TextureHandle>,
    frame_w: u32,
    frame_h: u32,
    last_mirror_rect: Option<Rect>,
    left_pressed: bool,
}

impl App {
    pub fn new(
        frame_rx: mpsc::Receiver<RgbaFrame>,
        command_tx: mpsc::SyncSender<network::DesktopCommand>,
        status_rx: mpsc::Receiver<network::NetworkStatus>,
        bind: String,
        pairing_code: String,
    ) -> Self {
        let status = DesktopStatus::new(bind, pairing_code);
        Self {
            frame_rx,
            command_tx,
            status_rx,
            status,
            active_panel: Panel::Mirror,
            frame_texture: None,
            frame_w: 0,
            frame_h: 0,
            last_mirror_rect: None,
            left_pressed: false,
        }
    }

    fn send_input(&self, event: InputEvent) {
        let _ = self
            .command_tx
            .try_send(network::DesktopCommand::Input(event));
    }

    fn drain_status(&mut self) {
        while let Ok(status) = self.status_rx.try_recv() {
            self.status.apply(status);
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
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _eframe: &mut eframe::Frame) {
        self.drain_status();
        self.drain_frames(ctx);

        let title = self.status.window_title();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        let connected = matches!(self.status.connection, ConnectionState::Connected)
            && self.status.input_authenticated;

        // Top bar
        egui::TopBottomPanel::top("topbar")
            .frame(Frame::new().inner_margin(Margin::same(8)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("AndroidConnect").heading());
                    ui.separator();
                    let subject = self
                        .status
                        .device_name
                        .clone()
                        .or_else(|| self.status.peer.clone())
                        .unwrap_or_else(|| "no device".to_owned());
                    ui.label(subject);
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        let (color, label) = match self.status.connection {
                            ConnectionState::Listening => (Color32::GRAY, "listening"),
                            ConnectionState::Connected if self.status.input_authenticated => {
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

        // Left nav rail
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(160.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                for panel in Panel::ALL {
                    let selected = panel == self.active_panel;
                    let label = RichText::new(panel.label()).strong();
                    if ui.selectable_label(selected, label).clicked() {
                        self.active_panel = panel;
                    }
                }
            });

        // Bottom status bar
        egui::TopBottomPanel::bottom("statusbar")
            .frame(Frame::new().inner_margin(Margin::same(6)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let battery = self
                        .status
                        .battery_status
                        .clone()
                        .unwrap_or_else(|| "battery —".to_owned());
                    ui.label(format!("🔋 {battery}"));
                    ui.separator();
                    ui.label(format!(
                        "📶 {}",
                        self.status.feature_summary.as_deref().unwrap_or("—")
                    ));
                    ui.separator();
                    ui.label(format!(
                        "🔊 {}",
                        self.status.media_summary.as_deref().unwrap_or("—")
                    ));
                    ui.separator();
                    if let Some(nonce) = self.status.last_pong_nonce {
                        ui.label(format!("♥ #{nonce}"));
                    } else {
                        ui.label("♥ —");
                    }
                });
            });

        // Central panel
        egui::CentralPanel::default().show(ctx, |ui| {
            if !connected {
                draw_pair_screen(ui, &self.status);
                return;
            }
            match self.active_panel {
                Panel::Mirror => draw_mirror_panel(ui, self),
                Panel::Notifications => draw_placeholder(ui, "Notifications", "Coming in Lane 3B"),
                Panel::Messages => draw_placeholder(ui, "Messages", "Coming in Lane 3B"),
                Panel::Files => draw_placeholder(ui, "Files", "Coming in Lane 3C"),
                Panel::Phone => draw_placeholder(ui, "Phone (quick settings)", "Coming in Lane 3B"),
            }
        });

        // Forward egui input events to Android when the Mirror panel is active.
        if connected && self.active_panel == Panel::Mirror {
            self.forward_input_events(ctx);
        }

        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

impl App {
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

fn draw_pair_screen(ui: &mut egui::Ui, status: &DesktopStatus) {
    ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Pair a device").font(FontId::proportional(28.0)));
            ui.add_space(12.0);
            ui.label(
                "Open AndroidConnect on your phone and scan the QR, or enter the code manually.",
            );
            ui.add_space(28.0);

            // QR placeholder — real QR rendering lands in #23.
            Frame::new()
                .stroke(Stroke::new(2.0, Color32::from_gray(120)))
                .corner_radius(4)
                .inner_margin(Margin::same(20))
                .show(ui, |ui| {
                    ui.set_width(220.0);
                    ui.set_height(220.0);
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.label("[QR code]");
                        ui.label("(rendered in #23)");
                    });
                });
            ui.add_space(20.0);

            ui.label(RichText::new("Manual pairing").heading());
            ui.add_space(6.0);
            ui.label(format!("Address: {}", status.bind));
            ui.label(format!(
                "Pairing code: {}",
                crate::status::format_pairing_code(&status.pairing_code)
            ));
            ui.add_space(16.0);
            if !matches!(status.connection, ConnectionState::Listening) {
                ui.label(RichText::new("Waiting for handshake…").italics());
                if let Some(err) = &status.last_error {
                    ui.colored_label(Color32::RED, err);
                }
            }
        });
    });
}

fn draw_mirror_panel(ui: &mut egui::Ui, app: &mut App) {
    if let Some(handle) = &app.frame_texture {
        let available = ui.available_size();
        let aspect = if app.frame_w == 0 || app.frame_h == 0 {
            9.0 / 16.0
        } else {
            app.frame_w as f32 / app.frame_h as f32
        };
        let mut w = available.x;
        let mut h = w / aspect;
        if h > available.y {
            h = available.y;
            w = h * aspect;
        }
        let (rect, _resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click_and_drag());
        app.last_mirror_rect = Some(rect);
        let image = Image::from_texture((handle.id(), rect.size())).fit_to_exact_size(rect.size());
        image.paint_at(ui, rect);
    } else {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(RichText::new("No video yet").heading());
            ui.label("Mirror starts as soon as your phone shares its screen.");
        });
    }
}

fn draw_placeholder(ui: &mut egui::Ui, title: &str, sub: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(60.0);
        ui.label(RichText::new(title).heading());
        ui.add_space(8.0);
        ui.label(sub);
    });
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

/// Run a file-transfer in a background thread; called from a file drop or picker.
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
        .map(|duration| duration.as_micros())
        .unwrap_or_default()
}

pub fn shell_window_title() -> &'static str {
    SHELL_WINDOW_TITLE
}
