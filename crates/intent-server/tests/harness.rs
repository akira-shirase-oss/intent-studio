//! The harness cleans up after itself: a test database and a server's temporary directory
//! are removed when they go out of scope, even when a test never reaches its own cleanup
//! (a failed assertion). Run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

mod support;

use intent_provider_fixture::Mode;
use sqlx::Connection as _;
use support::{ADMIN_URL, ProviderFixture, Server, TestDb, drill_env};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn nothing_is_left_behind_when_a_test_skips_its_cleanup() {
    let db = TestDb::create(1).await;
    let name = db.name.clone();
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = Server::start(drill_env(&db, &fixture.url)).await;
    let dir = server.log_path.parent().unwrap().to_path_buf();
    assert!(dir.exists());
    // Out of scope without `drop_database`, as after a failed assertion.
    drop(server);
    drop(db);
    assert!(
        !dir.exists(),
        "the server's temporary directory was left behind"
    );
    let mut admin = sqlx::PgConnection::connect(ADMIN_URL).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_database WHERE datname = $1")
        .bind(&name)
        .fetch_one(&mut admin)
        .await
        .unwrap();
    assert_eq!(left, 0, "the test database was left behind");
}
