//! The notification outbox: its states and the retry schedule (D-005, D-020, D-030).
//!
//! A lead and its outbox row commit together; the notifier then attempts delivery under a lease,
//! never inside a transaction. Each attempt ends in an [`AttemptOutcome`]; [`RetrySchedule::after`]
//! turns the outcome and the attempt number into the row's next state. Delivery is at least once:
//! an attempt that timed out may have reached the provider, so it is counted as uncertain and a
//! duplicate notification after it is possible (D-020).

use std::time::Duration;

use crate::limits::{
    NOTIFY_MAX_ATTEMPTS, NOTIFY_RETRY_BASE, NOTIFY_RETRY_CAP_FACTOR, NOTIFY_RETRY_GROWTH,
};

/// An outbox row's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxState {
    /// Waiting for its first attempt, or for a retry.
    Pending,
    /// The provider accepted the notification.
    Delivered,
    /// No further automatic attempts: attempts exhausted, or the provider refused it permanently.
    /// Shown in the inbox; a manual resend starts again (D-020).
    Dead,
}

impl OutboxState {
    /// The stored text.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Dead => "dead",
        }
    }

    /// Parses the stored text.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(Self::Pending),
            "delivered" => Some(Self::Delivered),
            "dead" => Some(Self::Dead),
            _ => None,
        }
    }
}

/// How one delivery attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The provider accepted it (2xx).
    Accepted,
    /// The provider said no for now (connection refused, 408, 429, 5xx): it did not take the
    /// notification, so a retry cannot duplicate it.
    Unavailable,
    /// No answer before the deadline, or the connection broke after the request was sent: the
    /// provider may have taken it. Retried; a duplicate is possible and counted.
    TimedOut,
    /// The provider refused this notification permanently (other 4xx). Retrying cannot help.
    Rejected,
}

impl AttemptOutcome {
    /// Whether the provider may have received the notification although we do not know it.
    pub const fn is_uncertain(self) -> bool {
        matches!(self, Self::TimedOut)
    }

    /// The code stored as the row's last error; never provider text.
    pub const fn error_code(self) -> Option<&'static str> {
        match self {
            Self::Accepted => None,
            Self::Unavailable => Some("provider_unavailable"),
            Self::TimedOut => Some("provider_timeout"),
            Self::Rejected => Some("provider_rejected"),
        }
    }
}

/// What happens to a row after an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Done.
    Delivered,
    /// Try again after this delay.
    RetryIn(Duration),
    /// Stop.
    Dead(DeadReason),
}

/// Why a row stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadReason {
    /// Every allowed attempt failed.
    AttemptsExhausted,
    /// The provider refused it permanently.
    Rejected,
}

/// The retry schedule (D-020 as corrected by D-030).
///
/// At most [`NOTIFY_MAX_ATTEMPTS`] attempts. After failed attempt `k` the next attempt waits a
/// delay drawn uniformly from `[c_k / 2, c_k]`, where `c_k = min(cap, base × 4^(k−1))` and
/// `cap = 360 × base`. With the production base of one minute the ceilings are 1, 4, 16, 64,
/// 256, 360 and 360 minutes, so the seven waits total 8.8 to 17.7 hours.
///
/// Drills and load runs use [`RetrySchedule::scaled`] with a smaller base: the same shape, faster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetrySchedule {
    base: Duration,
    max_attempts: u8,
}

impl RetrySchedule {
    /// The production schedule: base one minute, cap six hours, eight attempts.
    pub const PRODUCTION: Self = Self {
        base: NOTIFY_RETRY_BASE,
        max_attempts: NOTIFY_MAX_ATTEMPTS,
    };

    /// The same schedule with a different base (the cap scales with it).
    pub const fn scaled(base: Duration) -> Self {
        Self {
            base,
            max_attempts: NOTIFY_MAX_ATTEMPTS,
        }
    }

    /// The base delay.
    pub const fn base(&self) -> Duration {
        self.base
    }

    /// The largest delay.
    pub fn cap(&self) -> Duration {
        self.base.saturating_mul(NOTIFY_RETRY_CAP_FACTOR)
    }

    /// Most attempts.
    pub const fn max_attempts(&self) -> u8 {
        self.max_attempts
    }

    /// The delay ceiling after failed attempt `attempt` (1-based).
    pub fn ceiling(&self, attempt: u8) -> Duration {
        let exponent = u32::from(attempt.saturating_sub(1));
        let factor = NOTIFY_RETRY_GROWTH
            .checked_pow(exponent)
            .unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.cap())
    }

    /// The delay after failed attempt `attempt`, for a uniform random `unit` in `[0, 1]`
    /// (clamped; NaN counts as 0).
    pub fn delay(&self, attempt: u8, unit: f64) -> Duration {
        let unit = if unit.is_nan() {
            0.0
        } else {
            unit.clamp(0.0, 1.0)
        };
        let ceiling = self.ceiling(attempt);
        let half = ceiling / 2;
        half + ceiling.saturating_sub(half).mul_f64(unit)
    }

    /// The row's next state after attempt `attempt` (1-based) ended with `outcome`.
    pub fn after(&self, attempt: u8, outcome: AttemptOutcome, unit: f64) -> Transition {
        match outcome {
            AttemptOutcome::Accepted => Transition::Delivered,
            AttemptOutcome::Rejected => Transition::Dead(DeadReason::Rejected),
            AttemptOutcome::Unavailable | AttemptOutcome::TimedOut => {
                if attempt >= self.max_attempts {
                    Transition::Dead(DeadReason::AttemptsExhausted)
                } else {
                    Transition::RetryIn(self.delay(attempt, unit))
                }
            }
        }
    }

    /// The shortest and longest total wait across every retry.
    pub fn total_wait_bounds(&self) -> (Duration, Duration) {
        (1..self.max_attempts).fold((Duration::ZERO, Duration::ZERO), |(lo, hi), attempt| {
            (lo + self.delay(attempt, 0.0), hi + self.delay(attempt, 1.0))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn production_ceilings_are_one_four_sixteen_sixty_four_minutes_then_capped_at_six_hours() {
        let schedule = RetrySchedule::PRODUCTION;
        let ceilings: Vec<u64> = (1..=7)
            .map(|k| schedule.ceiling(k).as_secs() / 60)
            .collect();
        assert_eq!(ceilings, vec![1, 4, 16, 64, 256, 360, 360]);
        assert_eq!(schedule.cap(), Duration::from_hours(6));
        assert_eq!(schedule.ceiling(200), schedule.cap());
    }

    #[test]
    fn delays_stay_within_half_the_ceiling_and_the_ceiling() {
        let schedule = RetrySchedule::PRODUCTION;
        for attempt in 1..=7 {
            let ceiling = schedule.ceiling(attempt);
            assert_eq!(schedule.delay(attempt, 0.0), ceiling / 2);
            assert_eq!(schedule.delay(attempt, 1.0), ceiling);
            let middle = schedule.delay(attempt, 0.5);
            assert!(middle > ceiling / 2 && middle < ceiling);
            // Out-of-range randomness cannot escape the bounds.
            assert_eq!(schedule.delay(attempt, -3.0), ceiling / 2);
            assert_eq!(schedule.delay(attempt, 7.0), ceiling);
            assert_eq!(schedule.delay(attempt, f64::NAN), ceiling / 2);
        }
    }

    #[test]
    fn the_eighth_failure_is_dead_and_earlier_ones_retry() {
        let schedule = RetrySchedule::PRODUCTION;
        for attempt in 1..8 {
            assert!(matches!(
                schedule.after(attempt, AttemptOutcome::Unavailable, 0.5),
                Transition::RetryIn(_)
            ));
            assert!(matches!(
                schedule.after(attempt, AttemptOutcome::TimedOut, 0.5),
                Transition::RetryIn(_)
            ));
        }
        assert_eq!(
            schedule.after(8, AttemptOutcome::Unavailable, 0.5),
            Transition::Dead(DeadReason::AttemptsExhausted)
        );
        assert_eq!(
            schedule.after(8, AttemptOutcome::Accepted, 0.5),
            Transition::Delivered
        );
    }

    #[test]
    fn a_permanent_refusal_is_dead_at_once() {
        assert_eq!(
            RetrySchedule::PRODUCTION.after(1, AttemptOutcome::Rejected, 0.5),
            Transition::Dead(DeadReason::Rejected)
        );
    }

    #[test]
    fn the_total_wait_is_eight_point_eight_to_seventeen_point_seven_hours() {
        let (lo, hi) = RetrySchedule::PRODUCTION.total_wait_bounds();
        // 1 + 4 + 16 + 64 + 256 + 360 + 360 = 1,061 minutes at most; half of that at least.
        assert_eq!(hi, MINUTE * 1061);
        assert_eq!(lo, MINUTE * 1061 / 2);
    }

    #[test]
    fn a_scaled_schedule_keeps_the_shape() {
        let fast = RetrySchedule::scaled(Duration::from_millis(10));
        assert_eq!(fast.ceiling(1), Duration::from_millis(10));
        assert_eq!(fast.ceiling(3), Duration::from_millis(160));
        assert_eq!(fast.cap(), Duration::from_millis(3600));
        assert_eq!(fast.max_attempts(), 8);
    }

    #[test]
    fn only_a_timeout_is_uncertain_and_codes_are_stable() {
        assert!(AttemptOutcome::TimedOut.is_uncertain());
        assert!(!AttemptOutcome::Unavailable.is_uncertain());
        assert_eq!(AttemptOutcome::Accepted.error_code(), None);
        assert_eq!(
            AttemptOutcome::Unavailable.error_code(),
            Some("provider_unavailable")
        );
        for state in [
            OutboxState::Pending,
            OutboxState::Delivered,
            OutboxState::Dead,
        ] {
            assert_eq!(OutboxState::parse(state.as_str()), Some(state));
        }
        assert_eq!(OutboxState::parse("sent"), None);
    }
}
