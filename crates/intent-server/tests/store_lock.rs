//! Startup behaviour around the content store's root lock (D-061). Runs without
//! the stack: no database is touched — a store `open` happens before the lazy
//! pool would.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]
mod support;

use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The smallest environment that reaches `studio()`: parseable database URL,
/// the development-only recorder, a real operator hash, the shared store root.
fn serve_env(store: &Path) -> Vec<(String, String)> {
    let mut child = Command::new(support::server_binary())
        .arg("hash-password")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"store-lock-password\n")
        .unwrap();
    let hash = String::from_utf8(child.wait_with_output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_owned();
    [
        (
            "INTENT_DATABASE_URL",
            "postgres://intent:intent-dev@127.0.0.1:1/none".to_owned(),
        ),
        ("INTENT_PROFILE", "development".to_owned()),
        ("INTENT_NOTIFY_PROVIDER", "recorder".to_owned()),
        ("INTENT_OPERATOR_PASSWORD_HASH", hash),
        ("INTENT_COOKIE_SECURE", "false".to_owned()),
        ("INTENT_BIND", "127.0.0.1:0".to_owned()),
        ("INTENT_STORE_DIR", store.display().to_string()),
        // Any existing file satisfies the worker check; the store lock is judged first.
        ("INTENT_WORKER_BIN", support::server_binary().to_owned()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

fn serve(store: &Path) -> std::process::Child {
    Command::new(support::server_binary())
        .arg("serve")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .envs(serve_env(store))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Waits for `child` to exit within `limit`; its exit status and captured stderr.
fn wait_for_exit(
    child: &mut std::process::Child,
    limit: Duration,
) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if start.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Two live processes must never share one store root — the CI regression behind
/// D-061's lock: the harness ran a second `intent-server` on a root the first
/// still held, and every start attempt exited the moment `studio()` opened the
/// store. A held root refuses the second server, clearly and at once; once the
/// holder is gone the same root serves again.
#[test]
fn a_second_server_sharing_the_store_root_is_refused_at_startup() {
    let root: PathBuf = std::env::temp_dir().join(format!(
        "intent-store-lock-{}",
        uuid::Uuid::now_v7().simple()
    ));
    // The first process: a live store opened in this process holds the root's lock.
    let holder = intent_store::ContentStore::open(&root, 1024).unwrap();

    // The second process, exactly as the harness spawned it, must be refused.
    let mut second = serve(&root);
    let status = wait_for_exit(&mut second, Duration::from_secs(15));
    let mut stderr = String::new();
    second
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        status.is_some_and(|s| !s.success()),
        "a second server on a held root must exit nonzero, got {status:?}"
    );
    assert!(
        stderr.contains("held by another live store"),
        "the refusal must name the lock, got: {stderr}"
    );

    // The holder released: the same root accepts a server again — it stays up on
    // a dead database (readiness is the prober's, not startup's).
    drop(holder);
    let mut third = serve(&root);
    assert!(
        wait_for_exit(&mut third, Duration::from_secs(3)).is_none(),
        "with the lock free the server runs; it must not exit"
    );
    third.kill().unwrap();
    third.wait().unwrap();
    let _ = std::fs::remove_dir_all(&root);
}
