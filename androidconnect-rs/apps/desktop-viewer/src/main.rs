mod network;

use std::io::Read;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use log::error;
use pixels::{Pixels, SurfaceTexture};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use androidconnect_protocol::{
    DEFAULT_CONTROL_PORT, InputEvent, Modifiers, PointerButton, PointerEvent, PointerPhase,
    SystemAction, TextInput, normalize_pairing_code,
};

pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

struct App {
    window: Option<Arc<Window>>,
    pixels: Option<Pixels<'static>>,
    frame_rx: mpsc::Receiver<RgbaFrame>,
    input_tx: mpsc::SyncSender<InputEvent>,
    status_rx: mpsc::Receiver<network::NetworkStatus>,
    buf_w: u32,
    buf_h: u32,
    frame_w: u32,
    frame_h: u32,
    cursor_pos: Option<PhysicalPosition<f64>>,
    left_pressed: bool,
    modifiers: ModifiersState,
    status: DesktopStatus,
    window_title: String,
}

impl App {
    fn new(
        frame_rx: mpsc::Receiver<RgbaFrame>,
        input_tx: mpsc::SyncSender<InputEvent>,
        status_rx: mpsc::Receiver<network::NetworkStatus>,
        bind: String,
        pairing_code: String,
    ) -> Self {
        let status = DesktopStatus::new(bind, pairing_code);
        let window_title = status.window_title();
        Self {
            window: None,
            pixels: None,
            frame_rx,
            input_tx,
            status_rx,
            buf_w: 540,
            buf_h: 960,
            frame_w: 0,
            frame_h: 0,
            cursor_pos: None,
            left_pressed: false,
            modifiers: ModifiersState::default(),
            status,
            window_title,
        }
    }

    fn send_input(&self, event: InputEvent) {
        let _ = self.input_tx.try_send(event);
    }

    fn current_modifiers(&self) -> Modifiers {
        Modifiers {
            shift: self.modifiers.shift_key(),
            ctrl: self.modifiers.control_key(),
            alt: self.modifiers.alt_key(),
            meta: self.modifiers.super_key(),
        }
    }

    fn map_cursor(&self, clamp_to_frame: bool) -> Option<(i32, i32)> {
        let pos = self.cursor_pos?;
        map_window_to_frame(
            pos.x,
            pos.y,
            self.buf_w,
            self.buf_h,
            self.frame_w,
            self.frame_h,
            clamp_to_frame,
        )
    }

    fn send_pointer(&self, phase: PointerPhase, button: PointerButton, x: i32, y: i32) {
        self.send_input(InputEvent::Pointer(PointerEvent {
            pointer_id: 0,
            x,
            y,
            phase,
            button,
            delta_x: 0,
            delta_y: 0,
        }));
    }

    fn handle_mouse_button(&mut self, state: ElementState, button: WinitMouseButton) {
        let Some(pointer_button) = pointer_button(button) else {
            return;
        };

        let phase = match state {
            ElementState::Pressed => PointerPhase::Down,
            ElementState::Released => PointerPhase::Up,
        };
        let clamp = self.left_pressed && state == ElementState::Released;
        let Some((x, y)) = self.map_cursor(clamp) else {
            return;
        };

        if pointer_button == PointerButton::Left {
            self.left_pressed = state == ElementState::Pressed;
        }
        self.send_pointer(phase, pointer_button, x, y);
    }

    fn handle_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        self.cursor_pos = Some(position);
        if !self.left_pressed {
            return;
        }

        if let Some((x, y)) = self.map_cursor(true) {
            self.send_pointer(PointerPhase::Move, PointerButton::Left, x, y);
        }
    }

    fn handle_mouse_wheel(&self, delta: MouseScrollDelta) {
        let Some((x, y)) = self.map_cursor(false) else {
            return;
        };
        let (delta_x, delta_y) = scroll_delta(delta);
        if delta_x == 0 && delta_y == 0 {
            return;
        }

        self.send_input(InputEvent::Pointer(PointerEvent {
            pointer_id: 0,
            x,
            y,
            phase: PointerPhase::Wheel,
            button: PointerButton::None,
            delta_x,
            delta_y,
        }));
    }

    fn handle_keyboard_input(&self, event: winit::event::KeyEvent, is_synthetic: bool) {
        if is_synthetic || event.state != ElementState::Pressed {
            return;
        }

        let modifiers = self.current_modifiers();
        if let Some(action) = system_action_for_key(&event.logical_key, modifiers) {
            self.send_input(InputEvent::System(action));
            return;
        }

        if modifiers.ctrl || modifiers.alt || modifiers.meta {
            return;
        }

        if let Some(text) = text_from_key(&event) {
            self.send_input(InputEvent::Text(TextInput { text }));
        }
    }

    fn drain_status_events(&mut self) {
        let mut changed = false;
        while let Ok(status) = self.status_rx.try_recv() {
            self.status.apply(status);
            changed = true;
        }

        if !changed {
            return;
        }

        let title = self.status.window_title();
        if title == self.window_title {
            return;
        }

        self.window_title = title;
        if let Some(window) = &self.window {
            window.set_title(&self.window_title);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title(self.window_title.clone())
            .with_inner_size(PhysicalSize::new(540u32, 960u32));

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                error!("window creation failed: {e}");
                return;
            }
        };

        let inner = window.inner_size();
        let surface =
            SurfaceTexture::new(inner.width.max(1), inner.height.max(1), Arc::clone(&window));
        match Pixels::new(self.buf_w, self.buf_h, surface) {
            Ok(px) => {
                self.pixels = Some(px);
                self.window = Some(window);
            }
            Err(e) => error!("pixels init failed: {e}"),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                let w = size.width.max(1);
                let h = size.height.max(1);
                if let Some(px) = &mut self.pixels {
                    if let Err(e) = px.resize_surface(w, h) {
                        error!("resize_surface: {e}");
                    }
                    if let Err(e) = px.resize_buffer(w, h) {
                        error!("resize_buffer: {e}");
                    }
                }
                self.buf_w = w;
                self.buf_h = h;
            }
            WindowEvent::CursorMoved { position, .. } => self.handle_cursor_moved(position),
            WindowEvent::CursorLeft { .. } => {
                if self.left_pressed
                    && let Some((x, y)) = self.map_cursor(true)
                {
                    self.send_pointer(PointerPhase::Up, PointerButton::Left, x, y);
                }
                self.cursor_pos = None;
                self.left_pressed = false;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_mouse_wheel(delta),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => self.handle_keyboard_input(event, is_synthetic),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::RedrawRequested => {
                // Drain the channel and use only the latest frame.
                let mut latest: Option<RgbaFrame> = None;
                while let Ok(frame) = self.frame_rx.try_recv() {
                    latest = Some(frame);
                }

                if let (Some(frame), Some(px)) = (latest, &mut self.pixels) {
                    self.frame_w = frame.width;
                    self.frame_h = frame.height;
                    letterbox_scale(
                        &frame.data,
                        frame.width,
                        frame.height,
                        px.frame_mut(),
                        self.buf_w,
                        self.buf_h,
                    );
                }

                if let Some(px) = &mut self.pixels
                    && let Err(e) = px.render()
                {
                    error!("render: {e:#}");
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.drain_status_events();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopStatus {
    bind: String,
    pairing_code: String,
    connection: ConnectionState,
    peer: Option<String>,
    device_name: Option<String>,
    input_authenticated: bool,
    video_format: Option<String>,
    last_error: Option<String>,
}

impl DesktopStatus {
    fn new(bind: String, pairing_code: String) -> Self {
        Self {
            bind,
            pairing_code,
            connection: ConnectionState::Listening,
            peer: None,
            device_name: None,
            input_authenticated: false,
            video_format: None,
            last_error: None,
        }
    }

    fn apply(&mut self, status: network::NetworkStatus) {
        match status {
            network::NetworkStatus::Listening { bind } => {
                self.bind = bind;
                self.connection = ConnectionState::Listening;
                self.peer = None;
                self.device_name = None;
                self.input_authenticated = false;
                self.video_format = None;
                self.last_error = None;
            }
            network::NetworkStatus::ClientConnected { peer } => {
                self.connection = ConnectionState::Connected;
                self.peer = Some(peer);
                self.device_name = None;
                self.input_authenticated = false;
                self.video_format = None;
                self.last_error = None;
            }
            network::NetworkStatus::DeviceHello { device_name } => {
                self.device_name = Some(device_name);
                self.connection = ConnectionState::Connected;
                self.last_error = None;
            }
            network::NetworkStatus::PairingAuthenticated => {
                self.input_authenticated = true;
                self.connection = ConnectionState::Connected;
                self.last_error = None;
            }
            network::NetworkStatus::PairingRejected { message } => {
                self.input_authenticated = false;
                self.connection = ConnectionState::Connected;
                self.last_error = Some(format!("pairing rejected: {message}"));
            }
            network::NetworkStatus::VideoFormat {
                width,
                height,
                frame_rate,
                rotation_degrees,
            } => {
                self.video_format = Some(format!(
                    "{width}x{height}@{frame_rate}fps rot {rotation_degrees}"
                ));
                self.connection = ConnectionState::Connected;
            }
            network::NetworkStatus::ClientDisconnected => {
                self.connection = ConnectionState::Listening;
                self.peer = None;
                self.device_name = None;
                self.input_authenticated = false;
                self.video_format = None;
                self.last_error = None;
            }
            network::NetworkStatus::ClientError { message } => {
                self.connection = ConnectionState::Error;
                self.input_authenticated = false;
                self.last_error = Some(message);
            }
        }
    }

    fn window_title(&self) -> String {
        let subject = self
            .device_name
            .as_deref()
            .or(self.peer.as_deref())
            .unwrap_or(self.bind.as_str());
        let auth = if self.input_authenticated {
            "input paired"
        } else {
            "pairing required"
        };

        let state = match self.connection {
            ConnectionState::Listening => format!("listening on {}", self.bind),
            ConnectionState::Connected => {
                let mut state = format!("connected to {subject} - {auth}");
                if let Some(video) = &self.video_format {
                    state.push_str(" - ");
                    state.push_str(video);
                }
                state
            }
            ConnectionState::Error => {
                let message = self.last_error.as_deref().unwrap_or("connection error");
                format!("error: {}", truncate_title(message, 72))
            }
        };

        format!(
            "AndroidConnect - code {} - {state}",
            format_pairing_code(&self.pairing_code)
        )
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum ConnectionState {
    Listening,
    Connected,
    Error,
}

fn format_pairing_code(code: &str) -> String {
    let clean = normalize_pairing_code(code);
    if clean.len() == 6 && clean.chars().all(|ch| ch.is_ascii_digit()) {
        format!("{} {}", &clean[..3], &clean[3..])
    } else {
        clean
    }
}

fn truncate_title(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

/// Nearest-neighbour scale `src` (sw×sh RGBA) into `dst` (dw×dh RGBA).
/// Preserves aspect ratio; unfilled margins are black.
fn letterbox_scale(src: &[u8], sw: u32, sh: u32, dst: &mut [u8], dw: u32, dh: u32) {
    dst.fill(0);
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return;
    }
    let layout = letterbox_layout(sw, sh, dw, dh);
    let fit_w = layout.fit_w;
    let fit_h = layout.fit_h;
    if fit_w == 0 || fit_h == 0 {
        return;
    }
    let x_off = layout.x_off;
    let y_off = layout.y_off;
    let x_ratio = sw as f32 / fit_w as f32;
    let y_ratio = sh as f32 / fit_h as f32;
    for dy in 0..fit_h {
        let sy = (dy as f32 * y_ratio) as u32;
        for dx in 0..fit_w {
            let sx = (dx as f32 * x_ratio) as u32;
            let si = ((sy * sw + sx) * 4) as usize;
            let di = (((dy + y_off) * dw + dx + x_off) * 4) as usize;
            dst[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct LetterboxLayout {
    fit_w: u32,
    fit_h: u32,
    x_off: u32,
    y_off: u32,
}

fn letterbox_layout(sw: u32, sh: u32, dw: u32, dh: u32) -> LetterboxLayout {
    let scale = (dw as f32 / sw as f32).min(dh as f32 / sh as f32);
    let fit_w = (sw as f32 * scale) as u32;
    let fit_h = (sh as f32 * scale) as u32;
    LetterboxLayout {
        fit_w,
        fit_h,
        x_off: (dw - fit_w) / 2,
        y_off: (dh - fit_h) / 2,
    }
}

fn map_window_to_frame(
    x: f64,
    y: f64,
    window_w: u32,
    window_h: u32,
    frame_w: u32,
    frame_h: u32,
    clamp_to_frame: bool,
) -> Option<(i32, i32)> {
    if frame_w == 0 || frame_h == 0 || window_w == 0 || window_h == 0 {
        return None;
    }

    let layout = letterbox_layout(frame_w, frame_h, window_w, window_h);
    if layout.fit_w == 0 || layout.fit_h == 0 {
        return None;
    }
    let left = layout.x_off as f64;
    let top = layout.y_off as f64;
    let right = (layout.x_off + layout.fit_w) as f64;
    let bottom = (layout.y_off + layout.fit_h) as f64;

    if !clamp_to_frame && (x < left || x >= right || y < top || y >= bottom) {
        return None;
    }

    let clamped_x = x.clamp(left, (right - 1.0).max(left));
    let clamped_y = y.clamp(top, (bottom - 1.0).max(top));
    let sx = ((clamped_x - left) * frame_w as f64 / layout.fit_w as f64).floor() as i32;
    let sy = ((clamped_y - top) * frame_h as f64 / layout.fit_h as f64).floor() as i32;
    Some((
        sx.clamp(0, frame_w.saturating_sub(1) as i32),
        sy.clamp(0, frame_h.saturating_sub(1) as i32),
    ))
}

fn pointer_button(button: WinitMouseButton) -> Option<PointerButton> {
    match button {
        WinitMouseButton::Left => Some(PointerButton::Left),
        WinitMouseButton::Middle => Some(PointerButton::Middle),
        WinitMouseButton::Right => Some(PointerButton::Right),
        WinitMouseButton::Back => Some(PointerButton::Back),
        WinitMouseButton::Forward => Some(PointerButton::Forward),
        WinitMouseButton::Other(_) => None,
    }
}

fn scroll_delta(delta: MouseScrollDelta) -> (i32, i32) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => {
            ((x * 120.0).round() as i32, (y * 120.0).round() as i32)
        }
        MouseScrollDelta::PixelDelta(pos) => (pos.x.round() as i32, pos.y.round() as i32),
    }
}

fn system_action_for_key(key: &Key, modifiers: Modifiers) -> Option<SystemAction> {
    match key {
        Key::Named(NamedKey::Escape | NamedKey::GoBack | NamedKey::BrowserBack) => {
            Some(SystemAction::Back)
        }
        Key::Named(NamedKey::GoHome) => Some(SystemAction::Home),
        Key::Named(NamedKey::AppSwitch) => Some(SystemAction::Recents),
        Key::Named(NamedKey::Power | NamedKey::PowerOff | NamedKey::Standby) => {
            Some(SystemAction::LockScreen)
        }
        Key::Character(value) if modifiers.ctrl && modifiers.alt && !modifiers.meta => {
            match value.to_lowercase().as_str() {
                "b" => Some(SystemAction::Back),
                "h" => Some(SystemAction::Home),
                "r" => Some(SystemAction::Recents),
                "l" => Some(SystemAction::LockScreen),
                _ => None,
            }
        }
        _ => None,
    }
}

fn text_from_key(event: &winit::event::KeyEvent) -> Option<String> {
    if let Some(text) = &event.text
        && !text.is_empty()
        && text.as_str() != "\u{1b}"
    {
        return Some(text.to_string());
    }

    match &event.logical_key {
        Key::Named(NamedKey::Backspace) => Some("\u{8}".to_owned()),
        Key::Named(NamedKey::Delete) => Some("\u{7f}".to_owned()),
        Key::Named(NamedKey::Enter) => Some("\n".to_owned()),
        Key::Named(NamedKey::Tab) => Some("\t".to_owned()),
        _ => None,
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = parse_config(std::env::args().skip(1))?;

    let (frame_tx, frame_rx) = mpsc::channel::<RgbaFrame>();
    let (input_tx, input_rx) = mpsc::sync_channel::<InputEvent>(1024);
    let (status_tx, status_rx) = mpsc::channel::<network::NetworkStatus>();

    let bind_for_thread = config.bind.clone();
    let pairing_code_for_thread = config.pairing_code.clone();
    let status_tx_for_error = status_tx.clone();
    thread::spawn(move || {
        if let Err(e) = network::run(
            &bind_for_thread,
            frame_tx,
            input_rx,
            pairing_code_for_thread,
            status_tx,
        ) {
            error!("network thread: {e:#}");
            let _ = status_tx_for_error.send(network::NetworkStatus::ClientError {
                message: e.to_string(),
            });
        }
    });

    log::info!("androidconnect desktop viewer — binding {}", config.bind);
    log::info!("pairing code: {}", config.pairing_code);

    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut App::new(
        frame_rx,
        input_tx,
        status_rx,
        config.bind,
        config.pairing_code,
    ))?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    bind: String,
    pairing_code: String,
}

fn parse_config(args: impl IntoIterator<Item = String>) -> Result<Config> {
    let mut bind = None;
    let mut pairing_code = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                bail!("usage: androidconnect-desktop-viewer [BIND] [--pairing-code CODE]");
            }
            "--pairing-code" => {
                let code = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--pairing-code requires a value"))?;
                pairing_code = Some(normalize_pairing_code(&code));
            }
            value if value.starts_with("--pairing-code=") => {
                let code = value.trim_start_matches("--pairing-code=");
                pairing_code = Some(normalize_pairing_code(code));
            }
            value if value.starts_with('-') => bail!("unknown argument: {value}"),
            value => {
                if bind.replace(value.to_owned()).is_some() {
                    bail!("multiple bind addresses provided");
                }
            }
        }
    }

    let pairing_code = pairing_code.unwrap_or_else(generate_pairing_code);
    if pairing_code.is_empty() {
        bail!("pairing code cannot be empty");
    }

    Ok(Config {
        bind: bind.unwrap_or_else(|| format!("0.0.0.0:{DEFAULT_CONTROL_PORT}")),
        pairing_code,
    })
}

fn generate_pairing_code() -> String {
    let mut bytes = [0_u8; 8];
    if fill_random(&mut bytes).is_err() {
        fill_fallback_random(&mut bytes);
    }
    let value = u64::from_le_bytes(bytes) % 1_000_000;
    format!("{value:06}")
}

fn fill_random(output: &mut [u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::open("/dev/urandom")?;
    file.read_exact(output)
}

fn fill_fallback_random(output: &mut [u8]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut seed = now ^ ((std::process::id() as u128) << 64);
    for chunk in output.chunks_mut(8) {
        seed ^= seed << 7;
        seed ^= seed >> 9;
        seed = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let bytes = seed.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_centered_letterbox_coordinates() {
        assert_eq!(
            map_window_to_frame(270.0, 480.0, 540, 960, 1080, 2340, false),
            Some((541, 1170))
        );
    }

    #[test]
    fn rejects_pointer_outside_letterbox_by_default() {
        assert_eq!(
            map_window_to_frame(10.0, 10.0, 1000, 500, 500, 1000, false),
            None
        );
    }

    #[test]
    fn clamps_drag_points_to_frame() {
        assert_eq!(
            map_window_to_frame(-10.0, 250.0, 1000, 500, 500, 1000, true),
            Some((0, 500))
        );
    }

    #[test]
    fn parses_bind_and_pairing_code() {
        let config = parse_config([
            "127.0.0.1:48172".to_owned(),
            "--pairing-code".to_owned(),
            " 12 ab ".to_owned(),
        ])
        .expect("config");

        assert_eq!(
            config,
            Config {
                bind: "127.0.0.1:48172".to_owned(),
                pairing_code: "12AB".to_owned(),
            }
        );
    }

    #[test]
    fn formats_six_digit_pairing_code_for_title() {
        assert_eq!(format_pairing_code("123456"), "123 456");
        assert_eq!(format_pairing_code(" ab cd "), "ABCD");
    }

    #[test]
    fn status_title_tracks_pairing_and_video() {
        let mut status = DesktopStatus::new("0.0.0.0:48172".to_owned(), "123456".to_owned());
        assert_eq!(
            status.window_title(),
            "AndroidConnect - code 123 456 - listening on 0.0.0.0:48172"
        );

        status.apply(network::NetworkStatus::DeviceHello {
            device_name: "Pixel".to_owned(),
        });
        assert_eq!(
            status.window_title(),
            "AndroidConnect - code 123 456 - connected to Pixel - pairing required"
        );

        status.apply(network::NetworkStatus::PairingAuthenticated);
        status.apply(network::NetworkStatus::VideoFormat {
            width: 1080,
            height: 2340,
            frame_rate: 30,
            rotation_degrees: 0,
        });
        assert_eq!(
            status.window_title(),
            "AndroidConnect - code 123 456 - connected to Pixel - input paired - 1080x2340@30fps rot 0"
        );
    }
}
