//! Drawing on a paused frame: arrows and circles for "I should have been
//! here". Strokes are stored in 0..1 coordinates of the picture, so they
//! line up at any window size, and saved with the next note.

use gpui_kit::*;

use crate::store::Stroke;
use crate::theme;

pub const COLORS: usize = 3;

pub fn color(i: u8) -> Hsla {
    match i {
        1 => theme::death(),
        2 => theme::kill(),
        _ => theme::accent(),
    }
}

/// Draw mode state: the strokes so far and the pen colour.
#[derive(Default)]
pub struct Sketch {
    pub strokes: Vec<Stroke>,
    pub color: u8,
    pub pen_down: bool,
}

/// Where the picture sits inside `bounds` (it's letterboxed to fit).
pub fn picture_rect(bounds: Bounds<Pixels>, frame: Option<(f32, f32)>) -> Bounds<Pixels> {
    let (fw, fh) = frame.unwrap_or((16.0, 9.0));
    let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if bw <= 0.0 || bh <= 0.0 {
        return bounds;
    }
    let scale = (bw / fw).min(bh / fh);
    let (w, h) = (fw * scale, fh * scale);
    Bounds::new(
        point(bounds.origin.x + px((bw - w) / 2.0), bounds.origin.y + px((bh - h) / 2.0)),
        size(px(w), px(h)),
    )
}

/// Window position → 0..1 picture coordinates.
pub fn normalize(at: Point<Pixels>, pic: Bounds<Pixels>) -> [f32; 2] {
    let w = f32::from(pic.size.width).max(1.0);
    let h = f32::from(pic.size.height).max(1.0);
    [
        (f32::from(at.x - pic.origin.x) / w).clamp(0.0, 1.0),
        (f32::from(at.y - pic.origin.y) / h).clamp(0.0, 1.0),
    ]
}

pub fn paint(strokes: &[Stroke], pic: Bounds<Pixels>, window: &mut Window) {
    let width = (f32::from(pic.size.height) / 220.0).clamp(2.5, 6.0);
    let at = |p: [f32; 2]| point(pic.origin.x + pic.size.width * p[0], pic.origin.y + pic.size.height * p[1]);
    for s in strokes {
        let Some(first) = s.points.first() else { continue };
        let mut b = PathBuilder::stroke(px(width));
        b.move_to(at(*first));
        if s.points.len() == 1 {
            // A tap: a small dot.
            b.line_to(at([first[0] + 0.001, first[1] + 0.001]));
        }
        for p in &s.points[1..] {
            b.line_to(at(*p));
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color(s.color));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn maps_points_inside_the_letterboxed_picture() {
        // A 16:9 picture in a square box sits in the middle band.
        let b = Bounds::new(point(px(0.), px(0.)), size(px(160.), px(160.)));
        let pic = picture_rect(b, Some((1920.0, 1080.0)));
        assert_eq!(pic.size.height, px(90.));
        assert_eq!(pic.origin.y, px(35.));
        assert_eq!(normalize(point(px(80.), px(80.)), pic), [0.5, 0.5]);
        assert_eq!(normalize(point(px(-5.), px(0.)), pic), [0.0, 0.0]);
    }
}
