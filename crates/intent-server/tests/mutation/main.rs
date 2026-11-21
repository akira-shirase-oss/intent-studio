//! The mutation proofs (`tests/mutation/`): every guard the attacks exercise gets its own
//! test here, booted with `INTENT_MUTATION=<guard>` under the `mutation-seams` cargo
//! feature, asserting the *vulnerable* outcome the paired attack makes impossible. A proof
//! that goes red means the seam, not the protection, is what broke. The proofs are their
//! own target so `tests/attacks` can keep running against the default — shipped — build.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

#[path = "../support/mod.rs"]
mod support;

mod mutation;
