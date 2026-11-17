//! The leased notification outbox (D-005, D-020, invariant 2).
//!
//! A notifier claims due rows under a lease (`FOR UPDATE SKIP LOCKED`, so several notifiers
//! never take the same row), commits the claim, calls the provider with no transaction open,
//! then records the result — but only if it still holds the lease. The attempt is counted at
//! claim time, so a crash mid-attempt still uses one up and the D-020 limit holds across
//! restarts. A row whose lease expired without a recorded result had an uncertain attempt.

use std::time::Duration;

use intent_domain::{
    ids::{LeadId, OutboxId},
    outbox::{AttemptOutcome, Transition},
};
use sqlx::Row as _;
use uuid::Uuid;

use crate::{Db, DbError, classify};

/// A claimed row with what the notification needs.
#[derive(Clone)]
pub struct ClaimedNotification {
    /// The row id: the idempotency key.
    pub id: OutboxId,
    /// The lead.
    pub lead_id: LeadId,
    /// This attempt's number (1-based), already counted.
    pub attempt: u8,
    /// Operator-configured site name.
    pub site_name: String,
    /// Operator-configured recipient.
    pub notify_to: String,
    /// Lead fields.
    pub name: String,
    /// Lead fields.
    pub email: Option<String>,
    /// Lead fields.
    pub phone: Option<String>,
    /// Lead fields.
    pub service: Option<String>,
    /// Lead fields.
    pub area: Option<String>,
    /// Lead fields.
    pub message: String,
}

impl std::fmt::Debug for ClaimedNotification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimedNotification")
            .field("id", &self.id)
            .field("lead_id", &self.lead_id)
            .field("attempt", &self.attempt)
            .finish_non_exhaustive()
    }
}

/// One attempt's result, to record.
#[derive(Debug, Clone, Copy)]
pub struct AttemptRecord {
    /// The row.
    pub id: OutboxId,
    /// The lease holder that made the attempt.
    pub owner: Uuid,
    /// The attempt number the claim was given. Together with `owner` it fences the record: a
    /// late result from an earlier claim of the same row (even by the same process) no longer
    /// matches and is discarded.
    pub attempt: u8,
    /// How the attempt ended.
    pub outcome: AttemptOutcome,
    /// What the schedule decided.
    pub transition: Transition,
}

/// Whether the record was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordResult {
    /// Recorded.
    Applied,
    /// The lease had expired and the row was claimed again or swept; this result is discarded
    /// (the other holder's result counts).
    LeaseLost,
    /// This claim's result is already there: an earlier try committed it and only its answer
    /// was lost (an I/O error), so the retry has nothing left to do.
    AlreadyRecorded,
}

const CLAIM: &str = r"
WITH due AS (
    SELECT id FROM notification_outbox
    WHERE state = 'pending' AND next_attempt_at <= now() AND attempts < $3
      AND (lease_expires_at IS NULL OR lease_expires_at < now())
    ORDER BY next_attempt_at
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
UPDATE notification_outbox o
SET attempts = o.attempts + 1,
    uncertain_attempts = o.uncertain_attempts + CASE WHEN o.lease_owner IS NULL THEN 0 ELSE 1 END,
    last_error = CASE WHEN o.lease_owner IS NULL THEN o.last_error ELSE 'lease_expired' END,
    lease_owner = $1,
    lease_expires_at = now() + $4 * interval '1 millisecond'
FROM due, leads l, sites s
WHERE o.id = due.id AND l.id = o.lead_id AND s.id = l.site_id
RETURNING o.id, o.lead_id, o.attempts, s.name AS site_name, s.notify_to,
          l.name, l.email, l.phone, l.service, l.area, l.message";

const RECORD: &str = r"
UPDATE notification_outbox
SET state = $3,
    next_attempt_at = CASE WHEN $3 = 'pending' THEN now() + $4 * interval '1 millisecond'
                           ELSE next_attempt_at END,
    delivered_at = CASE WHEN $3 = 'delivered' THEN now() END,
    dead_at = CASE WHEN $3 = 'dead' THEN now() END,
    last_error = $5,
    uncertain_attempts = uncertain_attempts + $6,
    lease_owner = NULL,
    lease_expires_at = NULL
WHERE id = $1 AND lease_owner = $2 AND attempts = $7 AND state = 'pending'";

/// Rows with no attempt left that nobody holds: the last attempt's process died (its lease
/// expired) or its result never landed (no lease at all). The outcome is unknown and no
/// attempt is left, so they are dead and visible. A row whose last attempt is still running
/// (an unexpired lease) is left alone.
const SWEEP_EXHAUSTED: &str = r"
UPDATE notification_outbox
SET state = 'dead', dead_at = now(), last_error = 'lease_expired',
    uncertain_attempts = least(uncertain_attempts + 1, 8), lease_owner = NULL, lease_expires_at = NULL
WHERE state = 'pending' AND attempts >= $1
  AND (lease_expires_at IS NULL OR lease_expires_at < now())";

fn millis(duration: Duration) -> f64 {
    // Lease and retry delays are at most hours: exact in an f64.
    let ms = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
    f64::from(ms)
}

impl Db {
    /// Claims up to `limit` due rows for `owner` with a lease of `lease`.
    pub async fn claim_notifications(
        &self,
        owner: Uuid,
        limit: u16,
        max_attempts: u8,
        lease: Duration,
    ) -> Result<Vec<ClaimedNotification>, DbError> {
        let rows = sqlx::query(CLAIM)
            .bind(owner)
            .bind(i64::from(limit))
            .bind(i16::from(max_attempts))
            .bind(millis(lease))
            .fetch_all(&self.pool)
            .await
            .map_err(classify)?;
        rows.iter()
            .map(|row| -> Result<ClaimedNotification, sqlx::Error> {
                let attempts: i16 = row.try_get("attempts")?;
                Ok(ClaimedNotification {
                    id: OutboxId::from_uuid(row.try_get("id")?),
                    lead_id: LeadId::from_uuid(row.try_get("lead_id")?),
                    attempt: u8::try_from(attempts).unwrap_or(u8::MAX),
                    site_name: row.try_get("site_name")?,
                    notify_to: row.try_get("notify_to")?,
                    name: row.try_get("name")?,
                    email: row.try_get("email")?,
                    phone: row.try_get("phone")?,
                    service: row.try_get("service")?,
                    area: row.try_get("area")?,
                    message: row.try_get("message")?,
                })
            })
            .collect::<Result<_, _>>()
            .map_err(DbError::Other)
    }

    /// Records an attempt if `owner` still holds the lease.
    pub async fn record_attempt(&self, record: &AttemptRecord) -> Result<RecordResult, DbError> {
        let (state, delay) = match record.transition {
            Transition::Delivered => ("delivered", Duration::ZERO),
            Transition::RetryIn(delay) => ("pending", delay),
            Transition::Dead(_) => ("dead", Duration::ZERO),
        };
        let result = sqlx::query(RECORD)
            .bind(record.id.as_uuid())
            .bind(record.owner)
            .bind(state)
            .bind(millis(delay))
            .bind(record.outcome.error_code())
            .bind(i16::from(record.outcome.is_uncertain()))
            .bind(i16::from(record.attempt))
            .execute(&self.pool)
            .await
            .map_err(classify)?;
        if result.rows_affected() == 1 {
            return Ok(RecordResult::Applied);
        }
        // Nothing matched. Either this claim's own result is already there (the row still has
        // the attempt number this claim was given, is released, and was not swept for an
        // expired lease), or the row moved on without us.
        let ours: Option<bool> = sqlx::query_scalar(
            "SELECT attempts = $2 AND lease_owner IS NULL
                    AND last_error IS DISTINCT FROM 'lease_expired'
             FROM notification_outbox WHERE id = $1",
        )
        .bind(record.id.as_uuid())
        .bind(i16::from(record.attempt))
        .fetch_optional(&self.pool)
        .await
        .map_err(classify)?;
        Ok(if ours == Some(true) {
            RecordResult::AlreadyRecorded
        } else {
            RecordResult::LeaseLost
        })
    }

    /// Marks rows dead that died holding their last attempt. Returns how many.
    pub async fn sweep_exhausted(&self, max_attempts: u8) -> Result<u64, DbError> {
        sqlx::query(SWEEP_EXHAUSTED)
            .bind(i16::from(max_attempts))
            .execute(&self.pool)
            .await
            .map(|r| r.rows_affected())
            .map_err(classify)
    }

    /// Pending rows (the backlog), for readiness detail and metrics.
    pub async fn pending_notifications(&self) -> Result<i64, DbError> {
        sqlx::query_scalar("SELECT count(*) FROM notification_outbox WHERE state = 'pending'")
            .fetch_one(&self.pool)
            .await
            .map_err(classify)
    }
}
