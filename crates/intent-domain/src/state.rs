//! A lead's states, and which transitions exist.
//!
//! ```text
//! received ──commit lead + outbox row──▶ stored ──attempt ok──────────────▶ delivered
//!  (memory only,                          │  ▲                              ▲
//!   never acknowledged)                   │  └─ retry ─┐                     │
//!                                         └─attempt failed─▶ notification_pending ─attempt ok─┘
//!                                         │                    │
//!                                         └──── refused / 8th failure ─▶ dead (visible; resend)
//! ```
//!
//! `received` exists only in memory: the server acknowledges a lead only once it is `stored`,
//! which is the moment the lead and its outbox row committed together (invariant 5). A stored
//! lead is never lost by anything that happens to its notification.

use crate::outbox::OutboxState;

/// Where a lead is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeadState {
    /// Parsed and validated, not yet committed. Never shown to the client as success.
    Received,
    /// Committed with its outbox row; no notification attempt has finished yet.
    Stored,
    /// At least one attempt failed; a retry is scheduled.
    NotificationPending,
    /// The provider accepted the notification.
    Delivered,
    /// Notification stopped (attempts exhausted or refused). The lead itself is still stored.
    NotificationDead,
}

impl LeadState {
    /// The wire and display name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Stored => "stored",
            Self::NotificationPending => "notification_pending",
            Self::Delivered => "delivered",
            Self::NotificationDead => "notification_dead",
        }
    }

    /// Whether `next` may follow `self`.
    pub const fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Received, Self::Stored)
                | (
                    Self::Stored | Self::NotificationPending,
                    Self::NotificationPending | Self::Delivered | Self::NotificationDead
                )
        )
    }

    /// Whether a client may be told this lead was saved.
    pub const fn is_acknowledged(self) -> bool {
        !matches!(self, Self::Received)
    }

    /// The state of a stored lead, from its outbox row.
    pub const fn of_stored(outbox: OutboxState, attempts: u8) -> Self {
        match outbox {
            OutboxState::Pending if attempts == 0 => Self::Stored,
            OutboxState::Pending => Self::NotificationPending,
            OutboxState::Delivered => Self::Delivered,
            OutboxState::Dead => Self::NotificationDead,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LeadState::{self, *};
    use crate::outbox::OutboxState;

    const ALL: [LeadState; 5] = [
        Received,
        Stored,
        NotificationPending,
        Delivered,
        NotificationDead,
    ];

    #[test]
    fn exactly_the_drawn_transitions_exist() {
        let allowed: Vec<(LeadState, LeadState)> = ALL
            .iter()
            .flat_map(|&a| ALL.iter().map(move |&b| (a, b)))
            .filter(|&(a, b)| a.allows(b))
            .collect();
        assert_eq!(
            allowed,
            vec![
                (Received, Stored),
                (Stored, NotificationPending),
                (Stored, Delivered),
                (Stored, NotificationDead),
                (NotificationPending, NotificationPending),
                (NotificationPending, Delivered),
                (NotificationPending, NotificationDead),
            ]
        );
    }

    #[test]
    fn terminal_states_have_no_way_out_and_nothing_returns_to_received() {
        for state in ALL {
            assert!(!Delivered.allows(state));
            assert!(!NotificationDead.allows(state));
            assert!(!state.allows(Received));
        }
    }

    #[test]
    fn only_a_received_lead_is_unacknowledged() {
        assert!(!Received.is_acknowledged());
        for state in &ALL[1..] {
            assert!(state.is_acknowledged());
        }
    }

    #[test]
    fn stored_states_follow_the_outbox_row() {
        assert_eq!(LeadState::of_stored(OutboxState::Pending, 0), Stored);
        assert_eq!(
            LeadState::of_stored(OutboxState::Pending, 3),
            NotificationPending
        );
        assert_eq!(LeadState::of_stored(OutboxState::Delivered, 1), Delivered);
        assert_eq!(LeadState::of_stored(OutboxState::Dead, 8), NotificationDead);
        assert_eq!(NotificationDead.as_str(), "notification_dead");
    }
}
