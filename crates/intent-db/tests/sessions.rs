//! Database round-trips for operator sessions — ignored until the stack is up
//! (`scripts/stack.sh up`), same posture as the server drills.

use std::time::Duration;

use intent_db::{Db, PoolLimits};

const ADMIN_URL: &str = "postgres://intent:intent-dev@127.0.0.1:59432/intent";

#[tokio::test]
#[ignore = "needs the stack database"]
async fn session_lifetime_returns_the_remaining_seconds_of_a_live_row() {
    let db = Db::connect_lazy(
        ADMIN_URL,
        PoolLimits {
            max_connections: 1,
            acquire_timeout: Duration::from_secs(2),
            statement_timeout: Duration::from_secs(2),
        },
    )
    .expect("database url parses");
    db.migrate().await.expect("migrations apply on the stack");

    let digest: [u8; 32] = *b"n15-live-session-digest-padded32";
    db.create_session(&digest, Duration::from_secs(300))
        .await
        .expect("the session row inserts");

    let remaining = db
        .session_lifetime(&digest, Duration::from_secs(600))
        .await
        .expect("the RETURNING value decodes — numeric without ::float8 fails here (N15)")
        .expect("a live row reports its remaining lifetime");
    assert!(
        (290.0..=300.0).contains(&remaining),
        "remaining {remaining}"
    );

    assert!(db.delete_session(&digest).await.unwrap());
    assert!(
        db.session_lifetime(&digest, Duration::from_secs(600))
            .await
            .unwrap()
            .is_none()
    );
}
