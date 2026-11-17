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
//!
//! I1 adds the studio's records: fact records that expire ([`fact`]), the brief ([`brief`]),
//! the claim lint for free text ([`claims`]), the typed page tree and its validator
//! ([`page_tree`]), and the named, path-carrying problems they report ([`problem`]).

pub mod brief;
pub mod claims;
pub mod content;
pub mod fact;
pub mod ids;
pub mod lead;
pub mod limits;
pub mod notification;
pub mod outbox;
pub mod page_tree;
pub mod problem;
pub mod ratelimit;
pub mod refusal;
pub mod retention;
pub mod state;
pub mod submission;
