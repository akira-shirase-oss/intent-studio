//! Readiness measures the database, not intake's own load (F17): with every intake connection
//! busy, the probe still answers, so a saturated server never declares the database down and
//! refuses everything. Run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

mod support;

use std::time::{Duration, Instant};

use intent_provider_fixture::Mode;
use sqlx::Connection as _;
use support::{
    LeadSpec, ProviderFixture, Server, TestDb, address_for, client, drill_env, site_for, submit,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn readiness_stays_green_while_intakes_pool_is_saturated() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    env.push(("INTENT_DB_MAX_CONNECTIONS".into(), "2".into()));
    let server = Server::start(env).await;
    let site = site_for(0, 1);
    // Warm up: the site is verified and a lead stored.
    let warm = submit(
        &client(),
        &server.base,
        site,
        &LeadSpec::new("ready", 0).json(),
        Some("10.90.0.1"),
        0,
        Instant::now(),
    )
    .await;
    assert_eq!(warm.status, Some(201));

    // Another session locks the leads table, so every insert waits (up to the 3 s statement
    // timeout) and holds one of intake's two connections meanwhile.
    let mut locker = sqlx::PgConnection::connect(&db.url).await.unwrap();
    let mut lock = locker.begin().await.unwrap();
    sqlx::query("LOCK TABLE leads IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let load = tokio::spawn({
        let base = server.base.clone();
        async move {
            let client = client();
            let mut tasks = tokio::task::JoinSet::new();
            for n in 1..=40_u64 {
                let (client, base) = (client.clone(), base.clone());
                tasks.spawn(async move {
                    submit(
                        &client,
                        &base,
                        site,
                        &LeadSpec::new("ready", n).json(),
                        Some(&address_for(n)),
                        n,
                        Instant::now(),
                    )
                    .await
                });
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            while tasks.join_next().await.is_some() {}
        }
    });
    // For three seconds, readiness must stay green: the database is up, only intake is busy.
    let probe = client();
    let start = Instant::now();
    let mut red = 0;
    while start.elapsed() < Duration::from_secs(3) {
        let status = probe
            .get(format!("{}/readyz", server.base))
            .send()
            .await
            .unwrap()
            .status();
        if status != 200 {
            red += 1;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    lock.rollback().await.unwrap();
    load.await.unwrap();
    assert_eq!(
        red, 0,
        "readiness went red {red} time(s) while only intake's pool was busy"
    );
    db.drop_database().await;
}
