/// Spike: stream RGBA video frames through the new VideoFrame primitive in FluentGUI/GPUI.
///
/// Validates the new API:
///   - `Window::alloc_video_texture(w, h)` — allocates a dedicated Rgba8Unorm GPU texture once
///   - `Window::paint_video_frame(id, bounds, rgba_bytes)` — uploads raw RGBA and inserts the
///     VideoFrame primitive; no atlas round-trip, no drop_image bookkeeping required
///   - A background task feeds 30 fps synthetic frames into a GPUI Entity
use std::sync::Arc;
use std::time::{Duration, Instant};

use fluent_app::FluentApp;
use gpui::{
    Context, Entity, IntoElement, Styled, VideoTextureId, Window, canvas, div, prelude::*, px,
};

// --- Model ---------------------------------------------------------------

struct FrameState {
    current: Option<Arc<Vec<u8>>>,
    fps: f32,
    frame_count: u64,
    last_fps_tick: Instant,
    fps_count: u64,
}

impl FrameState {
    fn new() -> Self {
        Self {
            current: None,
            fps: 0.0,
            frame_count: 0,
            last_fps_tick: Instant::now(),
            fps_count: 0,
        }
    }

    fn push_frame(&mut self, pixels: Arc<Vec<u8>>, cx: &mut Context<Self>) {
        self.current = Some(pixels);
        self.frame_count += 1;
        self.fps_count += 1;
        let elapsed = self.last_fps_tick.elapsed();
        if elapsed >= Duration::from_secs(1) {
            self.fps = self.fps_count as f32 / elapsed.as_secs_f32();
            self.fps_count = 0;
            self.last_fps_tick = Instant::now();
        }
        cx.notify();
    }
}

// --- View ----------------------------------------------------------------

struct VideoView {
    state: Entity<FrameState>,
    video_texture: Option<VideoTextureId>,
    width: u32,
    height: u32,
}

impl VideoView {
    fn new(state: Entity<FrameState>, width: u32, height: u32, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            video_texture: None,
            width,
            height,
        }
    }
}

impl Render for VideoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Lazily allocate the GPU video texture on first render.
        if self.video_texture.is_none() {
            self.video_texture = window.alloc_video_texture(self.width, self.height).ok();
        }

        let state = self.state.read(cx);
        let fps_label = format!("Frame #{} — {:.1} fps", state.frame_count, state.fps);
        let current = state.current.clone();
        let video_id = self.video_texture;

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::black())
            .child(
                canvas(
                    move |_bounds, _window, _cx| (current, video_id),
                    |bounds, (pixels_opt, id_opt), window, _cx| {
                        let (Some(pixels), Some(id)) = (pixels_opt, id_opt) else {
                            return;
                        };
                        let _ = window.paint_video_frame(id, bounds, &pixels);
                    },
                )
                .flex_grow(),
            )
            .child(
                div()
                    .h(px(24.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .bg(gpui::rgb(0x1a1a1a))
                    .child(
                        div()
                            .text_color(gpui::rgb(0xaaaaaa))
                            .text_size(px(12.0))
                            .child(fps_label),
                    ),
            )
    }
}

// --- Frame generator -----------------------------------------------------

fn make_frame(frame_num: u64, width: u32, height: u32) -> Arc<Vec<u8>> {
    let bar_x = ((frame_num * 4) % width as u64) as u32;
    let hue_r = ((frame_num * 2) % 256) as u8;
    let hue_g = ((frame_num * 3 + 85) % 256) as u8;
    let hue_b = ((frame_num * 5 + 170) % 256) as u8;

    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for _y in 0..height {
        for x in 0..width {
            let is_bar = x >= bar_x && x < bar_x + 8;
            if is_bar {
                pixels.extend_from_slice(&[255u8, 255, 255, 255]);
            } else {
                pixels.extend_from_slice(&[hue_r, hue_g, hue_b, 255]);
            }
        }
    }
    Arc::new(pixels)
}

// --- Entry point ---------------------------------------------------------

fn main() {
    const WIDTH: u32 = 1280;
    const HEIGHT: u32 = 720;

    FluentApp::new("AndroidConnect — VideoFrame Spike")
        .window_size(1280.0, 720.0 + 24.0)
        .run(|cx| {
            let state = cx.new(|_| FrameState::new());
            let view = cx.new(|cx| VideoView::new(state.clone(), WIDTH, HEIGHT, cx));

            let state_weak = state.downgrade();
            cx.spawn(async move |cx| {
                let mut frame_num = 0u64;
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(33))
                        .await;
                    frame_num += 1;
                    let frame = make_frame(frame_num, WIDTH, HEIGHT);
                    state_weak
                        .update(cx, |state, cx| state.push_frame(frame, cx))
                        .ok();
                }
            })
            .detach();

            view
        });
}
