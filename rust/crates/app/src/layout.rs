//! Workspace layout: a top bar, a left dock (the workspace-list panel — our addition over Zed), and the content
//! area. The dock is resizable and, instead of collapsing to nothing, shrinks to a fixed icon rail (never fully
//! hidden). This module computes the rectangles and hit regions; the app drives input and rendering.

use crate::ui::Rect;

pub const TOP_BAR_H: f32 = 40.0;
pub const RAIL_W: f32 = 48.0; // collapsed dock width — the icon rail; the dock never goes narrower than this
pub const DOCK_MIN: f32 = 180.0; // narrowest expanded width
pub const DOCK_MAX: f32 = 480.0;
pub const DIVIDER_HIT: f32 = 5.0; // half-width of the draggable divider hit region

// Placeholder palette (theme-driven later).
const TOP_BAR: [u8; 3] = [33, 37, 43];
const RAIL: [u8; 3] = [22, 24, 30];
const DOCK: [u8; 3] = [30, 33, 40];
const DIVIDER: [u8; 3] = [70, 75, 87];
const ICON: [u8; 3] = [58, 63, 74];
const CONTENT_A: [u8; 3] = [200, 70, 70]; // red / yellow / blue placeholders for the content region
const CONTENT_B: [u8; 3] = [210, 180, 70];
const CONTENT_C: [u8; 3] = [70, 110, 200];

pub struct Layout {
    pub dock_w: f32, // expanded width
    pub collapsed: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self { dock_w: 260.0, collapsed: false }
    }
}

impl Layout {
    pub fn dock_effective(&self) -> f32 {
        if self.collapsed {
            RAIL_W
        } else {
            self.dock_w.clamp(DOCK_MIN, DOCK_MAX)
        }
    }

    /// Drag the divider to width `x`; below the expanded minimum it snaps to the collapsed rail (never fully hidden).
    pub fn set_divider(&mut self, x: f32) {
        if x < DOCK_MIN - 20.0 {
            self.collapsed = true;
        } else {
            self.collapsed = false;
            self.dock_w = x.clamp(DOCK_MIN, DOCK_MAX);
        }
    }

    pub fn toggle(&mut self) {
        self.collapsed = !self.collapsed;
    }

    /// The collapse/expand toggle button region (top-left of the dock header).
    pub fn toggle_rect(&self) -> (f32, f32, f32, f32) {
        (10.0, TOP_BAR_H + 8.0, 26.0, 24.0)
    }

    /// Is `x` over the divider between the dock and the content?
    pub fn on_divider(&self, x: f32, y: f32) -> bool {
        y > TOP_BAR_H && (x - self.dock_effective()).abs() <= DIVIDER_HIT
    }

    pub fn hit_toggle(&self, x: f32, y: f32) -> bool {
        let (tx, ty, tw, th) = self.toggle_rect();
        x >= tx && x <= tx + tw && y >= ty && y <= ty + th
    }

    pub fn rects(&self, w: f32, h: f32) -> Vec<Rect> {
        let dock = self.dock_effective();
        let body_y = TOP_BAR_H;
        let body_h = (h - TOP_BAR_H).max(0.0);
        let mut r = vec![
            // top bar
            Rect { x: 0.0, y: 0.0, w, h: TOP_BAR_H, color: TOP_BAR },
            // dock (rail vs full panel)
            Rect { x: 0.0, y: body_y, w: dock, h: body_h, color: if self.collapsed { RAIL } else { DOCK } },
        ];

        // content region: three colored placeholder columns (red / yellow / blue)
        let cx = dock;
        let cw = (w - dock).max(0.0);
        let col = cw / 3.0;
        r.push(Rect { x: cx, y: body_y, w: col, h: body_h, color: CONTENT_A });
        r.push(Rect { x: cx + col, y: body_y, w: col, h: body_h, color: CONTENT_B });
        r.push(Rect { x: cx + 2.0 * col, y: body_y, w: cw - 2.0 * col, h: body_h, color: CONTENT_C });

        // divider line
        r.push(Rect { x: dock - 1.0, y: body_y, w: 1.0, h: body_h, color: DIVIDER });

        // dock header: collapse toggle + a couple of placeholder icons (icon rail buttons)
        let (tx, ty, tw, th) = self.toggle_rect();
        r.push(Rect { x: tx, y: ty, w: tw, h: th, color: ICON });
        // icon-rail buttons down the left (always visible so the collapsed rail has affordances)
        for i in 0..3 {
            let y = TOP_BAR_H + 44.0 + i as f32 * 40.0;
            r.push(Rect { x: 10.0, y, w: 28.0, h: 28.0, color: ICON });
        }
        r
    }
}
