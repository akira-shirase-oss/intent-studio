//! The pentest regression suite (`tests/attacks/`): real attack requests against a running
//! `intent-server` — forged and absent session cookies on every operator route, a client that
//! tries to choose its own session token, a dead session replayed, cross-tenant and
//! path-confusion access, `X-Forwarded-For` spoofing to move the per-address limiter, the
//! metrics token refused however the client describes itself, server-side request forgery
//! through brief references (loopback, link-local, the metadata address), oversized and
//! deeply nested bodies, content-type confusion, a slow-dribbled body, and a replayed lead —
//! each asserting the refusal, not the crash. The `fuzz` module is the never-panics surface:
//! property tests over every decoder of untrusted bytes (SVG, PNG, JPEG, font) with a seed
//! corpus under `fuzz/corpus/`; a crash found is a red-first bug.
//!
//! The attacks need the stack (`scripts/stack.sh up`) and run by `scripts/drills.sh` like the
//! other drills; the fuzz targets are pure and run in every `cargo test`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

#[path = "../support/mod.rs"]
mod support;

mod auth;
mod bodies;
mod fuzz;
mod headers;
mod mutation;
mod replay;
mod ssrf;
