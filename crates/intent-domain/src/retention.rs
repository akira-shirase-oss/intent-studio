//! How long a lead is kept (D-019: 12 months by default, configurable per tenant).
//!
//! Retention is counted in days: 365 by default, so "12 months" never depends on month lengths
//! or leap days. Each lead stores its own `retain_until`, computed when it is stored; deletion on
//! request and the purge job arrive with I3.

/// A tenant's retention period, in days: 30 to 3,650 (ten years).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionDays(u16);

impl RetentionDays {
    /// Fewest days a tenant may choose.
    pub const MIN: u16 = 30;
    /// Most days a tenant may choose.
    pub const MAX: u16 = 3_650;
    /// The default: 365 days (D-019's 12 months).
    pub const DEFAULT: Self = Self(365);

    /// Validates a configured period.
    pub fn new(days: u16) -> Result<Self, RetentionOutOfRange> {
        if (Self::MIN..=Self::MAX).contains(&days) {
            Ok(Self(days))
        } else {
            Err(RetentionOutOfRange(days))
        }
    }

    /// The period in days.
    pub const fn days(self) -> u16 {
        self.0
    }

    /// The `retain_until` instant, in Unix seconds, for a lead received at `received_unix_secs`.
    pub const fn retain_until(self, received_unix_secs: i64) -> i64 {
        received_unix_secs.saturating_add(self.0 as i64 * 86_400)
    }
}

impl Default for RetentionDays {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A retention period outside 30 to 3,650 days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("retention must be between 30 and 3650 days, got {0}")]
pub struct RetentionOutOfRange(pub u16);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_hold_at_both_ends() {
        assert!(RetentionDays::new(29).is_err());
        assert_eq!(RetentionDays::new(30).unwrap().days(), 30);
        assert_eq!(RetentionDays::new(3_650).unwrap().days(), 3_650);
        assert_eq!(RetentionDays::new(3_651), Err(RetentionOutOfRange(3_651)));
        assert_eq!(RetentionDays::default().days(), 365);
    }

    #[test]
    fn retain_until_adds_whole_days() {
        assert_eq!(RetentionDays::DEFAULT.retain_until(0), 365 * 86_400);
        assert_eq!(RetentionDays::DEFAULT.retain_until(i64::MAX), i64::MAX);
    }
}
