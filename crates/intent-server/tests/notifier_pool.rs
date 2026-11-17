//! The notifier uses only its own connection pool (F4, D-034): with intake's pool pointed at
//! nothing at all, the notifier still claims, sends and records. Run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

mod support;

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use intent_db::{Db, PoolLimits};
use intent_notify::{Provider, Recorder};
use intent_server::{app::App, config::Config, health::Health, notifier::Notifier};
use support::{LeadSpec, TestDb, site_for};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn the_notifier_never_touches_intakes_pool() {
    let test = TestDb::create(1).await;
    // One stored lead, through the real server, so the outbox row is genuine.
    let mut server = support::Server::start(support::drill_env(
        &test,
        "http://127.0.0.1:59499/v1/notify",
    ))
    .await;
    let outcome = support::submit(
        &support::client(),
        &server.base,
        site_for(0, 1),
        &LeadSpec::new("pool", 1).json(),
        None,
        1,
        Instant::now(),
    )
    .await;
    assert_eq!(outcome.status, Some(201));
    server.terminate();

    let env: HashMap<&str, String> = HashMap::from([
        ("INTENT_DATABASE_URL", test.url.clone()),
        ("INTENT_NOTIFY_PROVIDER", "recorder".to_owned()),
        ("INTENT_PROFILE", "development".to_owned()),
        ("INTENT_NOTIFY_POLL_MS", "20".to_owned()),
    ]);
    let config = Config::from_lookup(|key| env.get(key).cloned()).unwrap();
    let short = PoolLimits {
        max_connections: 1,
        acquire_timeout: Duration::from_millis(200),
        statement_timeout: Duration::from_secs(1),
    };
    // Intake's pool points at a port where nothing listens (59499 is kept closed).
    let intake_db = Db::connect_lazy(
        "postgres://intent:intent-dev@127.0.0.1:59499/nothing",
        short,
    )
    .unwrap();
    let notifier_db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
    let health = Health::default();
    health.set_database(true);
    let studio = intent_server::serve::studio(&config).unwrap();
    let app = App::new(config, intake_db, health, studio);
    let recorder = Recorder::default();
    let cancel = CancellationToken::new();
    let running = tokio::spawn(
        Notifier {
            app,
            db: notifier_db,
            provider: Provider::Recorder(recorder.clone()),
        }
        .run(cancel.clone()),
    );
    let start = Instant::now();
    while recorder.accepted() == 0 {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "nothing delivered: the notifier used intake's (unreachable) pool"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cancel.cancel();
    running.await.unwrap();
    test.wait_outbox_terminal(Duration::from_secs(5)).await;
    assert_eq!(
        test.scalar("SELECT count(*) FROM notification_outbox WHERE state = 'delivered'")
            .await,
        1
    );
    test.drop_database().await;
}
