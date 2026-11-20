//! The shipped worker's confinement, proved on the real binary: deleting the `confine()`
//! call in `main` fails this — a test that only exercises the filter function cannot.
#![cfg(all(target_os = "linux", target_pointer_width = "64"))]

use std::process::{Command, Stdio};

#[test]
fn the_started_worker_has_no_new_privs_and_a_seccomp_filter() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_intent-worker"))
        .arg("svg")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    // The worker confines itself before reading a byte; held-open stdin keeps it parked
    // inside the confined read while its status is examined.
    let mut status = String::new();
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        status =
            std::fs::read_to_string(format!("/proc/{}/status", child.id())).unwrap_or_default();
        if status.contains("Seccomp:") {
            break;
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        status.contains("NoNewPrivs:\t1"),
        "worker did not set no_new_privs: {status}"
    );
    assert!(
        status.contains("Seccomp:\t2"),
        "worker is not under a seccomp filter: {status}"
    );
}
