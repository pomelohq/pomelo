use std::time::Duration;

use crate::{Limits, LimitsError};

/// The usage endpoint is only asked when no agent reported limits lately, and rarely: Claude Code asks
/// it too, and it refuses callers that ask often.
pub const ASK_EVERY: Duration = Duration::from_secs(300);
pub const BACK_OFF: Duration = Duration::from_secs(900);
/// Limits a status line reported this recently are current enough to show.
pub const REPORTED_FRESH: Duration = Duration::from_secs(20 * 60);

/// The plan limits as last known: what a status line reported, else what the endpoint said, asked at most
/// every [`ASK_EVERY`] (every [`BACK_OFF`] after it said to slow down).
#[derive(Clone, Debug)]
pub struct LimitsKeeper {
    kept: Option<(Limits, u64)>,
    /// Between asks, what the last ask said still holds.
    last_error: LimitsError,
    next_ask: u64,
}

impl Default for LimitsKeeper {
    fn default() -> LimitsKeeper {
        LimitsKeeper {
            kept: None,
            last_error: LimitsError::SignedOut,
            next_ask: 0,
        }
    }
}

impl LimitsKeeper {
    /// The limits at `now`; `fetch` runs only when signed in, nothing fresh was reported, and an ask is due
    /// (or `asked` says the person wants one now).
    pub fn update(
        &mut self,
        now: u64,
        signed_in: bool,
        reported: Option<(Limits, u64)>,
        asked: bool,
        fetch: impl FnOnce() -> Result<Limits, LimitsError>,
    ) -> Result<Limits, LimitsError> {
        let reported =
            reported.filter(|(_, at)| now.saturating_sub(*at) < REPORTED_FRESH.as_secs());
        if let Some(reported) = reported {
            if self.kept.is_none_or(|(_, kept_at)| reported.1 >= kept_at) {
                self.kept = Some(reported);
            }
        }
        let mut limits = self
            .kept
            .map(|(limits, _)| limits)
            .ok_or_else(|| self.last_error.clone());
        if signed_in && reported.is_none() && (now >= self.next_ask || asked) {
            match fetch() {
                Ok(fresh) => {
                    self.kept = Some((fresh, now));
                    limits = Ok(fresh);
                    self.next_ask = now + ASK_EVERY.as_secs();
                }
                Err(error) => {
                    let wait = if error == LimitsError::RateLimited {
                        BACK_OFF
                    } else {
                        ASK_EVERY
                    };
                    self.next_ask = now + wait.as_secs();
                    self.last_error = error.clone();
                    if self.kept.is_none() {
                        limits = Err(error);
                    }
                }
            }
        }
        limits
    }

    /// When the limits shown were read.
    pub fn kept_at(&self) -> Option<u64> {
        self.kept.map(|(_, at)| at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Window;

    fn limits(used: f32) -> Limits {
        Limits {
            session: Window {
                used,
                resets_at: None,
            },
            weekly: Window::default(),
        }
    }

    #[test]
    fn the_endpoint_is_asked_rarely_and_less_after_a_rate_limit() {
        let mut keeper = LimitsKeeper::default();
        let mut asks = 0;
        let mut fetch = |answer: Result<Limits, LimitsError>| {
            asks += 1;
            answer
        };
        assert_eq!(
            keeper.update(1000, true, None, false, || fetch(Ok(limits(10.0)))),
            Ok(limits(10.0))
        );
        assert_eq!(
            keeper.update(1060, true, None, false, || fetch(Ok(limits(99.0)))),
            Ok(limits(10.0)),
            "not due yet"
        );
        assert_eq!(
            keeper.update(1300, true, None, false, || fetch(Err(
                LimitsError::RateLimited
            ))),
            Ok(limits(10.0)),
            "a refusal keeps what was read"
        );
        assert_eq!(
            keeper.update(1300 + 600, true, None, false, || fetch(Ok(limits(50.0)))),
            Ok(limits(10.0)),
            "backing off"
        );
        assert_eq!(
            keeper.update(1300 + 900, true, None, false, || fetch(Ok(limits(50.0)))),
            Ok(limits(50.0))
        );
        assert_eq!(asks, 3);
        assert_eq!(keeper.kept_at(), Some(2200));
    }

    #[test]
    fn a_fresh_report_wins_and_spares_the_ask() {
        let mut keeper = LimitsKeeper::default();
        let answer = keeper.update(5000, true, Some((limits(42.0), 4900)), false, || {
            panic!("no ask while a status line reports")
        });
        assert_eq!(answer, Ok(limits(42.0)));
        let stale = Some((limits(1.0), 5000 - REPORTED_FRESH.as_secs() - 1));
        let answer = keeper.update(5000, false, stale, false, || panic!("signed out"));
        assert_eq!(answer, Ok(limits(42.0)));
    }

    #[test]
    fn signed_out_says_so_until_an_ask_works() {
        let mut keeper = LimitsKeeper::default();
        assert_eq!(
            keeper.update(10, false, None, false, || panic!("signed out")),
            Err(LimitsError::SignedOut)
        );
        assert_eq!(
            keeper.update(10, true, None, true, || Err(LimitsError::Failed(
                "x".into()
            ))),
            Err(LimitsError::Failed("x".into()))
        );
    }
}
