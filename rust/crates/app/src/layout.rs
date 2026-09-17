//! Workspace layout: a self-managed top bar, a left dock (the workspace-list panel — our addition over Zed) and a
//! right dock, both resizable via a divider and collapsing to a fixed icon rail instead of vanishing, plus the
//! content area. Computes rectangles, text runs and hit regions; the app drives input and rendering.

use crate::ui::{Rect, Text};

pub const TOP_BAR_H: f32 = 38.0;
pub const RAIL_W: f32 = 48.0; // collapsed dock width — the icon rail; the dock never goes narrower than this
pub const DOCK_MIN: f32 = 180.0; // narrowest expanded width
pub const DOCK_MAX: f32 = 520.0;
pub const DIVIDER_HIT: f32 = 5.0;
pub const TRAFFIC_INSET: f32 = 82.0; // left space reserved for the macOS traffic lights

const TOP_BAR: [u8; 3] = [33, 37, 43];
const RAIL: [u8; 3] = [22, 24, 30];
const DOCK: [u8; 3] = [30, 33, 40];
const DIVIDER: [u8; 3] = [70, 75, 87];
const ICON: [u8; 3] = [58, 63, 74];
const TEXT: [u8; 3] = [205, 211, 222];
const TEXT_DIM: [u8; 3] = [140, 147, 160];
const CONTENT_A: [u8; 3] = [200, 70, 70];
const CONTENT_B: [u8; 3] = [210, 180, 70];
const CONTENT_C: [u8; 3] = [70, 110, 200];

pub struct Dock {
    pub width: f32,
    pub collapsed: bool,
}

impl Dock {
    fn effective(&self) -> f32 {
        if self.collapsed {
            RAIL_W
        } else {
            self.width.clamp(DOCK_MIN, DOCK_MAX)
        }
    }
}

pub struct Layout {
    pub left: Dock,
    pub right: Dock,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            left: Dock { width: 260.0, collapsed: false },
            right: Dock { width: 300.0, collapsed: true },
        }
    }
}

impl Layout {
    pub fn left_w(&self) -> f32 {
        self.left.effective()
    }
    pub fn right_w(&self) -> f32 {
        self.right.effective()
    }

    /// Drag the left divider to width `x`; below the expanded minimum it snaps to the rail (never fully hidden).
    pub fn set_left_divider(&mut self, x: f32) {
        if x < DOCK_MIN - 20.0 {
            self.left.collapsed = true;
        } else {
            self.left.collapsed = false;
            self.left.width = x.clamp(DOCK_MIN, DOCK_MAX);
        }
    }

    /// Drag the right divider; `w` is the window width, `x` the pointer x. Right dock width = w - x.
    pub fn set_right_divider(&mut self, x: f32, w: f32) {
        let rw = w - x;
        if rw < DOCK_MIN - 20.0 {
            self.right.collapsed = true;
        } else {
            self.right.collapsed = false;
            self.right.width = rw.clamp(DOCK_MIN, DOCK_MAX);
        }
    }

    pub fn on_left_divider(&self, x: f32, y: f32) -> bool {
        y > TOP_BAR_H && (x - self.left_w()).abs() <= DIVIDER_HIT
    }
    pub fn on_right_divider(&self, x: f32, y: f32, w: f32) -> bool {
        y > TOP_BAR_H && (x - (w - self.right_w())).abs() <= DIVIDER_HIT
    }

    fn left_toggle(&self) -> (f32, f32, f32, f32) {
        (10.0, TOP_BAR_H + 8.0, 26.0, 24.0)
    }
    fn right_toggle(&self, w: f32) -> (f32, f32, f32, f32) {
        (w - RAIL_W + 12.0, TOP_BAR_H + 8.0, 26.0, 24.0)
    }

    pub fn hit_left_toggle(&self, x: f32, y: f32) -> bool {
        hit(self.left_toggle(), x, y)
    }
    pub fn hit_right_toggle(&self, x: f32, y: f32, w: f32) -> bool {
        hit(self.right_toggle(w), x, y)
    }

    pub fn build(&self, w: f32, h: f32) -> (Vec<Rect>, Vec<Text>) {
        let lw = self.left_w();
        let rw = self.right_w();
        let by = TOP_BAR_H;
        let bh = (h - TOP_BAR_H).max(0.0);
        let mut rects = Vec::new();
        let mut texts = Vec::new();

        // top bar
        rects.push(Rect { x: 0.0, y: 0.0, w, h: TOP_BAR_H, color: TOP_BAR });
        texts.push(Text { x: TRAFFIC_INSET, y: 11.0, size: 13.0, color: TEXT, text: "boom".into() });
        texts.push(Text { x: w / 2.0 - 34.0, y: 11.0, size: 13.0, color: TEXT_DIM, text: "main  ·  8".into() });

        // left dock
        rects.push(Rect { x: 0.0, y: by, w: lw, h: bh, color: if self.left.collapsed { RAIL } else { DOCK } });
        // right dock
        rects.push(Rect { x: w - rw, y: by, w: rw, h: bh, color: if self.right.collapsed { RAIL } else { DOCK } });

        // content: three colored placeholder columns
        let cx = lw;
        let cw = (w - lw - rw).max(0.0);
        let col = cw / 3.0;
        rects.push(Rect { x: cx, y: by, w: col, h: bh, color: CONTENT_A });
        rects.push(Rect { x: cx + col, y: by, w: col, h: bh, color: CONTENT_B });
        rects.push(Rect { x: cx + 2.0 * col, y: by, w: cw - 2.0 * col, h: bh, color: CONTENT_C });

        // dividers
        rects.push(Rect { x: lw - 1.0, y: by, w: 1.0, h: bh, color: DIVIDER });
        rects.push(Rect { x: w - rw, y: by, w: 1.0, h: bh, color: DIVIDER });

        // dock headers + toggles
        push_toggle(&mut rects, self.left_toggle());
        push_toggle(&mut rects, self.right_toggle(w));
        if !self.left.collapsed {
            texts.push(Text { x: 46.0, y: by + 12.0, size: 12.0, color: TEXT_DIM, text: "WORKSPACES".into() });
        }
        // left icon rail buttons
        for i in 0..3 {
            let y = by + 44.0 + i as f32 * 40.0;
            rects.push(Rect { x: 10.0, y, w: 28.0, h: 28.0, color: ICON });
        }

        (rects, texts)
    }
}

fn hit((tx, ty, tw, th): (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= tx && x <= tx + tw && y >= ty && y <= ty + th
}

fn push_toggle(rects: &mut Vec<Rect>, (x, y, w, h): (f32, f32, f32, f32)) {
    rects.push(Rect { x, y, w, h, color: ICON });
}
