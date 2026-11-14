//! The crop editor: a preview of the photo with a frame to drag – by its corners and edges, moved as a whole,
//! or drawn anew – free or in a fixed shape (1:1, 4:3, 3:2, 16:9, turned for upright shapes).
//! The frame is kept as fractions of the upright photo, so it fits the photo itself whatever the preview size.

use crate::convert::{CropRect, Preview};
use crate::i18n::tr;
use crate::theme::Palette;
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ratio {
    Free,
    R1x1,
    R4x3,
    R3x2,
    R16x9,
}

impl Ratio {
    pub const ALL: [Ratio; 5] = [Ratio::Free, Ratio::R1x1, Ratio::R4x3, Ratio::R3x2, Ratio::R16x9];

    pub fn label(self) -> &'static str {
        match self {
            Ratio::Free => tr("Free", "Volně"),
            Ratio::R1x1 => "1:1",
            Ratio::R4x3 => "4:3",
            Ratio::R3x2 => "3:2",
            Ratio::R16x9 => "16:9",
        }
    }

    /// The long side divided by the short one; None = free.
    fn value(self) -> Option<f32> {
        match self {
            Ratio::Free => None,
            Ratio::R1x1 => Some(1.0),
            Ratio::R4x3 => Some(4.0 / 3.0),
            Ratio::R3x2 => Some(1.5),
            Ratio::R16x9 => Some(16.0 / 9.0),
        }
    }
}

#[derive(Clone, Copy)]
enum Drag {
    /// A corner (both ±1) or an edge (one of them 0); the opposite side stays.
    Handle { hx: i8, hy: i8 },
    Move { start: Pos2, from: Rect },
    /// A new frame drawn from this point.
    New { anchor: Pos2 },
}

pub struct Editor {
    /// The photo in the list being cropped.
    pub idx: usize,
    /// Which loading of a preview this editor waits for (an older one that arrives late is ignored).
    pub generation: u64,
    pub tex: Option<TextureHandle>,
    pub error: Option<String>,
    /// The size of the upright photo.
    pub full: (u32, u32),
    pub rect: CropRect,
    pub ratio: Ratio,
    /// The fixed shape stands upright (3:4 instead of 4:3).
    pub tall: bool,
    drag: Option<Drag>,
}

const FULL: CropRect = [0.0, 0.0, 1.0, 1.0];
/// The smallest frame, in points on the screen.
const MIN: f32 = 24.0;

impl Editor {
    pub fn new(idx: usize, generation: u64) -> Self {
        Editor { idx, generation, tex: None, error: None, full: (0, 0), rect: FULL, ratio: Ratio::Free, tall: false, drag: None }
    }

    pub fn loading(&self) -> bool {
        self.tex.is_none() && self.error.is_none()
    }

    /// The preview arrived: shown with the frame the photo had (or the whole photo).
    pub fn loaded(&mut self, ctx: &egui::Context, p: Preview, crop: Option<CropRect>) {
        let image = egui::ColorImage::from_rgba_unmultiplied([p.width as usize, p.height as usize], &p.rgba);
        self.tex = Some(ctx.load_texture("crop-preview", image, egui::TextureOptions::LINEAR));
        self.full = p.full;
        self.tall = p.full.1 > p.full.0;
        self.rect = crop.unwrap_or(FULL);
        self.ratio = Ratio::Free;
        self.drag = None;
    }

    /// The width / height of the fixed shape in pixels; None = free.
    fn shape(&self) -> Option<f32> {
        self.ratio.value().map(|v| if self.tall { 1.0 / v } else { v })
    }

    /// A new shape: the biggest frame of it around the middle of the present one.
    pub fn set_ratio(&mut self, ratio: Ratio) {
        self.ratio = ratio;
        let Some(r) = self.shape() else { return };
        let (w, h) = (self.full.0.max(1) as f32, self.full.1.max(1) as f32);
        let (mut pw, mut ph) = (w, w / r);
        if ph > h {
            ph = h;
            pw = h * r;
        }
        let (fw, fh) = (pw / w, ph / h);
        let (cx, cy) = (self.rect[0] + self.rect[2] / 2.0, self.rect[1] + self.rect[3] / 2.0);
        let x = (cx - fw / 2.0).clamp(0.0, 1.0 - fw);
        let y = (cy - fh / 2.0).clamp(0.0, 1.0 - fh);
        self.rect = [x, y, fw, fh];
    }

    /// The fixed shape turned (landscape ↔ portrait).
    pub fn turn(&mut self) {
        self.tall = !self.tall;
        if self.ratio != Ratio::Free {
            self.set_ratio(self.ratio);
        }
    }

    /// The whole photo again.
    pub fn reset(&mut self) {
        self.rect = FULL;
        self.ratio = Ratio::Free;
    }

    /// What the photo gets: None when the frame is (almost) the whole photo.
    pub fn result(&self) -> Option<CropRect> {
        let whole = self.rect[0] < 0.001 && self.rect[1] < 0.001 && self.rect[2] > 0.999 && self.rect[3] > 0.999;
        (!whole).then_some(self.rect)
    }

    /// The size of the cropped photo in pixels.
    pub fn pixels(&self) -> (u32, u32) {
        let (_, _, w, h) = crate::convert::crop_pixels(self.full.0.max(1), self.full.1.max(1), self.rect);
        (w, h)
    }

    /// The preview with the frame, in the whole space given.
    pub fn show(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let area = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);
        let Some(tex) = &self.tex else {
            if let Some(e) = &self.error {
                painter.text(area.center(), egui::Align2::CENTER_CENTER, e, crate::theme::body(14.0), p.critical);
            } else {
                egui::Spinner::new().size(28.0).color(p.accent_text).paint_at(ui, Rect::from_center_size(area.center(), vec2(28.0, 28.0)));
            }
            return;
        };
        let (w, h) = (self.full.0.max(1) as f32, self.full.1.max(1) as f32);
        let room = area.shrink(12.0);
        let s = (room.width() / w).min(room.height() / h);
        let img = Rect::from_center_size(room.center(), vec2(w * s, h * s));
        painter.image(tex.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

        let to_screen = |c: CropRect| Rect::from_min_size(img.min + vec2(c[0] * img.width(), c[1] * img.height()), vec2(c[2] * img.width(), c[3] * img.height()));
        let frame = to_screen(self.rect);
        let corners = self.ratio == Ratio::Free;

        // where the pointer is: a handle, inside the frame, or outside
        let hit = |pos: Pos2| -> Option<(i8, i8)> {
            let near = |a: f32, b: f32| (a - b).abs() <= 12.0;
            let hx = if near(pos.x, frame.left()) { -1 } else if near(pos.x, frame.right()) { 1 } else { 0 };
            let hy = if near(pos.y, frame.top()) { -1 } else if near(pos.y, frame.bottom()) { 1 } else { 0 };
            let inside_x = pos.x > frame.left() - 12.0 && pos.x < frame.right() + 12.0;
            let inside_y = pos.y > frame.top() - 12.0 && pos.y < frame.bottom() + 12.0;
            match (hx, hy) {
                (0, 0) => None,
                (x, y) if x != 0 && y != 0 => Some((x, y)),
                // an edge: only for a free frame, and only along the frame
                (x, 0) if corners && inside_y => Some((x, 0)),
                (0, y) if corners && inside_x => Some((0, y)),
                _ => None,
            }
        };
        if let Some(pos) = resp.hover_pos().filter(|_| self.drag.is_none()) {
            let icon = match hit(pos) {
                Some((-1, -1)) | Some((1, 1)) => CursorIcon::ResizeNwSe,
                Some((1, -1)) | Some((-1, 1)) => CursorIcon::ResizeNeSw,
                Some((_, 0)) => CursorIcon::ResizeHorizontal,
                Some((0, _)) => CursorIcon::ResizeVertical,
                _ if frame.contains(pos) => CursorIcon::Grab,
                _ => CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        if resp.drag_started()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            self.drag = Some(match hit(pos) {
                Some((hx, hy)) => Drag::Handle { hx, hy },
                None if frame.contains(pos) => Drag::Move { start: pos, from: frame },
                None => Drag::New { anchor: img.clamp(pos) },
            });
        }
        if let (Some(drag), Some(pos)) = (self.drag, resp.interact_pointer_pos())
            && resp.dragged()
        {
            let shape = self.shape();
            let next = match drag {
                Drag::Move { start, from } => {
                    let d = pos - start;
                    let dx = d.x.clamp(img.left() - from.left(), img.right() - from.right());
                    let dy = d.y.clamp(img.top() - from.top(), img.bottom() - from.bottom());
                    from.translate(vec2(dx, dy))
                }
                Drag::New { anchor } => from_anchor(anchor, pos, img, shape),
                Drag::Handle { hx, hy } if hx != 0 && hy != 0 => {
                    let anchor = pos2(if hx < 0 { frame.right() } else { frame.left() }, if hy < 0 { frame.bottom() } else { frame.top() });
                    from_anchor(anchor, pos, img, shape)
                }
                Drag::Handle { hx, hy } => {
                    let mut r = frame;
                    match (hx, hy) {
                        (-1, _) => r.min.x = pos.x.clamp(img.left(), r.max.x - MIN),
                        (1, _) => r.max.x = pos.x.clamp(r.min.x + MIN, img.right()),
                        (_, -1) => r.min.y = pos.y.clamp(img.top(), r.max.y - MIN),
                        _ => r.max.y = pos.y.clamp(r.min.y + MIN, img.bottom()),
                    }
                    r
                }
            };
            self.rect = [
                ((next.left() - img.left()) / img.width()).clamp(0.0, 1.0),
                ((next.top() - img.top()) / img.height()).clamp(0.0, 1.0),
                (next.width() / img.width()).clamp(0.0, 1.0),
                (next.height() / img.height()).clamp(0.0, 1.0),
            ];
            if matches!(drag, Drag::Move { .. }) {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }

        // the outside darkened, the frame, the thirds and the handles
        let frame = to_screen(self.rect);
        let shade = Color32::from_black_alpha(150);
        painter.rect_filled(Rect::from_min_max(img.min, pos2(img.right(), frame.top())), 0, shade);
        painter.rect_filled(Rect::from_min_max(pos2(img.left(), frame.bottom()), img.max), 0, shade);
        painter.rect_filled(Rect::from_min_max(pos2(img.left(), frame.top()), pos2(frame.left(), frame.bottom())), 0, shade);
        painter.rect_filled(Rect::from_min_max(pos2(frame.right(), frame.top()), pos2(img.right(), frame.bottom())), 0, shade);
        let thin = Stroke::new(1.0, Color32::from_white_alpha(90));
        for k in [1.0 / 3.0, 2.0 / 3.0] {
            painter.vline(frame.left() + frame.width() * k, frame.y_range(), thin);
            painter.hline(frame.x_range(), frame.top() + frame.height() * k, thin);
        }
        painter.rect_stroke(frame, 0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Middle);
        // corner brackets as in the Photos app of Windows
        let (len, thick) = (18.0_f32.min(frame.width() / 2.0).min(frame.height() / 2.0), 4.0);
        let bracket = Stroke::new(thick, Color32::WHITE);
        // drawn just inside the frame, so the edge of the photo never cuts them
        let inner = frame.shrink(thick / 2.0);
        for (c, dx, dy) in [
            (inner.left_top(), 1.0, 1.0),
            (inner.right_top(), -1.0, 1.0),
            (inner.left_bottom(), 1.0, -1.0),
            (inner.right_bottom(), -1.0, -1.0),
        ] {
            painter.line_segment([c - vec2(dx * thick / 2.0, 0.0), c + vec2(dx * len, 0.0)], bracket);
            painter.line_segment([c, c + vec2(0.0, dy * len)], bracket);
        }
        if corners {
            for c in [inner.center_top(), inner.center_bottom()] {
                painter.line_segment([c - vec2(9.0, 0.0), c + vec2(9.0, 0.0)], bracket);
            }
            for c in [inner.left_center(), inner.right_center()] {
                painter.line_segment([c - vec2(0.0, 9.0), c + vec2(0.0, 9.0)], bracket);
            }
        }
    }
}

/// A frame from a fixed corner to the pointer, inside the picture, of the shape when one is set.
fn from_anchor(anchor: Pos2, pointer: Pos2, img: Rect, shape: Option<f32>) -> Rect {
    let sx = if pointer.x >= anchor.x { 1.0 } else { -1.0 };
    let sy = if pointer.y >= anchor.y { 1.0 } else { -1.0 };
    let max_w = if sx > 0.0 { img.right() - anchor.x } else { anchor.x - img.left() }.max(1.0);
    let max_h = if sy > 0.0 { img.bottom() - anchor.y } else { anchor.y - img.top() }.max(1.0);
    let mut w = (pointer.x - anchor.x).abs().clamp(MIN.min(max_w), max_w);
    let mut h = (pointer.y - anchor.y).abs().clamp(MIN.min(max_h), max_h);
    if let Some(r) = shape {
        if w / h > r {
            h = w / r;
        } else {
            w = h * r;
        }
        if w > max_w {
            w = max_w;
            h = w / r;
        }
        if h > max_h {
            h = max_h;
            w = h * r;
        }
    }
    let x = if sx > 0.0 { anchor.x } else { anchor.x - w };
    let y = if sy > 0.0 { anchor.y } else { anchor.y - h };
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}
