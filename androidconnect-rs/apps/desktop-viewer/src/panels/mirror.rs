use eframe::egui;
use egui::{Image, Rect, RichText, Sense, TextureHandle, Vec2};

pub struct DrawInput<'a> {
    pub texture: Option<&'a TextureHandle>,
    pub frame_w: u32,
    pub frame_h: u32,
}

/// Draws the streaming canvas inside the central panel. Returns the rect the image
/// landed in so the app can map pointer events back into frame coordinates.
pub fn draw(ui: &mut egui::Ui, input: DrawInput<'_>) -> Option<Rect> {
    let Some(handle) = input.texture else {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(RichText::new("No video yet").heading());
            ui.label("Mirror starts as soon as your phone shares its screen.");
        });
        return None;
    };

    let available = ui.available_size();
    let aspect = if input.frame_w == 0 || input.frame_h == 0 {
        9.0 / 16.0
    } else {
        input.frame_w as f32 / input.frame_h as f32
    };
    let mut w = available.x;
    let mut h = w / aspect;
    if h > available.y {
        h = available.y;
        w = h * aspect;
    }
    let (rect, _resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click_and_drag());
    Image::from_texture((handle.id(), rect.size()))
        .fit_to_exact_size(rect.size())
        .paint_at(ui, rect);
    Some(rect)
}
