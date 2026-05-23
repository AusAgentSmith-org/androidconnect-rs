pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct LetterboxLayout {
    pub fit_w: u32,
    pub fit_h: u32,
    pub x_off: u32,
    pub y_off: u32,
}

pub fn letterbox_layout(sw: u32, sh: u32, dw: u32, dh: u32) -> LetterboxLayout {
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

/// Scale video dimensions down so the short edge fits within `max_short_edge`, preserving ratio.
#[allow(dead_code)]
pub fn fit_window_to_video(vw: u32, vh: u32, max_short_edge: u32) -> (u32, u32) {
    let short = vw.min(vh);
    if short == 0 || short <= max_short_edge {
        return (vw.max(1), vh.max(1));
    }
    let scale = max_short_edge as f32 / short as f32;
    (
        ((vw as f32 * scale).round() as u32).max(1),
        ((vh as f32 * scale).round() as u32).max(1),
    )
}

/// Map a desktop-window pointer position to a frame-space pixel, accounting for
/// the letterboxed fit of `(frame_w, frame_h)` inside `(window_w, window_h)`.
pub fn map_window_to_frame(
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
}
