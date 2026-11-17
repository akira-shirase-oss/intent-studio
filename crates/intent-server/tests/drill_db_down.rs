//! Drill `db-down` (ROADMAP I0): PostgreSQL is stopped in the middle of a run.
//!
//! Pass: while it is down intake answers 503 with `Retry-After`, never 201 or 200, and quickly;
//! readiness goes red; after restart readiness returns, every lead acknowledged before the stop
//! is present, every refused lead resent with the same submission id is stored exactly once, and
//! the outbox drains. The log holds no lead content.
//!
//! This stops the stack's Postgres container, so it lives alone in its own test binary (Cargo
//! runs test binaries one at a time). Run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "test code: a failed unwrap is a failed test, and a drill reads top to bottom as one scenario"
)]

mod support;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use intent_provider_fixture::Mode;
use support::{
    LeadSpec, Outcome, ProviderFixture, Server, TestDb, address_for,
    assert_acknowledged_leads_stored_once, assert_no_leaks, client, compose, drill_env,
    resend_until_acknowledged, site_for, submit,
};

const SITES: u32 = 50;

/// Starts Postgres again if the test fails while it is stopped.
struct RestartOnDrop;

impl Drop for RestartOnDrop {
    fn drop(&mut self) {
        compose(&["start", "postgres"]);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Up,
    Stopping,
    Down,
    Starting,
    Recovered,
}

async fn wait_for_readiness(
    client: &reqwest::Client,
    base: &str,
    ready: bool,
    limit: Duration,
) -> Duration {
    let start = Instant::now();
    loop {
        let status = client
            .get(format!("{base}/readyz"))
            .timeout(Duration::from_secs(1))
            .send()
            .await
            .map_or(0, |r| r.status().as_u16());
        if (status == 200) == ready {
            return start.elapsed();
        }
        assert!(
            start.elapsed() < limit,
            "readiness did not become {ready} within {limit:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "stops the stack's Postgres: run alone by scripts/drills.sh"]
async fn db_down_refuses_honestly_and_recovers_without_loss() {
    let db = TestDb::create(SITES).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let server = Server::start(env).await;
    let probe = client();

    let phase = Arc::new(Mutex::new(Phase::Up));
    let outcomes: Arc<Mutex<Vec<(Phase, Outcome)>>> = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let clock = Instant::now();
    // The load: 100 new leads a second, each with its own id, until told to stop.
    let loader = {
        let (phase, outcomes, stop, base) = (
            phase.clone(),
            outcomes.clone(),
            stop.clone(),
            server.base.clone(),
        );
        tokio::spawn(async move {
            let client = client();
            let mut tasks = tokio::task::JoinSet::new();
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            let mut n = 0_u64;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                interval.tick().await;
                let (client, base, phase, outcomes) = (
                    client.clone(),
                    base.clone(),
                    phase.clone(),
                    outcomes.clone(),
                );
                let at_start = *phase.lock().unwrap();
                tasks.spawn(async move {
                    let spec = LeadSpec::new("dbd", n);
                    let outcome = submit(
                        &client,
                        &base,
                        site_for(n, SITES),
                        &spec.json(),
                        Some(&address_for(n)),
                        n,
                        clock,
                    )
                    .await;
                    // A request counts as "while down" only if it started and finished down.
                    let at_end = *phase.lock().unwrap();
                    let label = if at_start == at_end {
                        at_end
                    } else {
                        Phase::Stopping
                    };
                    outcomes.lock().unwrap().push((label, outcome));
                });
                n += 1;
            }
            while tasks.join_next().await.is_some() {}
            n
        })
    };

    tokio::time::sleep(Duration::from_secs(3)).await;
    let guard = RestartOnDrop;
    *phase.lock().unwrap() = Phase::Stopping;
    compose(&["stop", "postgres"]);
    *phase.lock().unwrap() = Phase::Down;
    let red_after = wait_for_readiness(&probe, &server.base, false, Duration::from_secs(5)).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    *phase.lock().unwrap() = Phase::Starting;
    drop(guard); // starts Postgres
    let green_after = wait_for_readiness(&probe, &server.base, true, Duration::from_secs(60)).await;
    *phase.lock().unwrap() = Phase::Recovered;
    tokio::time::sleep(Duration::from_secs(3)).await;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let sent = loader.await.unwrap();
    println!(
        "db-down: {sent} submissions; readiness red {red_after:?} after the stop, green {green_after:?} after the start"
    );

    let outcomes = Arc::try_unwrap(outcomes).unwrap().into_inner().unwrap();
    let down: Vec<&Outcome> = outcomes
        .iter()
        .filter(|(p, _)| *p == Phase::Down)
        .map(|(_, o)| o)
        .collect();
    assert!(
        down.len() > 200,
        "the load ran while the database was down ({} requests)",
        down.len()
    );
    for outcome in &down {
        assert_eq!(outcome.status, Some(503), "while down: {outcome:?}");
        assert!(
            outcome.retry_after.is_some(),
            "503 without Retry-After: {outcome:?}"
        );
        assert!(
            matches!(outcome.code.as_deref(), Some("unavailable" | "timeout")),
            "{outcome:?}"
        );
    }
    let mut down_latency: Vec<Duration> = down.iter().map(|o| o.latency).collect();
    let p99 = support::percentile(&mut down_latency, 0.99);
    println!("db-down: refusals while down p99 {p99:?}");
    assert!(
        p99 < Duration::from_secs(2),
        "refusals must be quick, p99 {p99:?}"
    );
    for (phase, outcome) in &outcomes {
        assert!(
            outcome.acknowledged() || outcome.status == Some(503),
            "only 201/200 or an honest 503 ({phase:?}): {outcome:?}"
        );
    }
    let up_acked = outcomes
        .iter()
        .filter(|(p, o)| *p == Phase::Up && o.acknowledged())
        .count();
    let recovered_acked = outcomes
        .iter()
        .filter(|(p, o)| *p == Phase::Recovered && o.acknowledged())
        .count();
    assert!(
        up_acked > 200 && recovered_acked > 100,
        "intake worked before and after ({up_acked}, {recovered_acked})"
    );

    // Every acknowledged lead survived; every refused one, resent, is stored once.
    let all: Vec<Outcome> = outcomes.iter().map(|(_, o)| o.clone()).collect();
    let refused: Vec<u64> = {
        let acknowledged: std::collections::HashSet<u64> = all
            .iter()
            .filter(|o| o.acknowledged())
            .map(|o| o.n)
            .collect();
        let mut ns: Vec<u64> = all
            .iter()
            .map(|o| o.n)
            .filter(|n| !acknowledged.contains(n))
            .collect();
        ns.sort_unstable();
        ns.dedup();
        ns
    };
    let resent = resend_until_acknowledged(&server.base, "dbd", SITES, &refused).await;
    let mut every = all.clone();
    every.extend(resent);
    let acknowledged = assert_acknowledged_leads_stored_once(&db, "dbd", &every).await;
    assert_eq!(
        u64::try_from(acknowledged.len()).unwrap(),
        sent,
        "every submission is stored exactly once"
    );
    assert_eq!(
        db.scalar("SELECT count(*) FROM notification_outbox").await,
        i64::try_from(sent).unwrap()
    );
    db.wait_outbox_terminal(Duration::from_secs(60)).await;
    assert_eq!(
        db.scalar("SELECT count(*) FROM notification_outbox WHERE state = 'delivered'")
            .await,
        i64::try_from(sent).unwrap()
    );
    // At least once: a notification the provider took just as the database went away may be
    // sent again after recovery. Each such duplicate must be accounted for by an uncertain
    // attempt on its row (D-020), never silent.
    let duplicates = fixture.fixture.stats().duplicate_acceptances;
    let uncertain = db
        .scalar("SELECT coalesce(sum(uncertain_attempts), 0)::bigint FROM notification_outbox")
        .await;
    println!(
        "db-down: {duplicates} duplicate notification(s), {uncertain} uncertain attempt(s) recorded"
    );
    assert!(
        i64::try_from(duplicates).unwrap() <= uncertain,
        "{duplicates} duplicates but only {uncertain} uncertain attempts"
    );

    let log = server.log();
    assert!(
        log.contains("database unreachable"),
        "the outage is logged once as a state change"
    );
    assert_no_leaks(
        &log,
        &[
            "@example.test",
            "Drillname ",
            "+1 555 01",
            "Please quote job",
        ],
    );
    db.drop_database().await;
}
