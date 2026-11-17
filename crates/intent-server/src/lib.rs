//! Intent Studio's server.
//!
//! I0, the lead-intake spine: `POST /v1/sites/{site}/leads` stores a lead and its notification
//! outbox row in one commit before acknowledging it; the leased notifier delivers on the D-020
//! schedule (as corrected by D-030) without ever blocking intake; readiness reports whether a
//! lead sent now would be stored; the operator reads the inbox behind a password session.

pub mod app;
pub mod config;
pub mod error;
pub mod health;
pub mod intake;
pub mod limiter;
pub mod metrics;
pub mod notifier;
pub mod operator;
pub mod reconcile;
pub mod serve;
pub mod telemetry;
