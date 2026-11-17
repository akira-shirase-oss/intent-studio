//! Drill `kill-9`: the server is killed with SIGKILL in the middle of a burst of submissions
//! and notification attempts, then started again on the same database.
//!
//! Pass: every lead acknowledged before the kill is stored exactly once; every lead whose
//! answer was lost, resent with the same submission id, is stored exactly once; every lead has
//! its outbox row; the leases the dead process held expire and those rows are delivered; any
//! duplicate notification is accounted for by an uncertain attempt. Run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "test code: a failed unwrap is a failed test, and a drill reads top to bottom as one scenario"
)]

mod support;

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use intent_provider_fixture::Mode;
use support::{
    LeadSpec, Outcome, ProviderFixture, Server, TestDb, address_for,
    assert_acknowledged_leads_stored_once, assert_no_leaks, client, drill_env,
    resend_until_acknowledged, site_for, submit,
};

const SITES: u32 = 100;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn kill_9_mid_request_loses_no_acknowledged_lead() {
    let db = TestDb::create(SITES).await;
    // Slow acceptance keeps notification attempts in flight at the moment of the kill.
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let mut server = Server::start(env).await;

    // 64 clients submit back to back, each lead with its own id.
    let outcomes: Arc<Mutex<Vec<Outcome>>> = Arc::new(Mutex::new(Vec::new()));
    let next = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let clock = Instant::now();
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..64 {
        let (outcomes, next, stop, base) = (
            outcomes.clone(),
            next.clone(),
            stop.clone(),
            server.base.clone(),
        );
        workers.spawn(async move {
            let client = client();
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let n = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let spec = LeadSpec::new("kill", n);
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
                let failed = outcome.status.is_none();
                outcomes.lock().unwrap().push(outcome);
                if failed {
                    // The server is gone; stop this worker.
                    break;
                }
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let before_kill = outcomes
        .lock()
        .unwrap()
        .iter()
        .filter(|o| o.acknowledged())
        .count();
    server.kill();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    while workers.join_next().await.is_some() {}
    let outcomes = Arc::try_unwrap(outcomes).unwrap().into_inner().unwrap();
    let lost_answers = outcomes.iter().filter(|o| o.status.is_none()).count();
    println!(
        "kill-9: {} submissions, {before_kill} acknowledged before the kill, {lost_answers} lost answers",
        outcomes.len()
    );
    assert!(
        before_kill > 100,
        "the burst was running ({before_kill} acknowledged)"
    );
    assert!(lost_answers > 0, "the kill landed mid-request");

    // Before restarting: everything acknowledged is already durable.
    let acknowledged_before = assert_acknowledged_leads_stored_once(&db, "kill", &outcomes).await;
    assert!(acknowledged_before.len() >= before_kill);
    assert_eq!(
        db.scalar("SELECT count(*) FROM leads l WHERE NOT EXISTS (SELECT 1 FROM notification_outbox o WHERE o.lead_id = l.id)").await,
        0,
        "a lead never exists without its outbox row"
    );

    server.restart().await;
    let acknowledged: HashSet<u64> = outcomes
        .iter()
        .filter(|o| o.acknowledged())
        .map(|o| o.n)
        .collect();
    let mut unanswered: Vec<u64> = outcomes
        .iter()
        .map(|o| o.n)
        .filter(|n| !acknowledged.contains(n))
        .collect();
    unanswered.sort_unstable();
    unanswered.dedup();
    let resent = resend_until_acknowledged(&server.base, "kill", SITES, &unanswered).await;
    let replayed = resent.iter().filter(|o| o.duplicate).count();
    println!(
        "kill-9: {} resent after restart, {replayed} of them had been committed before the kill",
        resent.len()
    );
    let mut every = outcomes.clone();
    every.extend(resent);
    let stored = assert_acknowledged_leads_stored_once(&db, "kill", &every).await;
    let submitted: HashSet<u64> = outcomes.iter().map(|o| o.n).collect();
    assert_eq!(
        stored.len(),
        submitted.len(),
        "every submission is stored exactly once"
    );

    db.wait_outbox_terminal(Duration::from_secs(60)).await;
    let delivered = db
        .scalar("SELECT count(*) FROM notification_outbox WHERE state = 'delivered'")
        .await;
    assert_eq!(
        delivered,
        i64::try_from(stored.len()).unwrap(),
        "leases held by the killed process expired and were delivered"
    );
    let duplicates = fixture.fixture.stats().duplicate_acceptances;
    let uncertain = db
        .scalar("SELECT coalesce(sum(uncertain_attempts), 0)::bigint FROM notification_outbox")
        .await;
    println!("kill-9: {duplicates} duplicate notification(s), {uncertain} uncertain attempt(s)");
    assert!(i64::try_from(duplicates).unwrap() <= uncertain);

    assert_no_leaks(
        &server.log(),
        &[
            "@example.test",
            "Drillname ",
            "+1 555 01",
            "Please quote job",
        ],
    );
    db.drop_database().await;
}
