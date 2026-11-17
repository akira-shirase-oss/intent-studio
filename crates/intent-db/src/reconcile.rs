//! Post-run reconciliation for drills and load runs: what the database holds for every
//! submission key with a given prefix, checked against what clients were told.

use std::collections::BTreeMap;

use sqlx::Row as _;
use uuid::Uuid;

use crate::{Db, DbError, classify};

/// Outbox rows of a run, by state and attempt count.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OutboxTally {
    /// `(state, attempts)` → rows.
    pub by_state_attempts: BTreeMap<(String, i16), i64>,
    /// Sum of uncertain attempts.
    pub uncertain_attempts: i64,
}

/// What the database holds for one run.
#[derive(Debug, Default, Clone)]
pub struct PrefixReport {
    /// Stored key → lead ids under it (more than one would be a duplicate logical lead; the
    /// unique constraint makes that impossible, and this report checks it anyway).
    pub leads: BTreeMap<String, Vec<Uuid>>,
    /// Leads with no outbox row (must be zero: they commit together).
    pub leads_without_outbox: i64,
    /// Leads with more than one outbox row.
    pub leads_with_several_outbox_rows: i64,
    /// The run's outbox rows.
    pub outbox: OutboxTally,
}

impl Db {
    /// Reports on every lead whose stored key starts with `c:<prefix>`.
    pub async fn reconcile_prefix(&self, prefix: &str) -> Result<PrefixReport, DbError> {
        let pattern = format!(
            "c:{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut report = PrefixReport::default();
        let rows = sqlx::query(
            "SELECT l.submission_key, l.id, count(o.id) AS outbox_rows
             FROM leads l LEFT JOIN notification_outbox o ON o.lead_id = l.id
             WHERE l.submission_key LIKE $1
             GROUP BY l.submission_key, l.id",
        )
        .bind(&pattern)
        .fetch_all(&self.pool)
        .await
        .map_err(classify)?;
        for row in &rows {
            let key: String = row.try_get("submission_key").map_err(DbError::Other)?;
            let id: Uuid = row.try_get("id").map_err(DbError::Other)?;
            let outbox_rows: i64 = row.try_get("outbox_rows").map_err(DbError::Other)?;
            match outbox_rows {
                0 => report.leads_without_outbox += 1,
                1 => {}
                _ => report.leads_with_several_outbox_rows += 1,
            }
            report.leads.entry(key).or_default().push(id);
        }
        let tally = sqlx::query(
            "SELECT o.state, o.attempts, count(*) AS n, sum(o.uncertain_attempts) AS uncertain
             FROM notification_outbox o JOIN leads l ON l.id = o.lead_id
             WHERE l.submission_key LIKE $1
             GROUP BY o.state, o.attempts",
        )
        .bind(&pattern)
        .fetch_all(&self.pool)
        .await
        .map_err(classify)?;
        for row in &tally {
            let state: String = row.try_get("state").map_err(DbError::Other)?;
            let attempts: i16 = row.try_get("attempts").map_err(DbError::Other)?;
            let n: i64 = row.try_get("n").map_err(DbError::Other)?;
            let uncertain: Option<i64> = row.try_get("uncertain").map_err(DbError::Other)?;
            report.outbox.by_state_attempts.insert((state, attempts), n);
            report.outbox.uncertain_attempts += uncertain.unwrap_or(0);
        }
        Ok(report)
    }
}
