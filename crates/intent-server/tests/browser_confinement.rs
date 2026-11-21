//! Browser-confinement proofs (docs/BROWSER-CONFINEMENT.md): each attack class
//! drives the probe twice — guarded (the wall holds and the attack fails) and
//! armed (`INTENT_MUTATION` opens the named hole and the attack lands). Every
//! test goes red if its guard disappears: that IS the mutation proof. Linux
//! only — namespaces do not exist elsewhere, so these are CI's to run.
#![cfg(all(target_os = "linux", feature = "mutation-seams"))]

use std::collections::HashMap;
use std::time::Duration;

use intent_render::browser::{self, BrowserJob, BrowserRefusal};
use intent_server::checks::browser_job;
use intent_server::config::Config;

fn config_with(mutation: Option<&str>) -> Config {
    let mut pairs = vec![
        (
            "INTENT_DATABASE_URL",
            "postgres://u:hunter2@127.0.0.1:59432/intent",
        ),
        ("INTENT_NOTIFY_PROVIDER", "recorder"),
        ("INTENT_PROFILE", "development"),
    ];
    if let Some(m) = mutation {
        pairs.push(("INTENT_MUTATION", m));
    }
    let map: HashMap<String, String> = pairs
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    Config::from_lookup(|key| map.get(key).cloned()).expect("minimal drill config")
}

/// Builds the confined job for `attack` (the probe copied into the bundle) and
/// runs it. `tweak` tightens bounds for the class being proved.
async fn confined(
    attack: &[&str],
    mutation: Option<&str>,
    tweak: impl Fn(&mut BrowserJob),
) -> Result<Vec<u8>, BrowserRefusal> {
    let bundle = std::env::temp_dir().join(format!("intent-bc-{}-{attack:?}", std::process::id()));
    std::fs::create_dir_all(&bundle).unwrap();
    let probe = bundle.join("probe");
    std::fs::copy(env!("CARGO_BIN_EXE_browser_probe"), &probe).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();

    let config = config_with(mutation);
    let args: Vec<String> = attack.iter().map(|s| (*s).to_owned()).collect();
    // Inside the jail the bundle is /bundle; under the host-mounts hole the
    // job sees the host path instead — the same program either way.
    let mut job = browser_job(&config, "/bundle/probe", &args, &bundle);
    if job.host_mounts {
        job.program = probe.to_string_lossy().into_owned();
    }
    tweak(&mut job);
    browser::run(&job).await
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_has_no_network() {
    let out = confined(&["net"], None, |_| {})
        .await
        .expect("confined run");
    assert!(
        !String::from_utf8_lossy(&out).contains("GOT-NET"),
        "a confined job reached a TCP socket"
    );
    let armed = confined(&["net"], Some("browser-allow-net"), |_| {}).await;
    assert!(
        armed.is_ok_and(|o| String::from_utf8_lossy(&o).contains("GOT-NET")),
        "the allow-net seam did not hand the job a route: {armed:?}"
    );
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_sees_only_its_bundle() {
    let out = confined(&["read"], None, |_| {})
        .await
        .expect("confined run");
    assert!(
        !String::from_utf8_lossy(&out).contains("GOT-READ"),
        "a confined job read a host path"
    );
    let armed = confined(&["read"], Some("browser-host-mounts"), |_| {}).await;
    assert!(
        armed.is_ok_and(|o| String::from_utf8_lossy(&o).contains("GOT-READ")),
        "the host-mounts seam hid the host filesystem anyway: {armed:?}"
    );
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_cannot_fork_bomb() {
    let out = confined(&["fork"], None, |j| j.max_procs = 96)
        .await
        .expect("confined run");
    let n: u32 = String::from_utf8_lossy(&out)
        .trim_start_matches("GOT-FORK:")
        .trim()
        .parse()
        .unwrap_or(u32::MAX);
    assert!(n < 512, "a confined job spawned {n} processes");
    let armed = confined(&["fork"], Some("browser-unbounded-procs"), |_| {}).await;
    assert!(
        armed.is_ok_and(|o| String::from_utf8_lossy(&o).trim() == "GOT-FORK:512"),
        "the unbounded-procs seam still bounded the fork bomb: {armed:?}"
    );
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_cannot_memory_bomb() {
    let guarded = confined(&["mem", "2048"], None, |j| j.mem_bytes = 256 * 1024 * 1024).await;
    assert!(
        matches!(guarded, Err(BrowserRefusal::Crashed)),
        "a confined job reached its memory target: {guarded:?}"
    );
    let armed = confined(&["mem", "2048"], Some("browser-unbounded-mem"), |_| {}).await;
    assert!(
        armed.is_ok_and(|o| String::from_utf8_lossy(&o).contains("GOT-MEM")),
        "the unbounded-mem seam still bounded the heap: {armed:?}"
    );
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_dies_on_its_deadline() {
    let guarded = confined(&["hang"], None, |j| j.deadline = Duration::from_secs(2)).await;
    assert!(
        matches!(guarded, Err(BrowserRefusal::Deadline)),
        "a hung page outlived the deadline: {guarded:?}"
    );
    // Armed, run() never returns on its own — the test bounds it, and the
    // future's drop (kill_on_drop + --kill-child) reaps the tree.
    let armed = tokio::time::timeout(
        Duration::from_secs(4),
        confined(&["hang"], Some("browser-no-deadline"), |j| {
            j.deadline = Duration::from_secs(2);
        }),
    )
    .await;
    assert!(
        armed.is_err(),
        "the no-deadline seam let the job run past its deadline"
    );
}

#[tokio::test]
#[ignore = "linux confinement proof — CI runs it"]
async fn a_browser_job_is_cut_at_its_output_bound() {
    let guarded = confined(&["big"], None, |j| {
        j.max_output_bytes = 1024 * 1024;
        j.deadline = Duration::from_secs(10);
    })
    .await;
    assert!(
        matches!(guarded, Err(BrowserRefusal::TooLarge)),
        "an output flood was not cut at the bound: {guarded:?}"
    );
    // Armed, nothing cuts the output — the job runs until the deadline kills it.
    let armed = confined(&["big"], Some("browser-unbounded-output"), |j| {
        j.deadline = Duration::from_secs(3);
    })
    .await;
    assert!(
        matches!(armed, Err(BrowserRefusal::Deadline)),
        "the unbounded-output seam still cut the report: {armed:?}"
    );
}
