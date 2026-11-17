//! The outbox lease is fenced per claim (F3): a late result from an earlier claim can never
//! overwrite the current one, and no row can be left pending with no attempt left and no lease.
//!
//! Drives `intent-db` directly against the stack, replaying the exact sequence step by step.
//! Run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

mod support;

use std::time::Duration;

use intent_db::{AttemptRecord, Db, NewLead, PoolLimits, RecordResult, StoreOutcome};
use intent_domain::{
    ids::{LeadId, OutboxId},
    lead::LeadForm,
    outbox::{AttemptOutcome, Transition},
    submission::{StoredKey, fingerprint},
};
use support::{TestDb, site_for};
use uuid::Uuid;

const LEASE: Duration = Duration::from_millis(150);

async fn one_lead(db: &Db) -> OutboxId {
    let lead = LeadForm {
        submission_id: Some("fencing-lead-1".into()),
        name: Some("Fencing".into()),
        email: Some("fencing@example.test".into()),
        consent: Some(true),
        consent_notice: Some("privacy-v1".into()),
        ..LeadForm::default()
    }
    .validate()
    .unwrap();
    let key = StoredKey::client(lead.submission_id().unwrap());
    let outbox_id = OutboxId::from_uuid(Uuid::now_v7());
    let outcome = db
        .store_lead(&NewLead {
            id: LeadId::from_uuid(Uuid::now_v7()),
            outbox_id,
            site: intent_domain::ids::SiteId::from_uuid(site_for(0, 1)),
            key: &key,
            previous_window: None,
            fingerprint: fingerprint(&lead),
            lead: &lead,
            received_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .unwrap();
    assert!(matches!(outcome, StoreOutcome::Created(_)));
    outbox_id
}

async fn row(test: &TestDb) -> (String, i64, i64) {
    let mut conn = test.connect().await;
    sqlx::query_as(
        "SELECT state, attempts::bigint, uncertain_attempts::bigint FROM notification_outbox",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_late_result_from_an_expired_claim_cannot_overwrite_the_current_one() {
    let test = TestDb::create(1).await;
    let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
    let id = one_lead(&db).await;
    // One process: the same owner id for every claim, as the notifier uses.
    let owner = Uuid::now_v7();

    // Attempts 1 to 6 fail and are recorded with a zero delay.
    for attempt in 1..=6_u8 {
        let claimed = db.claim_notifications(owner, 10, 8, LEASE).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].attempt, attempt);
        let applied = db
            .record_attempt(&AttemptRecord {
                id,
                owner,
                attempt,
                outcome: AttemptOutcome::Unavailable,
                transition: Transition::RetryIn(Duration::ZERO),
            })
            .await
            .unwrap();
        assert_eq!(applied, RecordResult::Applied);
    }
    // Attempt 7 is claimed, then runs past its lease.
    let seventh = db.claim_notifications(owner, 10, 8, LEASE).await.unwrap();
    assert_eq!(seventh[0].attempt, 7);
    tokio::time::sleep(LEASE + Duration::from_millis(100)).await;
    // The same process claims the row again: attempt 8.
    let eighth = db
        .claim_notifications(owner, 10, 8, LEASE * 20)
        .await
        .unwrap();
    assert_eq!(eighth[0].attempt, 8);

    // Attempt 7's late failure arrives now. It must not count.
    let late = db
        .record_attempt(&AttemptRecord {
            id,
            owner,
            attempt: 7,
            outcome: AttemptOutcome::Unavailable,
            transition: Transition::RetryIn(Duration::from_millis(1)),
        })
        .await
        .unwrap();
    assert_eq!(
        late,
        RecordResult::LeaseLost,
        "a stale attempt-7 result was applied over attempt 8's claim"
    );
    // Attempt 8 was accepted by the provider: that result must stick.
    let current = db
        .record_attempt(&AttemptRecord {
            id,
            owner,
            attempt: 8,
            outcome: AttemptOutcome::Accepted,
            transition: Transition::Delivered,
        })
        .await
        .unwrap();
    assert_eq!(
        current,
        RecordResult::Applied,
        "attempt 8's accepted result was dropped"
    );
    let (state, attempts, uncertain) = row(&test).await;
    assert_eq!((state.as_str(), attempts, uncertain), ("delivered", 8, 1));
    db.close().await;
    test.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_row_with_no_attempt_left_and_no_lease_is_swept_dead() {
    let test = TestDb::create(1).await;
    let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
    one_lead(&db).await;
    // The shape the race above used to leave behind: pending, 8 attempts, no lease.
    let mut conn = test.connect().await;
    sqlx::query(
        "UPDATE notification_outbox SET attempts = 8, next_attempt_at = now() + interval '1 hour'",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    assert!(
        db.claim_notifications(Uuid::now_v7(), 10, 8, LEASE)
            .await
            .unwrap()
            .is_empty()
    );
    let swept = db.sweep_exhausted(8).await.unwrap();
    assert_eq!(
        swept, 1,
        "a pending row with no attempt left and no lease was not swept"
    );
    let (state, attempts, _) = row(&test).await;
    assert_eq!((state.as_str(), attempts), ("dead", 8));
    db.close().await;
    test.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_record_retried_after_its_answer_was_lost_is_not_called_a_lost_lease() {
    let test = TestDb::create(1).await;
    let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
    let id = one_lead(&db).await;
    let owner = Uuid::now_v7();
    let claimed = db
        .claim_notifications(owner, 10, 8, LEASE * 20)
        .await
        .unwrap();
    assert_eq!(claimed[0].attempt, 1);
    let record = AttemptRecord {
        id,
        owner,
        attempt: 1,
        outcome: AttemptOutcome::Unavailable,
        transition: Transition::RetryIn(Duration::from_secs(3600)),
    };
    // The first try commits, but its answer is lost (an I/O error): the notifier tries again.
    assert_eq!(
        db.record_attempt(&record).await.unwrap(),
        RecordResult::Applied
    );
    let again = db.record_attempt(&record).await.unwrap();
    assert_ne!(
        again,
        RecordResult::LeaseLost,
        "a result already recorded by the first try was reported as a lost lease"
    );
    let (state, attempts, uncertain) = row(&test).await;
    assert_eq!((state.as_str(), attempts, uncertain), ("pending", 1, 0));
    db.close().await;
    test.drop_database().await;
}
