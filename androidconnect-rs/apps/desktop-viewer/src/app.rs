use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use log::error;
use pixels::{Pixels, SurfaceTexture};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use androidconnect_protocol::{
    FileTransferChunk, FileTransferComplete, FileTransferStart, InputEvent, MediaControl,
    Modifiers, Payload, PointerButton, PointerEvent, PointerPhase, TextInput, TransferDirection,
    TransferStatus,
};

use crate::network;
use crate::status::DesktopStatus;
use crate::streaming::{
    RgbaFrame, fit_window_to_video, letterbox_scale, map_window_to_frame, media_control_for_key,
    pointer_button, scroll_delta, system_action_for_key, text_from_key,
};

pub struct App {
    window: Option<Arc<Window>>,
    pixels: Option<Pixels<'static>>,
    frame_rx: mpsc::Receiver<RgbaFrame>,
    command_tx: mpsc::SyncSender<network::DesktopCommand>,
    status_rx: mpsc::Receiver<network::NetworkStatus>,
    buf_w: u32,
    buf_h: u32,
    frame_w: u32,
    frame_h: u32,
    format_w: u32,
    format_h: u32,
    cursor_pos: Option<PhysicalPosition<f64>>,
    left_pressed: bool,
    modifiers: ModifiersState,
    status: DesktopStatus,
    window_title: String,
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
        let window_title = status.window_title();
        Self {
            window: None,
            pixels: None,
            frame_rx,
            command_tx,
            status_rx,
            buf_w: 540,
            buf_h: 960,
            frame_w: 0,
            frame_h: 0,
            format_w: 0,
            format_h: 0,
            cursor_pos: None,
            left_pressed: false,
            modifiers: ModifiersState::default(),
            status,
            window_title,
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

    fn send_file_to_android(&self, path: PathBuf) {
        let command_tx = self.command_tx.clone();
        thread::spawn(move || {
            if let Err(error) = send_file_to_android(command_tx, path) {
                error!("send file to Android failed: {error:#}");
            }
        });
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
        if let Some(action) = media_control_for_key(&event.logical_key) {
            self.send_utility(Payload::MediaControl(MediaControl { action }));
            return;
        }

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
        let mut resize_to: Option<(u32, u32)> = None;

        while let Ok(status) = self.status_rx.try_recv() {
            if let network::NetworkStatus::VideoFormat { width, height, .. } = &status {
                let (w, h) = (*width, *height);
                if w != self.format_w || h != self.format_h {
                    self.format_w = w;
                    self.format_h = h;
                    resize_to = Some(fit_window_to_video(w, h, 540));
                }
            }
            self.status.apply(status);
            changed = true;
        }

        if let (Some((tw, th)), Some(window)) = (resize_to, &self.window) {
            let _ = window.request_inner_size(PhysicalSize::new(tw, th));
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
            WindowEvent::DroppedFile(path) => self.send_file_to_android(path),
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

fn send_file_to_android(
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

fn now_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros())
        .unwrap_or_default()
}
