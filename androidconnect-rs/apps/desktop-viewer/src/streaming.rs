use androidconnect_protocol::{MediaControlAction, Modifiers, PointerButton, SystemAction};
use winit::event::{MouseButton as WinitMouseButton, MouseScrollDelta};
use winit::keyboard::{Key, NamedKey};

pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Nearest-neighbour scale `src` (sw×sh RGBA) into `dst` (dw×dh RGBA).
/// Preserves aspect ratio; unfilled margins are black.
pub fn letterbox_scale(src: &[u8], sw: u32, sh: u32, dst: &mut [u8], dw: u32, dh: u32) {
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

pub fn pointer_button(button: WinitMouseButton) -> Option<PointerButton> {
    match button {
        WinitMouseButton::Left => Some(PointerButton::Left),
        WinitMouseButton::Middle => Some(PointerButton::Middle),
        WinitMouseButton::Right => Some(PointerButton::Right),
        WinitMouseButton::Back => Some(PointerButton::Back),
        WinitMouseButton::Forward => Some(PointerButton::Forward),
        WinitMouseButton::Other(_) => None,
    }
}

pub fn scroll_delta(delta: MouseScrollDelta) -> (i32, i32) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => {
            ((x * 120.0).round() as i32, (y * 120.0).round() as i32)
        }
        MouseScrollDelta::PixelDelta(pos) => (pos.x.round() as i32, pos.y.round() as i32),
    }
}

pub fn system_action_for_key(key: &Key, modifiers: Modifiers) -> Option<SystemAction> {
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

pub fn media_control_for_key(key: &Key) -> Option<MediaControlAction> {
    match key {
        Key::Named(NamedKey::MediaPlay) => Some(MediaControlAction::Play),
        Key::Named(NamedKey::MediaPause) => Some(MediaControlAction::Pause),
        Key::Named(NamedKey::MediaPlayPause) => Some(MediaControlAction::PlayPause),
        Key::Named(NamedKey::MediaStop) => Some(MediaControlAction::Stop),
        Key::Named(NamedKey::MediaTrackNext) => Some(MediaControlAction::Next),
        Key::Named(NamedKey::MediaTrackPrevious) => Some(MediaControlAction::Previous),
        _ => None,
    }
}

pub fn text_from_key(event: &winit::event::KeyEvent) -> Option<String> {
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
