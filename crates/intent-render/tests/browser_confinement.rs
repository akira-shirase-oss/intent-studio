//! Browser-confinement proofs (docs/BROWSER-CONFINEMENT.md): each attack class
//! drives the probe twice — guarded (the wall holds and the attack fails) and
//! armed (the hole flag opens the wall and the attack lands). The flag is the
//! hole: `INTENT_MUTATION`'s only job is setting it, and intent-server's seam
//! test proves that mapping on every platform. Every test here goes red if its
//! guard disappears: that IS the mutation proof.
//!
//! Linux only — namespaces do not exist elsewhere, so these are CI's to run.
//! The file lives in intent-render so `cargo check --target
//! x86_64-unknown-linux-gnu -p intent-render --all-targets` compiles it on
//! hosts that cannot link the server's C dependencies — Linux-only code must
//! still compile where it cannot run.
#![cfg(all(target_os = "linux", feature = "mutation-seams"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use intent_render::browser::{self, BrowserJob, BrowserRefusal};

/// Builds the confined job for `attack` (the probe copied into the bundle),
/// sets `hole` (the flag the same-named seam would set), and runs it.
/// `tweak` tightens bounds for the class being proved.
async fn confined(
    attack: &[&str],
    hole: impl Fn(&mut BrowserJob),
    tweak: impl Fn(&mut BrowserJob),
) -> Result<Vec<u8>, BrowserRefusal> {
    let bundle = std::env::temp_dir().join(format!("intent-bc-{}-{attack:?}", std::process::id()));
    std::fs::create_dir_all(&bundle).unwrap();
    let probe = bundle.join("probe");
    std::fs::copy(env!("CARGO_BIN_EXE_browser_probe"), &probe).unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut job = BrowserJob {
        program: "/bundle/probe".to_owned(),
        args: attack.iter().map(|s| (*s).to_owned()).collect(),
        bundle_dir: bundle,
        ..BrowserJob::default()
    };
    hole(&mut job);
    // Inside the jail the bundle is /bundle; under the host-mounts hole the
    // job sees the host path instead — the same program either way.
    if job.holes.host_mounts {
        job.program = probe.to_string_lossy().into_owned();
    }
    tweak(&mut job);
    browser::run(&job).await
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_has_no_network() {
    let out = confined(&["net"], |_| {}, |_| {})
        .await
        .expect("confined run");
    assert!(
        !String::from_utf8_lossy(&out).contains("GOT-NET"),
        "a confined job reached a TCP socket"
    );
    let armed = confined(&["net"], |j| j.holes.allow_net = true, |_| {}).await;
    assert!(
        armed
            .as_ref()
            .is_ok_and(|o| String::from_utf8_lossy(o).contains("GOT-NET")),
        "the allow-net hole did not hand the job a route: {armed:?}"
    );
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_sees_only_its_bundle() {
    let out = confined(&["read"], |_| {}, |_| {})
        .await
        .expect("confined run");
    assert!(
        !String::from_utf8_lossy(&out).contains("GOT-READ"),
        "a confined job read a host path"
    );
    let armed = confined(&["read"], |j| j.holes.host_mounts = true, |_| {}).await;
    assert!(
        armed
            .as_ref()
            .is_ok_and(|o| String::from_utf8_lossy(o).contains("GOT-READ")),
        "the host-mounts hole hid the host filesystem anyway: {armed:?}"
    );
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_cannot_fork_bomb() {
    let out = confined(&["fork"], |_| {}, |j| j.max_procs = 96)
        .await
        .expect("confined run");
    let n: u32 = String::from_utf8_lossy(&out)
        .trim_start_matches("GOT-FORK:")
        .trim()
        .parse()
        .unwrap_or(u32::MAX);
    assert!(n < 512, "a confined job spawned {n} processes");
    let armed = confined(&["fork"], |j| j.holes.unbounded_procs = true, |_| {}).await;
    assert!(
        armed
            .as_ref()
            .is_ok_and(|o| String::from_utf8_lossy(o).trim() == "GOT-FORK:512"),
        "the unbounded-procs hole still bounded the fork bomb: {armed:?}"
    );
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_cannot_memory_bomb() {
    let guarded = confined(
        &["mem", "2048"],
        |_| {},
        |j| {
            j.mem_bytes = 256 * 1024 * 1024;
        },
    )
    .await;
    assert!(
        matches!(guarded, Err(BrowserRefusal::Crashed)),
        "a confined job reached its memory target: {guarded:?}"
    );
    let armed = confined(&["mem", "2048"], |j| j.holes.unbounded_mem = true, |_| {}).await;
    assert!(
        armed
            .as_ref()
            .is_ok_and(|o| String::from_utf8_lossy(o).contains("GOT-MEM")),
        "the unbounded-mem hole still bounded the heap: {armed:?}"
    );
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_dies_on_its_deadline() {
    let guarded = confined(
        &["hang"],
        |_| {},
        |j| {
            j.deadline = Duration::from_secs(2);
        },
    )
    .await;
    assert!(
        matches!(guarded, Err(BrowserRefusal::Deadline)),
        "a hung page outlived the deadline: {guarded:?}"
    );
    // With the hole, run() never returns on its own — the test bounds it, and
    // the future's drop (kill_on_drop + --kill-child) reaps the tree.
    let armed = tokio::time::timeout(
        Duration::from_secs(4),
        confined(
            &["hang"],
            |j| j.holes.no_deadline = true,
            |j| {
                j.deadline = Duration::from_secs(2);
            },
        ),
    )
    .await;
    assert!(
        armed.is_err(),
        "the no-deadline hole let the job run past its deadline"
    );
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_is_cut_at_its_output_bound() {
    let guarded = confined(
        &["big"],
        |_| {},
        |j| {
            j.max_output_bytes = 1024 * 1024;
            j.deadline = Duration::from_secs(10);
        },
    )
    .await;
    assert!(
        matches!(guarded, Err(BrowserRefusal::TooLarge)),
        "an output flood was not cut at the bound: {guarded:?}"
    );
    // With the hole, nothing cuts the output — the job runs until the deadline
    // kills it.
    let armed = confined(
        &["big"],
        |j| j.holes.unbounded_output = true,
        |j| {
            j.deadline = Duration::from_secs(3);
        },
    )
    .await;
    assert!(
        matches!(armed, Err(BrowserRefusal::Deadline)),
        "the unbounded-output hole still cut the report: {armed:?}"
    );
}
