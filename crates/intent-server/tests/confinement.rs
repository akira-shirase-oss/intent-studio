//! The shipped server's hardening, proved on the real binary: deleting the
//! `deny_dumpable()` call in `main` fails this — a test that only exercises the function
//! cannot see whether the process entry point calls it.
#![cfg(target_os = "linux")]

use std::process::Command;

#[test]
fn the_started_server_reports_non_dumpable() {
    let output = Command::new(env!("CARGO_BIN_EXE_intent-server"))
        .env("INTENT_PROBE_DUMPABLE", "1")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "NotDumpable",
        "the server did not go non-dumpable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
