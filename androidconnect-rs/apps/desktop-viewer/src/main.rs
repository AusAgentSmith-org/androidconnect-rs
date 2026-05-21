mod network;

use std::sync::{Arc, mpsc};
use std::thread;

use anyhow::Result;
use log::error;
use pixels::{Pixels, SurfaceTexture};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use androidconnect_protocol::DEFAULT_CONTROL_PORT;

pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

struct App {
    window: Option<Arc<Window>>,
    pixels: Option<Pixels<'static>>,
    rx: mpsc::Receiver<RgbaFrame>,
    buf_w: u32,
    buf_h: u32,
}

impl App {
    fn new(rx: mpsc::Receiver<RgbaFrame>) -> Self {
        Self {
            window: None,
            pixels: None,
            rx,
            buf_w: 540,
            buf_h: 960,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("AndroidConnect")
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
            WindowEvent::RedrawRequested => {
                // Drain the channel and use only the latest frame.
                let mut latest: Option<RgbaFrame> = None;
                while let Ok(frame) = self.rx.try_recv() {
                    latest = Some(frame);
                }

                if let (Some(frame), Some(px)) = (latest, &mut self.pixels) {
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
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

/// Nearest-neighbour scale `src` (sw×sh RGBA) into `dst` (dw×dh RGBA).
/// Preserves aspect ratio; unfilled margins are black.
fn letterbox_scale(src: &[u8], sw: u32, sh: u32, dst: &mut [u8], dw: u32, dh: u32) {
    dst.fill(0);
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return;
    }
    let scale = (dw as f32 / sw as f32).min(dh as f32 / sh as f32);
    let fit_w = (sw as f32 * scale) as u32;
    let fit_h = (sh as f32 * scale) as u32;
    let x_off = (dw - fit_w) / 2;
    let y_off = (dh - fit_h) / 2;
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

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let bind = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("0.0.0.0:{DEFAULT_CONTROL_PORT}"));

    let (tx, rx) = mpsc::channel::<RgbaFrame>();

    let bind_for_thread = bind.clone();
    thread::spawn(move || {
        if let Err(e) = network::run(&bind_for_thread, tx) {
            error!("network thread: {e:#}");
        }
    });

    log::info!("androidconnect desktop viewer — binding {bind}");

    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut App::new(rx))?;

    Ok(())
}
