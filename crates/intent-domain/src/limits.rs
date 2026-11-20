//! Every number the lead-intake spine enforces, in one place (D-019, D-020, D-030).
//!
//! These are the proposed starting values from `docs/DECISIONS.md`; changing one needs a
//! measurement and a new decision entry.

use std::time::Duration;

/// Largest accepted request body for a lead submission (D-019: 16 KiB).
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// Per-site token bucket: leads per minute, which is also the burst (D-019).
pub const SITE_LEADS_PER_MINUTE: u32 = 60;

/// Per-client-address token bucket: leads per minute, which is also the burst (D-019).
pub const ADDRESS_LEADS_PER_MINUTE: u32 = 5;

/// Entries in each bounded rate-limit table; least recently used entries are evicted (D-019).
pub const RATE_TABLE_ENTRIES: usize = 100_000;

/// Window for a server-derived submission key when the form carries none (D-019: 10 minutes).
pub const DERIVED_KEY_WINDOW_SECS: i64 = 600;

/// Field limits, in Unicode scalar values (characters), after trimming.
pub const NAME_MAX_CHARS: usize = 200;
/// Longest accepted e-mail address (RFC 5321 path limit).
pub const EMAIL_MAX_CHARS: usize = 254;
/// Longest accepted phone field, formatting included.
pub const PHONE_MAX_CHARS: usize = 32;
/// Fewest digits a phone number may have.
pub const PHONE_MIN_DIGITS: usize = 6;
/// Most digits a phone number may have (E.164).
pub const PHONE_MAX_DIGITS: usize = 15;
/// Longest accepted free-text message.
pub const MESSAGE_MAX_CHARS: usize = 5_000;
/// Longest accepted service-type and service-area fields.
pub const SHORT_FIELD_MAX_CHARS: usize = 100;
/// Longest privacy-notice version id (D-044).
pub const CONSENT_NOTICE_MAX_CHARS: usize = 64;
/// Client-supplied submission ids: length bounds.
pub const SUBMISSION_ID_MIN_CHARS: usize = 8;
/// Client-supplied submission ids: length bounds.
pub const SUBMISSION_ID_MAX_CHARS: usize = 64;

/// One notification attempt's deadline (D-020: 10 s).
pub const NOTIFY_ATTEMPT_DEADLINE: Duration = Duration::from_secs(10);

/// Most notification attempts before an outbox row is `dead` (D-020: 8).
pub const NOTIFY_MAX_ATTEMPTS: u8 = 8;

/// The first retry delay's ceiling (D-020: 1 minute).
pub const NOTIFY_RETRY_BASE: Duration = Duration::from_secs(60);

/// No retry delay exceeds this (D-020: 6 hours) — 360 times the base.
pub const NOTIFY_RETRY_CAP_FACTOR: u32 = 360;

/// Each retry delay's ceiling grows by this factor per attempt (D-030).
pub const NOTIFY_RETRY_GROWTH: u32 = 4;

/// Operator sign-in attempts per minute from one address (D-036).
pub const SIGN_IN_PER_ADDRESS_PER_MINUTE: u32 = 5;

/// Operator sign-in attempts per minute from everywhere together, so guesses spread over many
/// addresses are bounded too (D-041).
pub const SIGN_IN_GLOBAL_PER_MINUTE: u32 = 20;

/// Failed session checks per minute from one address. A session check is an UPDATE, and an
/// anonymous client can ask for one on every request; the charge is refunded when the session
/// turns out to be live, so only failed checks are counted (D-019's bound applies here too).
pub const SESSION_CHECK_FAILURES_PER_ADDRESS_PER_MINUTE: u32 = 20;
