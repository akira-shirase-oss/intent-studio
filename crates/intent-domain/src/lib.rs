//! Intent Studio's domain, in pure form.
//!
//! Everything here is plain data and functions: no async runtime, no database, no network, no
//! filesystem and no clock. Time and randomness are parameters. `scripts/check-architecture.sh`
//! keeps it that way, so every rule in this crate stays cheap to test exhaustively.
//!
//! I0 holds the lead-intake spine: the lead and its validation ([`lead`]), the idempotency key
//! that makes a retried submit one lead ([`submission`]), the lead's states ([`state`]), the
//! notification retry schedule ([`outbox`]), the abuse limits ([`limits`], [`ratelimit`]),
//! retention ([`retention`]), the notification text ([`notification`]) and the stable refusal
//! codes clients branch on ([`refusal`]).

pub mod ids;
pub mod lead;
pub mod limits;
pub mod notification;
pub mod outbox;
pub mod ratelimit;
pub mod refusal;
pub mod retention;
pub mod state;
pub mod submission;
