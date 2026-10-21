use std::time::{Duration, Instant};

use winit::event::TouchPhase;

/// Wheel events further apart than this start a new gesture, for platforms that only report `Moved`.
const GESTURE_SEPARATION: Duration = Duration::from_millis(28);
/// The other axis takes over only once it moves this many times as far as the locked one...
const UNLOCK_RATIO: f32 = 1.9;
/// ...and by at least this many pixels in one event.
const UNLOCK_MIN: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// Keeps a touchpad gesture on the axis it started along, so a mostly vertical swipe doesn't drift sideways.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollGesture {
    last_event: Option<Instant>,
    axis: Option<Axis>,
}

impl ScrollGesture {
    /// The pixel delta `(dx, dy)` with the gesture's off-axis motion dropped.
    pub fn filter(&mut self, delta: (f32, f32), phase: TouchPhase) -> (f32, f32) {
        self.filter_at(delta, phase, Instant::now())
    }

    fn filter_at(&mut self, (dx, dy): (f32, f32), phase: TouchPhase, now: Instant) -> (f32, f32) {
        if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            *self = ScrollGesture::default();
            return (dx, dy);
        }
        let (x, y) = (dx.abs(), dy.abs());
        if x == 0.0 && y == 0.0 {
            if phase == TouchPhase::Started {
                *self = ScrollGesture::default();
            }
            return (dx, dy);
        }
        let new_gesture = phase == TouchPhase::Started
            || self
                .last_event
                .is_none_or(|last| now.duration_since(last) >= GESTURE_SEPARATION);
        if new_gesture {
            self.axis = Some(if x <= y {
                Axis::Vertical
            } else {
                Axis::Horizontal
            });
        } else if x.max(y) >= UNLOCK_MIN {
            match self.axis {
                Some(Axis::Vertical) if x > y && x >= y * UNLOCK_RATIO => self.axis = None,
                Some(Axis::Horizontal) if y > x && y >= x * UNLOCK_RATIO => self.axis = None,
                _ => {}
            }
        }
        self.last_event = Some(now);
        match self.axis {
            Some(Axis::Vertical) => (0.0, dy),
            Some(Axis::Horizontal) => (dx, 0.0),
            None => (dx, dy),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mostly_vertical_swipe_stays_vertical() {
        let mut gesture = ScrollGesture::default();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        assert_eq!(
            gesture.filter_at((2.0, -10.0), TouchPhase::Moved, at(0)),
            (0.0, -10.0)
        );
        assert_eq!(
            gesture.filter_at((4.0, -9.0), TouchPhase::Moved, at(8)),
            (0.0, -9.0)
        );
        assert_eq!(
            gesture.filter_at((5.0, -3.0), TouchPhase::Moved, at(16)),
            (0.0, -3.0)
        );
    }

    #[test]
    fn a_clear_turn_unlocks_and_a_pause_starts_over() {
        let mut gesture = ScrollGesture::default();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        gesture.filter_at((0.0, 10.0), TouchPhase::Moved, at(0));
        assert_eq!(
            gesture.filter_at((12.0, 2.0), TouchPhase::Moved, at(8)),
            (12.0, 2.0)
        );
        assert_eq!(
            gesture.filter_at((1.0, 3.0), TouchPhase::Moved, at(16)),
            (1.0, 3.0)
        );
        assert_eq!(
            gesture.filter_at((8.0, 1.0), TouchPhase::Moved, at(100)),
            (8.0, 0.0),
            "a new gesture locks to its own first direction"
        );
        assert_eq!(
            gesture.filter_at((8.0, 1.0), TouchPhase::Ended, at(108)),
            (8.0, 1.0)
        );
        assert_eq!(
            gesture.filter_at((1.0, 9.0), TouchPhase::Started, at(110)),
            (0.0, 9.0)
        );
    }
}
