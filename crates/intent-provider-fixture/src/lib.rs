//! The provider-down harness (failure case 3 in `docs/END_GOAL.md`).
//!
//! It speaks the webhook protocol `intent-notify` sends (`POST /v1/notify`, JSON, an
//! `Idempotency-Key` header) and can be switched at runtime between:
//!
//! - `ok`: accept (202);
//! - `unavailable`: refuse with 503 — the provider did not take it;
//! - `hang`: never answer (until `hang_limit`), so every attempt times out;
//! - `accept_then_hang`: record the notification as accepted, then never answer — the lost
//!   response that makes an at-least-once duplicate possible (D-020).
//!
//! It counts attempts and acceptances per idempotency key (bounded), so a drill can check the
//! retry schedule and count duplicates. It stores no recipient and no body: only keys, attempt
//! times and the last few subjects (which by design carry no submitted value).

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How the fixture answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// 202.
    Ok,
    /// 503.
    Unavailable,
    /// No answer.
    Hang,
    /// Counted as accepted, then no answer.
    AcceptThenHang,
}

impl Mode {
    const fn to_u8(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Unavailable => 1,
            Self::Hang => 2,
            Self::AcceptThenHang => 3,
        }
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Unavailable,
            2 => Self::Hang,
            3 => Self::AcceptThenHang,
            _ => Self::Ok,
        }
    }
}

/// Keys tracked individually; beyond this, attempts are still counted in the totals.
const MAX_TRACKED_KEYS: usize = 2_000_000;
/// Attempt times kept per key.
const MAX_TIMES_PER_KEY: usize = 16;
/// Subjects kept.
const MAX_SUBJECTS: usize = 64;

#[derive(Default)]
struct KeyRecord {
    attempt_ms: Vec<u64>,
    accepted: u32,
}

#[derive(Default)]
struct Ledger {
    attempts: u64,
    accepted: u64,
    refused: u64,
    hung: u64,
    keys: HashMap<Uuid, KeyRecord>,
    untracked: u64,
    subjects: VecDeque<String>,
}

/// Shared fixture state; cheap to clone.
#[derive(Clone)]
pub struct Fixture {
    mode: Arc<AtomicU8>,
    started: Instant,
    hang_limit: Duration,
    ledger: Arc<Mutex<Ledger>>,
}

/// Totals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// The current mode.
    pub mode: Mode,
    /// Requests received.
    pub attempts: u64,
    /// Requests counted as accepted (including duplicates).
    pub accepted: u64,
    /// Requests refused with 503.
    pub refused: u64,
    /// Requests left hanging.
    pub hung: u64,
    /// Distinct keys accepted at least once.
    pub unique_accepted: u64,
    /// Acceptances beyond the first per key: duplicate notifications.
    pub duplicate_acceptances: u64,
    /// Keys not tracked individually because the table was full.
    pub untracked: u64,
}

/// One key's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyHistory {
    /// Milliseconds since the fixture started, per attempt (first 16).
    pub attempt_ms: Vec<u64>,
    /// Times accepted.
    pub accepted: u32,
}

#[derive(Deserialize)]
struct NotifyBody {
    event_id: Uuid,
    to: String,
    subject: String,
    text: String,
}

#[derive(Deserialize)]
struct ModeBody {
    mode: Mode,
}

impl Fixture {
    /// A fixture in `mode`; hanging requests are released after `hang_limit`.
    pub fn new(mode: Mode, hang_limit: Duration) -> Self {
        Self {
            mode: Arc::new(AtomicU8::new(mode.to_u8())),
            started: Instant::now(),
            hang_limit,
            ledger: Arc::new(Mutex::new(Ledger::default())),
        }
    }

    /// Switches the mode.
    pub fn set_mode(&self, mode: Mode) {
        self.mode.store(mode.to_u8(), Ordering::SeqCst);
    }

    /// The mode.
    pub fn mode(&self) -> Mode {
        Mode::from_u8(self.mode.load(Ordering::SeqCst))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ledger> {
        // A panic while holding the lock cannot leave the counters inconsistent in a way that
        // matters to a test fixture; keep serving.
        self.ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Totals so far.
    pub fn stats(&self) -> Stats {
        let ledger = self.lock();
        let unique_accepted = ledger.keys.values().filter(|k| k.accepted > 0).count();
        let duplicate_acceptances = ledger
            .keys
            .values()
            .map(|k| u64::from(k.accepted.saturating_sub(1)))
            .sum();
        Stats {
            mode: self.mode(),
            attempts: ledger.attempts,
            accepted: ledger.accepted,
            refused: ledger.refused,
            hung: ledger.hung,
            unique_accepted: u64::try_from(unique_accepted).unwrap_or(u64::MAX),
            duplicate_acceptances,
            untracked: ledger.untracked,
        }
    }

    /// One key's history.
    pub fn key(&self, key: Uuid) -> Option<KeyHistory> {
        self.lock().keys.get(&key).map(|k| KeyHistory {
            attempt_ms: k.attempt_ms.clone(),
            accepted: k.accepted,
        })
    }

    /// The last subjects received.
    pub fn subjects(&self) -> Vec<String> {
        self.lock().subjects.iter().cloned().collect()
    }

    /// Forgets everything.
    pub fn reset(&self) {
        *self.lock() = Ledger::default();
    }

    fn record(&self, key: Uuid, subject: &str, accepted: bool, outcome: Mode) {
        let now_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut ledger = self.lock();
        ledger.attempts += 1;
        match outcome {
            Mode::Unavailable => ledger.refused += 1,
            Mode::Hang | Mode::AcceptThenHang => ledger.hung += 1,
            Mode::Ok => {}
        }
        if accepted {
            ledger.accepted += 1;
        }
        if ledger.subjects.len() == MAX_SUBJECTS {
            ledger.subjects.pop_front();
        }
        ledger
            .subjects
            .push_back(subject.chars().take(200).collect());
        let tracked = ledger.keys.len();
        match ledger.keys.get_mut(&key) {
            Some(record) => {
                if record.attempt_ms.len() < MAX_TIMES_PER_KEY {
                    record.attempt_ms.push(now_ms);
                }
                record.accepted += u32::from(accepted);
            }
            None if tracked < MAX_TRACKED_KEYS => {
                ledger.keys.insert(
                    key,
                    KeyRecord {
                        attempt_ms: vec![now_ms],
                        accepted: u32::from(accepted),
                    },
                );
            }
            None => ledger.untracked += 1,
        }
    }
}

async fn notify(
    State(fixture): State<Fixture>,
    headers: HeaderMap,
    Json(body): Json<NotifyBody>,
) -> StatusCode {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok());
    let Some(key) = key.filter(|k| *k == body.event_id) else {
        return StatusCode::BAD_REQUEST;
    };
    if body.to.is_empty() || body.text.is_empty() {
        return StatusCode::UNPROCESSABLE_ENTITY;
    }
    let mode = fixture.mode();
    match mode {
        Mode::Ok => {
            fixture.record(key, &body.subject, true, mode);
            StatusCode::ACCEPTED
        }
        Mode::Unavailable => {
            fixture.record(key, &body.subject, false, mode);
            StatusCode::SERVICE_UNAVAILABLE
        }
        Mode::Hang | Mode::AcceptThenHang => {
            fixture.record(key, &body.subject, mode == Mode::AcceptThenHang, mode);
            tokio::time::sleep(fixture.hang_limit).await;
            StatusCode::GATEWAY_TIMEOUT
        }
    }
}

/// The fixture's routes.
pub fn router(fixture: Fixture) -> Router {
    Router::new()
        .route("/v1/notify", post(notify))
        .route(
            "/control/mode",
            put(
                |State(f): State<Fixture>, Json(body): Json<ModeBody>| async move {
                    f.set_mode(body.mode);
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .route(
            "/control/stats",
            get(|State(f): State<Fixture>| async move { Json(f.stats()) }),
        )
        .route(
            "/control/keys/{key}",
            get(
                |State(f): State<Fixture>, Path(key): Path<Uuid>| async move {
                    f.key(key).map(Json).ok_or(StatusCode::NOT_FOUND)
                },
            ),
        )
        .route(
            "/control/subjects",
            get(|State(f): State<Fixture>| async move { Json(f.subjects()) }),
        )
        .route(
            "/control/reset",
            post(|State(f): State<Fixture>| async move {
                f.reset();
                StatusCode::NO_CONTENT
            }),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(fixture)
}

/// Serves the fixture on `listener` until `shutdown` resolves.
pub async fn serve(
    listener: tokio::net::TcpListener,
    fixture: Fixture,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router(fixture))
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip() {
        for mode in [
            Mode::Ok,
            Mode::Unavailable,
            Mode::Hang,
            Mode::AcceptThenHang,
        ] {
            assert_eq!(Mode::from_u8(mode.to_u8()), mode);
        }
    }

    #[test]
    fn duplicates_are_acceptances_beyond_the_first() {
        let fixture = Fixture::new(Mode::Ok, Duration::from_secs(1));
        let key = Uuid::from_u128(7);
        fixture.record(key, "s", false, Mode::Unavailable);
        fixture.record(key, "s", true, Mode::AcceptThenHang);
        fixture.record(key, "s", true, Mode::Ok);
        let stats = fixture.stats();
        assert_eq!(stats.attempts, 3);
        assert_eq!(stats.refused, 1);
        assert_eq!(stats.hung, 1);
        assert_eq!(stats.unique_accepted, 1);
        assert_eq!(stats.duplicate_acceptances, 1);
        assert_eq!(fixture.key(key).unwrap().attempt_ms.len(), 3);
    }
}
