//! The caret gliding to where it moves. Each of its four corners follows a critically damped spring of its
//! own; the corners facing the way it moves arrive almost at once and the rest lag behind, so a long jump
//! stretches into a trail that closes up. The motion model follows the Neovide cursor (MIT).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A frame longer than this (a stall, a background window) advances the springs by this much only.
const LONGEST_FRAME: Duration = Duration::from_millis(33);
const AT_REST: f32 = 0.001;
const STILL_MOVING: f32 = 0.01;
const CORNER_MOVING: f32 = 0.5;
const SAME_PLACE: f32 = 0.01;
/// A move along the line of at most this many caret widths counts as short.
const SHORT_MOVE: f32 = 8.0;
/// Corners facing the move at least this much take the leading time.
const LEADING: f32 = 0.5;
const LEADING_SECONDS: f32 = 0.02;
/// A corner slower than this starts its spring over instead of carrying its speed into the new move.
const RESTART_SECONDS: f32 = 0.075;
/// How far a corner may lag, in caret sizes.
const LONGEST_TRAIL: f32 = 100.0;
/// The trailing corners' times, by how little each faces the move (least first).
const TRAIL_BY_RANK: [f32; 4] = [1.0, 0.9, 0.5, 0.3];
const GLIDE_SECONDS: f32 = 0.125;
const SHORT_GLIDE_SECONDS: f32 = 0.05;
const TRAIL_SIZE: f32 = 1.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Point {
    x: f32,
    y: f32,
}

impl Point {
    fn unit(self) -> Point {
        let length = self.x.hypot(self.y);
        if length == 0.0 || !length.is_finite() {
            return Point::default();
        }
        Point {
            x: self.x / length,
            y: self.y / length,
        }
    }

    fn dot(self, other: Point) -> f32 {
        self.x * other.x + self.y * other.y
    }
}

/// Where the caret is drawn, in window px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CaretBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl CaretBox {
    fn usable(self) -> bool {
        [self.x, self.y, self.w, self.h]
            .iter()
            .all(|v| v.is_finite())
            && self.w > 0.0
            && self.h > 0.0
    }

    fn center(self) -> Point {
        Point {
            x: self.x + self.w / 2.0,
            y: self.y + self.h / 2.0,
        }
    }

    fn same_size(self, other: CaretBox) -> bool {
        near(self.w, other.w) && near(self.h, other.h)
    }

    fn same_place(self, other: CaretBox) -> bool {
        near(self.x, other.x) && near(self.y, other.y)
    }
}

/// What the text is scrolled and laid out at; any change moves every caret with the text, so they jump.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextView {
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub origin_x: f32,
    pub origin_y: f32,
    pub line_h: f32,
    pub column_w: f32,
}

/// One axis of a corner: how far it still is from where it goes, and how fast that closes.
#[derive(Clone, Copy, Debug, Default)]
struct Spring {
    gap: f32,
    speed: f32,
}

impl Spring {
    /// Advances `dt` seconds of a spring that settles in about `length`; answers whether it still moves.
    fn advance(&mut self, dt: f32, length: f32) -> bool {
        if !dt.is_finite()
            || dt < 0.0
            || !length.is_finite()
            || length <= dt
            || self.gap.abs() < AT_REST
        {
            *self = Spring::default();
            return false;
        }
        if dt == 0.0 {
            return self.gap.abs() >= STILL_MOVING;
        }
        let omega = 4.0 / length;
        let (gap, carried) = (self.gap, self.gap * omega + self.speed);
        let decay = (-omega * dt).exp();
        self.gap = (gap + carried * dt) * decay;
        self.speed = decay * (carried - gap * omega - carried * dt * omega);
        if !self.gap.is_finite() || !self.speed.is_finite() || self.gap.abs() < AT_REST {
            *self = Spring::default();
            return false;
        }
        self.gap.abs() >= STILL_MOVING
    }
}

#[derive(Clone, Copy, Debug)]
struct Corner {
    /// Which corner, as a fraction of the caret's size from its center (`-0.5, -0.5` is the top left).
    side: Point,
    at: Point,
    goal: Point,
    x: Spring,
    y: Spring,
    length: f32,
}

impl Corner {
    fn new(side: Point) -> Corner {
        Corner {
            side,
            at: Point::default(),
            goal: Point::default(),
            x: Spring::default(),
            y: Spring::default(),
            length: 0.0,
        }
    }

    fn place_in(self, caret: CaretBox) -> Point {
        let center = caret.center();
        Point {
            x: center.x + self.side.x * caret.w,
            y: center.y + self.side.y * caret.h,
        }
    }

    /// How much this corner faces the way it is about to travel (1: straight ahead).
    fn facing(self, caret: CaretBox) -> f32 {
        let place = self.place_in(caret);
        Point {
            x: place.x - self.at.x,
            y: place.y - self.at.y,
        }
        .unit()
        .dot(self.side.unit())
    }

    fn jump(&mut self, caret: CaretBox) {
        let place = self.place_in(caret);
        self.at = place;
        self.goal = place;
        self.x = Spring::default();
        self.y = Spring::default();
    }

    fn aim(&mut self, caret: CaretBox, rank: usize) {
        let place = self.place_in(caret);
        let across = (place.x - self.goal.x) / caret.w.max(f32::EPSILON);
        let down = (place.y - self.goal.y) / caret.h.max(f32::EPSILON);
        let leading = Point { x: across, y: down }.unit().dot(self.side.unit());
        let short = across.abs() <= SHORT_MOVE && down.abs() <= AT_REST;
        let base = if short {
            GLIDE_SECONDS.min(SHORT_GLIDE_SECONDS)
        } else {
            GLIDE_SECONDS
        };
        let own = if leading > LEADING {
            LEADING_SECONDS
        } else {
            base * TRAIL_BY_RANK[rank.min(3)]
        };
        self.length = base + (own - base) * TRAIL_SIZE;
        if self.length > RESTART_SECONDS {
            self.x = Spring::default();
            self.y = Spring::default();
        }
        self.goal = place;
        self.x.gap = place.x - self.at.x;
        self.y.gap = place.y - self.at.y;
    }

    fn advance(&mut self, dt: f32, longest: f32) -> bool {
        self.x.advance(dt, self.length);
        self.y.advance(dt, self.length);
        self.x.gap = self.x.gap.clamp(-longest, longest);
        self.y.gap = self.y.gap.clamp(-longest, longest);
        self.at = Point {
            x: self.goal.x - self.x.gap,
            y: self.goal.y - self.y.gap,
        };
        self.x.gap.abs() > CORNER_MOVING || self.y.gap.abs() > CORNER_MOVING
    }
}

/// One caret's glide.
#[derive(Clone)]
pub(crate) struct Glide {
    corners: [Corner; 4],
    goal: Option<CaretBox>,
    /// The caret's place in the text (row, offset): moving it glides, reflowing the text under it jumps.
    spot: Option<(usize, usize)>,
    view: Option<TextView>,
    last_frame: Option<Instant>,
    moving: bool,
}

impl Default for Glide {
    fn default() -> Glide {
        let sides = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)];
        Glide {
            corners: sides.map(|(x, y)| Corner::new(Point { x, y })),
            goal: None,
            spot: None,
            view: None,
            last_frame: None,
            moving: false,
        }
    }
}

impl Glide {
    /// The caret's corners this frame (top left, top right, bottom right, bottom left) while it glides; `None`
    /// once it sits at `caret`, which is then drawn as usual.
    pub(crate) fn frame(
        &mut self,
        spot: (usize, usize),
        caret: CaretBox,
        view: TextView,
        now: Instant,
    ) -> Option<[(f32, f32); 4]> {
        if !caret.usable() || !view_usable(view) {
            *self = Glide::default();
            return None;
        }
        let Some(before) = self.goal else {
            self.settle(spot, caret, view, now);
            return None;
        };
        let spot_moved = self.spot != Some(spot);
        let place_moved = !before.same_place(caret);
        if self.view != Some(view) || !before.same_size(caret) || (!spot_moved && place_moved) {
            self.settle(spot, caret, view, now);
            return None;
        }
        self.spot = Some(spot);
        self.view = Some(view);
        self.goal = Some(caret);
        if place_moved {
            let dt = if self.moving {
                self.since_last_frame(now)
            } else {
                Duration::ZERO
            };
            self.aim(caret);
            self.advance(dt);
            self.last_frame = Some(now);
        } else if self.moving {
            let dt = self.since_last_frame(now);
            self.advance(dt);
            self.last_frame = Some(now);
        }
        self.moving
            .then(|| self.corners.map(|corner| (corner.at.x, corner.at.y)))
    }

    fn settle(&mut self, spot: (usize, usize), caret: CaretBox, view: TextView, now: Instant) {
        for corner in &mut self.corners {
            corner.jump(caret);
        }
        self.goal = Some(caret);
        self.spot = Some(spot);
        self.view = Some(view);
        self.last_frame = Some(now);
        self.moving = false;
    }

    fn aim(&mut self, caret: CaretBox) {
        let mut facing: [(usize, f32); 4] =
            std::array::from_fn(|index| (index, self.corners[index].facing(caret)));
        facing.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut ranks = [0; 4];
        for (rank, (index, _)) in facing.into_iter().enumerate() {
            ranks[index] = rank;
        }
        for (index, corner) in self.corners.iter_mut().enumerate() {
            corner.aim(caret, ranks[index]);
        }
        self.moving = self
            .corners
            .iter()
            .any(|corner| corner.x.gap != 0.0 || corner.y.gap != 0.0);
    }

    fn since_last_frame(&self, now: Instant) -> Duration {
        self.last_frame
            .and_then(|last| now.checked_duration_since(last))
            .unwrap_or_default()
            .min(LONGEST_FRAME)
    }

    fn advance(&mut self, dt: Duration) {
        let dt = dt.as_secs_f32();
        let longest = self
            .goal
            .map_or(0.0, |caret| caret.w.max(caret.h) * LONGEST_TRAIL);
        let mut moving = false;
        for corner in &mut self.corners {
            moving |= corner.advance(dt, longest);
        }
        self.moving = moving;
        if !self.moving {
            if let Some(caret) = self.goal {
                for corner in &mut self.corners {
                    corner.jump(caret);
                }
            }
        }
    }
}

/// Every caret's glide in one editor, by selection id.
#[derive(Default)]
pub(crate) struct Glides {
    by_selection: HashMap<usize, Glide>,
    newest: Option<usize>,
    /// The newest caret's glide as last drawn, kept apart so a click (a fresh selection) glides from there.
    newest_before: Option<Glide>,
}

impl Glides {
    /// A new newest selection (a click, a new caret) starts from the previous newest caret's glide.
    pub(crate) fn newest_is(&mut self, id: usize) {
        if self.newest != Some(id) && !self.by_selection.contains_key(&id) {
            let inherited = self
                .newest
                .and_then(|newest| self.by_selection.get(&newest).cloned())
                .or_else(|| self.newest_before.clone());
            if let Some(glide) = inherited {
                self.by_selection.insert(id, glide);
            }
        }
        self.newest = Some(id);
    }

    pub(crate) fn frame(
        &mut self,
        id: usize,
        spot: (usize, usize),
        caret: CaretBox,
        view: TextView,
        now: Instant,
    ) -> Option<[(f32, f32); 4]> {
        self.by_selection
            .entry(id)
            .or_default()
            .frame(spot, caret, view, now)
    }

    /// After a frame: remembers the newest caret's glide and forgets the carets no longer drawn.
    pub(crate) fn keep_only(&mut self, drawn: &[usize]) {
        if let Some(glide) = self.newest.and_then(|id| self.by_selection.get(&id)) {
            self.newest_before = Some(glide.clone());
        }
        self.by_selection.retain(|id, _| drawn.contains(id));
    }

    pub(crate) fn clear(&mut self) {
        *self = Glides::default();
    }
}

fn view_usable(view: TextView) -> bool {
    [
        view.scroll_x,
        view.scroll_y,
        view.origin_x,
        view.origin_y,
        view.line_h,
        view.column_w,
    ]
    .iter()
    .all(|v| v.is_finite())
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= SAME_PLACE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret(x: f32, y: f32) -> CaretBox {
        CaretBox {
            x,
            y,
            w: 2.0,
            h: 20.0,
        }
    }

    fn view(scroll_y: f32) -> TextView {
        TextView {
            scroll_x: 0.0,
            scroll_y,
            origin_x: 0.0,
            origin_y: 0.0,
            line_h: 20.0,
            column_w: 8.0,
        }
    }

    #[test]
    fn a_spring_comes_to_rest() {
        let mut spring = Spring {
            gap: 100.0,
            speed: 0.0,
        };
        for _ in 0..60 {
            spring.advance(1.0 / 120.0, 0.1);
        }
        assert_eq!((spring.gap, spring.speed), (0.0, 0.0));
    }

    #[test]
    fn a_move_glides_and_settles_where_it_went() {
        let start = Instant::now();
        let mut glide = Glide::default();
        assert!(glide
            .frame((1, 1), caret(10.0, 20.0), view(0.0), start)
            .is_none());
        let first = glide
            .frame((4, 40), caret(200.0, 80.0), view(0.0), start)
            .expect("gliding");
        assert_eq!(first[0], (10.0, 20.0), "nothing has moved yet");
        let mut now = start;
        let mut frames = 0;
        while glide
            .frame((4, 40), caret(200.0, 80.0), view(0.0), now)
            .is_some()
        {
            now += Duration::from_millis(8);
            frames += 1;
            assert!(frames < 200, "never settles");
        }
        assert!(frames > 3, "it glided for {frames} frames");
        assert_eq!(glide.corners[0].at, Point { x: 200.0, y: 80.0 });
    }

    #[test]
    fn scrolling_or_reflowing_text_jumps_the_caret() {
        let now = Instant::now();
        let mut glide = Glide::default();
        glide.frame((1, 1), caret(10.0, 20.0), view(0.0), now);
        assert!(glide
            .frame((1, 1), caret(10.0, 60.0), view(2.0), now)
            .is_none());
        assert!(
            glide
                .frame((1, 1), caret(30.0, 60.0), view(2.0), now)
                .is_none(),
            "the text moved under a caret that stayed put"
        );
    }

    #[test]
    fn a_click_glides_from_the_caret_it_replaces() {
        let now = Instant::now();
        let mut glides = Glides::default();
        glides.newest_is(1);
        glides.frame(1, (0, 0), caret(10.0, 20.0), view(0.0), now);
        glides.keep_only(&[1]);
        glides.newest_is(2);
        assert!(glides
            .frame(2, (5, 90), caret(300.0, 120.0), view(0.0), now)
            .is_some());
    }
}
