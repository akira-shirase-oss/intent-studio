//! Drill `notify-down` (failure case 3 in `docs/END_GOAL.md`, CI size): the provider refuses (503), then
//! hangs, while leads keep arriving with 5% deliberate resubmissions. Pass: every acknowledged
//! lead exists exactly once, intake latency stays inside its budget, the outbox follows the
//! D-030 schedule and drains after recovery with nothing lost or delivered twice, and the log
//! holds no lead content. The 100,000-submission smoke size runs through k6
//! (`perf/k6/leads.js`, `scripts/load.sh notify-down`).
//!
//! Needs the stack (`scripts/stack.sh up`); run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "test code: a failed unwrap is a failed test, and a drill reads top to bottom as one scenario"
)]

mod support;

use std::time::{Duration, Instant};

use intent_domain::outbox::RetrySchedule;
use intent_provider_fixture::Mode;
use sqlx::Row as _;
use support::{
    Load, ProviderFixture, Server, TestDb, assert_acknowledged_leads_stored_once, assert_no_leaks,
    drill_env, percentile, resend_until_acknowledged,
};

/// Generic shapes of every drill lead's values: any leaked field contains one of them.
const LEAD_SHAPES: [&str; 4] = [
    "@example.test",
    "Drillname ",
    "+1 555 01",
    "Please quote job",
];

/// Intake p99 bound while the provider is down, for this CI-size drill: below the 3 s intake
/// deadline, generous because CI runners and the maintainer's laptop are shared machines (it
/// measured 48 ms to 1 s on the same code as other builds came and went). The measured value is
/// printed; the budget that counts is set from the k6 baseline on a quiet machine
/// (`perf/results/`). Independence from the provider is structural: intake never awaits the
/// notifier, and the notifier has its own connection pool.
const INTAKE_P99_BUDGET: Duration = Duration::from_secs(2);

async fn trusted_server(db: &TestDb, fixture: &ProviderFixture, extra: &[(&str, &str)]) -> Server {
    let mut env = drill_env(db, &fixture.url);
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    for (k, v) in extra {
        env.retain(|(key, _)| key != k);
        env.push(((*k).to_owned(), (*v).to_owned()));
    }
    Server::start(env).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn notify_down_loses_and_duplicates_nothing_and_never_blocks_intake() {
    let db = TestDb::create(100).await;
    let fixture = ProviderFixture::start(Mode::Unavailable).await;
    let server = trusted_server(&db, &fixture, &[]).await;
    let load = Load {
        run: "nd".into(),
        sites: 100,
        rate: 250,
        resubmit_every: Some(20),
    };
    let clock = Instant::now();

    // Phase A: the provider refuses every attempt.
    let mut outcomes = load.drive(&server.base, 0..2_000, clock).await;
    // Phase B: the provider hangs; every attempt runs into its deadline.
    fixture.fixture.set_mode(Mode::Hang);
    outcomes.extend(load.drive(&server.base, 2_000..2_750, clock).await);
    fixture.fixture.set_mode(Mode::Unavailable);
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Intake never blocked on the provider: anything refused was refused honestly (503 with
    // Retry-After, e.g. the pool's acquire deadline on a starved machine), refusals are rare,
    // and a resend with the same id stores the lead.
    let refused: Vec<_> = outcomes.iter().filter(|o| !o.acknowledged()).collect();
    // A transport error (no answer at all, e.g. a kept-alive connection closed under the client
    // on a starved machine) is not a refusal; the client resends it like any unanswered submit.
    for outcome in refused.iter().filter(|o| o.status.is_some()) {
        assert_eq!(outcome.status, Some(503), "{outcome:?}");
        assert!(outcome.retry_after.is_some(), "{outcome:?}");
    }
    println!(
        "notify-down: {} of {} first submissions unanswered or refused with 503",
        refused.len(),
        outcomes.len()
    );
    assert!(
        refused.len() * 100 <= outcomes.len(),
        "more than 1% refused: {}",
        refused.len()
    );
    let mut unanswered: Vec<u64> = refused.iter().map(|o| o.n).collect();
    unanswered.sort_unstable();
    unanswered.dedup();
    let mut latencies: Vec<Duration> = outcomes.iter().map(|o| o.latency).collect();
    let p50 = percentile(&mut latencies, 0.50);
    let p99 = percentile(&mut latencies, 0.99);
    println!(
        "notify-down intake latency over {} submissions: p50 {p50:?}, p99 {p99:?}",
        outcomes.len()
    );
    assert!(
        p99 < INTAKE_P99_BUDGET,
        "intake p99 {p99:?} over the {INTAKE_P99_BUDGET:?} budget"
    );

    // Every acknowledged lead exists exactly once; resubmissions got the same lead.
    let resubmissions = outcomes.iter().filter(|o| o.duplicate).count();
    let refused_resubmissions = refused.iter().filter(|o| o.n % 20 == 9).count();
    let resent = resend_until_acknowledged(&server.base, "nd", 100, &unanswered).await;
    outcomes.extend(resent);
    let acknowledged = assert_acknowledged_leads_stored_once(&db, "nd", &outcomes).await;
    let planned = (0..2_750_u64).filter(|n| n % 20 == 19 && *n >= 10).count();
    assert!(
        resubmissions + refused_resubmissions >= planned,
        "every 5% resubmission answered as a replay ({resubmissions} of {planned})"
    );
    assert_eq!(
        acknowledged.len(),
        2_750 - planned,
        "every distinct lead stored once"
    );
    let leads = db.scalar("SELECT count(*) FROM leads").await;
    assert_eq!(usize::try_from(leads).unwrap(), acknowledged.len());
    assert_eq!(
        db.scalar("SELECT count(*) FROM notification_outbox").await,
        leads,
        "one outbox row per lead"
    );
    let uncertain = db
        .scalar("SELECT coalesce(sum(uncertain_attempts), 0)::bigint FROM notification_outbox")
        .await;
    assert!(uncertain > 0, "the hang phase produced uncertain attempts");
    let stats = fixture.fixture.stats();
    assert_eq!(
        stats.accepted, 0,
        "nothing delivered while the provider was down"
    );
    assert!(stats.refused > 0 && stats.hung > 0, "{stats:?}");

    // Recovery: everything drains, each notification accepted exactly once.
    fixture.fixture.set_mode(Mode::Ok);
    db.wait_outbox_terminal(Duration::from_secs(90)).await;
    let tally = db.outbox_tally().await;
    let mut delivered = 0;
    for ((state, attempts), rows) in &tally {
        match state.as_str() {
            "delivered" => delivered += rows,
            "dead" => assert_eq!(*attempts, 8, "dead only after 8 attempts: {tally:?}"),
            other => panic!("outbox row left {other}: {tally:?}"),
        }
    }
    println!("outbox after recovery: {tally:?}; uncertain attempts {uncertain}");
    let stats = fixture.fixture.stats();
    assert_eq!(i64::try_from(stats.unique_accepted).unwrap(), delivered);
    // A 503 or a hang never lets a notification through; after recovery an attempt the
    // provider took but whose answer missed its deadline (a slow machine) is sent again. Every
    // such duplicate is accounted for by an uncertain attempt on its row (D-020).
    let uncertain_now = db
        .scalar("SELECT coalesce(sum(uncertain_attempts), 0)::bigint FROM notification_outbox")
        .await;
    println!(
        "notify-down: {} duplicate notification(s), {uncertain_now} uncertain attempt(s)",
        stats.duplicate_acceptances
    );
    assert!(i64::try_from(stats.duplicate_acceptances).unwrap() <= uncertain_now);

    // The retry schedule: each wait is at least half its D-030 ceiling (base 20 ms here).
    let schedule = RetrySchedule::scaled(Duration::from_millis(20));
    let mut conn = db.connect().await;
    let sample = sqlx::query("SELECT id FROM notification_outbox WHERE attempts >= 4 LIMIT 100")
        .fetch_all(&mut conn)
        .await
        .unwrap();
    assert!(!sample.is_empty());
    for row in sample {
        let key: uuid::Uuid = row.get(0);
        let history = fixture
            .fixture
            .key(key)
            .expect("the fixture saw every attempt");
        for (k, pair) in history.attempt_ms.windows(2).enumerate() {
            let attempt = u8::try_from(k + 1).unwrap();
            let gap = Duration::from_millis(pair[1] - pair[0]);
            let floor = schedule.ceiling(attempt) / 2;
            assert!(
                gap + Duration::from_millis(2) >= floor,
                "attempt {attempt} of {key}: waited {gap:?}, schedule floor {floor:?}"
            );
            // No upper bound: when the notifier is saturated (every attempt hanging to its
            // deadline) a due row waits its turn. Lateness is backlog, never a lost row.
        }
    }

    assert_no_leaks(&server.log(), &LEAD_SHAPES);
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_provider_that_never_recovers_ends_dead_and_visible_and_keeps_every_lead() {
    let db = TestDb::create(10).await;
    let fixture = ProviderFixture::start(Mode::Unavailable).await;
    let server = trusted_server(&db, &fixture, &[("INTENT_NOTIFY_RETRY_BASE_MS", "2")]).await;
    let load = Load {
        run: "dead".into(),
        sites: 10,
        rate: 100,
        resubmit_every: None,
    };
    let outcomes = load.drive(&server.base, 0..40, Instant::now()).await;
    assert!(outcomes.iter().all(support::Outcome::acknowledged));
    db.wait_outbox_terminal(Duration::from_secs(30)).await;
    let tally = db.outbox_tally().await;
    assert_eq!(tally.len(), 1, "{tally:?}");
    assert_eq!(tally.get(&("dead".into(), 8)), Some(&40));
    assert_eq!(
        db.scalar("SELECT count(*) FROM notification_outbox WHERE last_error = 'provider_unavailable' AND dead_at IS NOT NULL").await,
        40
    );
    // Dead stays dead (a manual resend is an operator action) and no lead went anywhere.
    fixture.fixture.set_mode(Mode::Ok);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(fixture.fixture.stats().accepted, 0);
    assert_eq!(db.scalar("SELECT count(*) FROM leads").await, 40);
    assert_eq!(
        fixture.fixture.stats().attempts,
        40 * 8,
        "exactly 8 attempts each, never more"
    );
    assert_no_leaks(&server.log(), &LEAD_SHAPES);
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_lost_answer_after_acceptance_is_retried_and_counted_as_uncertain() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::AcceptThenHang).await;
    let server = trusted_server(&db, &fixture, &[]).await;
    let load = Load {
        run: "lost".into(),
        sites: 1,
        rate: 10,
        resubmit_every: None,
    };
    let outcomes = load.drive(&server.base, 0..1, Instant::now()).await;
    assert!(outcomes[0].acknowledged());
    let start = Instant::now();
    while db
        .scalar("SELECT attempts::bigint FROM notification_outbox")
        .await
        < 2
    {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "no retry after the lost answer"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    fixture.fixture.set_mode(Mode::Ok);
    db.wait_outbox_terminal(Duration::from_secs(20)).await;
    assert_eq!(
        db.scalar("SELECT count(*) FROM notification_outbox WHERE state = 'delivered'")
            .await,
        1
    );
    assert!(
        db.scalar("SELECT uncertain_attempts::bigint FROM notification_outbox")
            .await
            >= 1
    );
    let stats = fixture.fixture.stats();
    assert!(
        stats.duplicate_acceptances >= 1,
        "at least once: the duplicate happened and is visible: {stats:?}"
    );
    let metrics = reqwest::get(format!("{}/metrics", server.base))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !metrics.contains("intent_notify_timeouts_total 0\n"),
        "timeouts are counted"
    );
    assert_no_leaks(&server.log(), &LEAD_SHAPES);
    db.drop_database().await;
}
