//! The harness's report when a server never starts. Runs without the stack:
//! the child exits on a refused configuration before any database exists.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]
mod support;

/// When `Server::start` gives up, the failure must carry the child's exit status
/// and its log tail — an empty "the server never started:" hid the store-lock
/// refusal that failed CI.
#[tokio::test]
async fn a_failed_start_reports_the_exit_status_and_the_log_tail() {
    // env_clear strips the ambient environment: the child exits on a refused
    // configuration at once, every attempt.
    let outcome = tokio::spawn(support::Server::start(Vec::new())).await;
    let panic = match outcome {
        Ok(_) => panic!("an empty environment started a server"),
        Err(join) => join.into_panic(),
    };
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&'static str>().copied())
        .expect("the start-failure panic is a string");
    assert!(
        message.contains("exit"),
        "the exit status belongs in the failure: {message}"
    );
    assert!(
        message.contains("INTENT_DATABASE_URL"),
        "the server's own stderr belongs in the failure: {message}"
    );
}
