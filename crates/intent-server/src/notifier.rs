//! The leased notifier (D-005, D-020, D-030, invariant 2).
//!
//! It claims due outbox rows under a lease, sends each through the provider with a deadline and
//! no transaction open, and records the schedule's decision only while it still holds the
//! lease. It never touches intake: a provider that refuses or hangs slows only this loop, and the
//! number of attempts in flight is bounded, so new leads keep being stored at full speed.

use std::{sync::Arc, time::Duration};

use intent_db::{AttemptRecord, ClaimedNotification, Db, RecordResult};
use intent_domain::{
    notification::{LeadContent, compose},
    outbox::{AttemptOutcome, RetrySchedule, Transition},
};
use intent_notify::{Notification, Provider};
use tokio::sync::Semaphore;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use crate::{app::App, metrics::Metrics};

/// Sweeps for rows that died holding their last attempt once every this many polls.
const SWEEP_EVERY: u32 = 20;

/// Everything the loop needs: the shared state (readiness, counters, settings), its own
/// database pool and the provider adapter.
pub struct Notifier {
    /// Shared state.
    pub app: App,
    /// The notifier's pool, separate from intake's.
    pub db: Db,
    /// The adapter.
    pub provider: Provider,
}

impl Notifier {
    /// Runs until `cancel` fires, then waits for attempts in flight (each bounded by its
    /// deadline). Unfinished leases simply expire and the rows are claimed again later.
    pub async fn run(self, cancel: CancellationToken) {
        let owner = Uuid::now_v7();
        let config = self.app.config.notify.clone();
        let schedule = RetrySchedule::scaled(config.retry_base);
        let permits = Arc::new(Semaphore::new(config.concurrency));
        let tracker = TaskTracker::new();
        let this = Arc::new(self);
        let mut polls: u32 = 0;
        tracing::info!(provider = this.provider.kind(), "notifier started");
        while !cancel.is_cancelled() {
            polls = polls.wrapping_add(1);
            if !this.app.health.database_up() {
                sleep_or_cancel(config.poll_interval, &cancel).await;
                continue;
            }
            if polls.is_multiple_of(SWEEP_EVERY) {
                match this.db.sweep_exhausted(schedule.max_attempts()).await {
                    Ok(0) => {}
                    Ok(n) => {
                        this.app
                            .metrics
                            .notify_swept_total
                            .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                        tracing::warn!(
                            rows = n,
                            "outbox rows died holding their last attempt: marked dead"
                        );
                    }
                    Err(error) => tracing::warn!(db = %error.summary(), "outbox sweep failed"),
                }
            }
            let available = permits.available_permits();
            if available == 0 {
                // Wait for a free slot before claiming, so no lease is held while queued.
                tokio::select! {
                    () = cancel.cancelled() => break,
                    permit = permits.clone().acquire_owned() => drop(permit),
                }
                continue;
            }
            let limit = u16::try_from(available)
                .unwrap_or(u16::MAX)
                .min(config.batch);
            // The notifier's own pool, like every other query here (D-034).
            let claimed = match this
                .db
                .claim_notifications(owner, limit, schedule.max_attempts(), config.lease)
                .await
            {
                Ok(rows) => rows,
                Err(error) => {
                    tracing::warn!(db = %error.summary(), "claiming notifications failed");
                    sleep_or_cancel(config.poll_interval, &cancel).await;
                    continue;
                }
            };
            let full_batch = claimed.len() == usize::from(limit);
            for row in claimed {
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    // Unreachable while this loop is the only acquirer; the lease expires and the
                    // row is claimed again if it ever happens.
                    break;
                };
                let this = this.clone();
                tracker.spawn(async move {
                    let _permit = permit;
                    this.attempt(owner, schedule, row).await;
                });
            }
            if !full_batch {
                sleep_or_cancel(config.poll_interval, &cancel).await;
            }
        }
        tracker.close();
        let drain = config.attempt_deadline + Duration::from_secs(2);
        if tokio::time::timeout(drain, tracker.wait()).await.is_err() {
            tracing::warn!(
                "notifier stopped with attempts still in flight; their leases will expire"
            );
        }
        tracing::info!("notifier stopped");
    }

    async fn attempt(&self, owner: Uuid, schedule: RetrySchedule, row: ClaimedNotification) {
        let text = compose(
            &row.site_name,
            row.lead_id,
            LeadContent {
                name: &row.name,
                email: row.email.as_deref(),
                phone: row.phone.as_deref(),
                service: row.service.as_deref(),
                area: row.area.as_deref(),
                message: &row.message,
            },
        );
        let notification = Notification {
            event_id: row.id,
            to: row.notify_to.clone(),
            subject: text.subject,
            text: text.body,
        };
        let outcome = self
            .provider
            .send(&notification, self.app.config.notify.attempt_deadline)
            .await;
        let metrics = &self.app.metrics;
        metrics.attempt(outcome);
        let transition = schedule.after(row.attempt, outcome, fastrand::f64());
        if matches!(transition, Transition::Dead(_)) {
            Metrics::inc(&metrics.notify_dead_total);
            tracing::warn!(outbox_id = %row.id, lead_id = %row.lead_id, attempt = row.attempt,
                outcome = outcome.error_code().unwrap_or("accepted"), "notification stopped: the lead stays in the inbox");
        } else if outcome != AttemptOutcome::Accepted {
            tracing::debug!(outbox_id = %row.id, attempt = row.attempt,
                outcome = outcome.error_code().unwrap_or("accepted"), "notification attempt failed; retry scheduled");
        }
        let record = AttemptRecord {
            id: row.id,
            owner,
            attempt: row.attempt,
            outcome,
            transition,
        };
        // Recording is retried for as long as the lease can still be ours (the attempt took at
        // most its deadline, so at least `lease - deadline` of the lease is left). If the
        // database stays away longer, the lease expires and the row is attempted again: at least
        // once, counted as uncertain then.
        let notify = &self.app.config.notify;
        let until = tokio::time::Instant::now()
            + notify
                .lease
                .saturating_sub(notify.attempt_deadline)
                .saturating_sub(Duration::from_millis(100));
        let mut pause = Duration::ZERO;
        let mut logged = false;
        loop {
            tokio::time::sleep(pause).await;
            match self.db.record_attempt(&record).await {
                Ok(RecordResult::Applied) => return,
                Ok(RecordResult::LeaseLost) => {
                    Metrics::inc(&metrics.notify_lease_lost_total);
                    tracing::warn!(outbox_id = %row.id, "attempt result discarded: the lease had expired");
                    return;
                }
                Err(error) => {
                    if !logged {
                        tracing::warn!(outbox_id = %row.id, db = %error.summary(), "recording an attempt failed; retrying while the lease lasts");
                        logged = true;
                    }
                }
            }
            pause = (pause * 2).clamp(Duration::from_millis(50), Duration::from_secs(1));
            if tokio::time::Instant::now() + pause >= until {
                tracing::warn!(outbox_id = %row.id, "gave up recording an attempt: the row will be retried after its lease");
                return;
            }
        }
    }
}

async fn sleep_or_cancel(duration: Duration, cancel: &CancellationToken) {
    tokio::select! {
        () = cancel.cancelled() => {}
        () = tokio::time::sleep(duration) => {}
    }
}
