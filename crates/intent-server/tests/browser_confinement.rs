//! The thin caller side of the browser-confinement proofs: `INTENT_MUTATION`
//! must arm exactly the hole flag its name claims. The behavioral proofs —
//! each class run guarded and armed inside the jail — live in
//! `intent-render/tests/browser_confinement.rs`, where they compile-check on
//! any host (`cargo check --target x86_64-unknown-linux-gnu`) and run on CI's
//! Linux. This half needs no namespaces, so it runs everywhere.
#![cfg(feature = "mutation-seams")]
#![allow(
    clippy::expect_used,
    reason = "test code: a failed expect is a failed test"
)]

use std::collections::HashMap;

use intent_render::browser::BrowserJob;
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

fn flags(job: &BrowserJob) -> [bool; 6] {
    [
        job.holes.allow_net,
        job.holes.host_mounts,
        job.holes.unbounded_procs,
        job.holes.unbounded_mem,
        job.holes.no_deadline,
        job.holes.unbounded_output,
    ]
}

#[test]
fn every_browser_seam_arms_exactly_its_own_hole() {
    let cases: [(&str, usize); 6] = [
        ("browser-allow-net", 0),
        ("browser-host-mounts", 1),
        ("browser-unbounded-procs", 2),
        ("browser-unbounded-mem", 3),
        ("browser-no-deadline", 4),
        ("browser-unbounded-output", 5),
    ];
    let dir = std::path::Path::new("/tmp/intent-bc-seams");
    for (name, index) in cases {
        let job = browser_job(&config_with(Some(name)), "probe", &[], dir);
        let set = flags(&job);
        assert!(
            set[index] && set.iter().filter(|f| **f).count() == 1,
            "{name} must arm exactly flag {index}, got {set:?}"
        );
    }
    // And a boot with no mutation arms nothing.
    let none = browser_job(&config_with(None), "probe", &[], dir);
    assert!(flags(&none).iter().all(|f| !f), "no mutation armed a hole");
}
