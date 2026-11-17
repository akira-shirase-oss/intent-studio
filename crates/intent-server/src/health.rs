//! Liveness, readiness and the database prober.
//!
//! Readiness means "a lead sent here now would be stored": the process has started, is not
//! draining, and its last database probe succeeded. The prober runs on its own interval with a
//! deadline, so a request never waits on a health check and a stopped database turns readiness
//! red within about one interval plus one probe deadline.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::Duration,
};

use intent_db::Db;
use tokio_util::sync::CancellationToken;

const STARTING: u8 = 0;
const READY: u8 = 1;
const DRAINING: u8 = 2;

/// Shared health flags.
#[derive(Debug, Clone)]
pub struct Health {
    lifecycle: Arc<AtomicU8>,
    database_up: Arc<AtomicBool>,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            lifecycle: Arc::new(AtomicU8::new(STARTING)),
            // Unknown until the first probe: refuse rather than guess.
            database_up: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Health {
    /// Starting → ready; a drain that already began is never undone.
    pub fn mark_ready(&self) {
        let _ =
            self.lifecycle
                .compare_exchange(STARTING, READY, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Readiness goes false for good.
    pub fn begin_drain(&self) {
        self.lifecycle.store(DRAINING, Ordering::SeqCst);
    }

    /// Whether the last database probe succeeded.
    pub fn database_up(&self) -> bool {
        self.database_up.load(Ordering::SeqCst)
    }

    /// The lifecycle name.
    pub fn lifecycle(&self) -> &'static str {
        match self.lifecycle.load(Ordering::SeqCst) {
            READY => "ready",
            DRAINING => "draining",
            _ => "starting",
        }
    }

    /// Whether traffic should be sent here.
    pub fn is_ready(&self) -> bool {
        self.lifecycle.load(Ordering::SeqCst) == READY && self.database_up()
    }

    fn set_database(&self, up: bool) {
        let was = self.database_up.swap(up, Ordering::SeqCst);
        if was != up {
            if up {
                tracing::info!("database reachable: intake open");
            } else {
                tracing::warn!("database unreachable: intake refuses with 503 until it is back");
            }
        }
    }
}

/// Probes the database every `interval` (each probe bounded by `deadline`) until cancelled.
pub async fn run_prober(
    db: Db,
    health: Health,
    interval: Duration,
    deadline: Duration,
    cancel: CancellationToken,
) {
    loop {
        let up = matches!(tokio::time::timeout(deadline, db.ping()).await, Ok(Ok(())));
        health.set_database(up);
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(interval) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_needs_both_the_lifecycle_and_the_database() {
        let health = Health::default();
        assert!(!health.is_ready());
        health.mark_ready();
        assert!(!health.is_ready(), "database not yet probed");
        health.set_database(true);
        assert!(health.is_ready());
        health.set_database(false);
        assert!(!health.is_ready());
    }

    #[test]
    fn draining_is_terminal() {
        let health = Health::default();
        health.set_database(true);
        health.begin_drain();
        health.mark_ready();
        assert_eq!(health.lifecycle(), "draining");
        assert!(!health.is_ready());
    }
}
