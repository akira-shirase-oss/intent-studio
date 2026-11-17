//! Intent Studio's PostgreSQL layer. PostgreSQL is the only authority (invariant 1).
//!
//! - [`Db::store_lead`] commits a lead and its notification outbox row in **one statement**, so
//!   they exist together or not at all, and only a committed lead is ever acknowledged
//!   (invariant 5).
//! - [`Db::claim_notifications`] / [`Db::record_attempt`] run the outbox under leases; no
//!   transaction is ever open across the provider call (invariant 2).
//! - Every query has a statement timeout and the pool an acquire timeout, so a slow or stopped
//!   database turns into a bounded, typed [`DbError::Unavailable`], never a hang.
//!
//! - [`Db::save_brief`], [`Db::approve_fact`] and the other studio writes commit with their audit
//!   row in one transaction and check the rows they depend on inside it (I1).
//!
//! Errors never carry lead content: Postgres's `DETAIL` (which can echo row values) is not part
//! of any message this crate formats, and [`DbError::summary`] gives a log-safe line.

mod assets;
mod inbox;
mod leads;
mod outbox;
mod reconcile;
mod seed;
mod sessions;
mod studio;

use std::time::Duration;

use sqlx::{
    ConnectOptions as _,
    postgres::{PgConnectOptions, PgPool, PgPoolOptions},
};

pub use assets::{AssetRow, NewAsset, NewSnapshot, SnapshotRow};
pub use inbox::{AuditEntry, InboxLead, InboxQuery};
pub use leads::{NewLead, StoreOutcome};
pub use outbox::{AttemptRecord, ClaimedNotification, RecordResult};
pub use reconcile::{OutboxTally, PrefixReport};
pub use seed::{DEMO_TENANT, demo_site};
pub use studio::{BriefRow, BriefSave, BriefSummary, FactChange, FactRow, LIST_MAX, TenantRow};

/// Embedded migrations (`crates/intent-db/migrations`).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Pool and statement limits.
#[derive(Debug, Clone, Copy)]
pub struct PoolLimits {
    /// Most open connections.
    pub max_connections: u32,
    /// Longest wait for a connection before the call fails as unavailable.
    pub acquire_timeout: Duration,
    /// Server-side limit on any one statement.
    pub statement_timeout: Duration,
}

impl Default for PoolLimits {
    fn default() -> Self {
        Self {
            max_connections: 16,
            acquire_timeout: Duration::from_secs(1),
            statement_timeout: Duration::from_secs(3),
        }
    }
}

/// A database error, classified for the caller.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// The database cannot be reached or is not answering in time: connection refused or lost,
    /// pool timeout, server shutting down, statement timeout, resource exhaustion, or a
    /// serialization failure. Nothing can be assumed committed; retrying later is safe.
    #[error("database unavailable")]
    Unavailable(#[source] sqlx::Error),
    /// Anything else: a defect, or data the schema refused.
    #[error("database error")]
    Other(#[source] sqlx::Error),
    /// Applying migrations failed.
    #[error("migration failed")]
    Migrate(#[source] sqlx::migrate::MigrateError),
}

impl DbError {
    /// Whether the database is (for now) out of reach.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }

    /// The SQLSTATE, when the error came from the server.
    pub fn sqlstate(&self) -> Option<String> {
        match self {
            Self::Unavailable(error) | Self::Other(error) => error
                .as_database_error()
                .and_then(sqlx::error::DatabaseError::code)
                .map(std::borrow::Cow::into_owned),
            Self::Migrate(_) => None,
        }
    }

    /// One log-safe line: the class, the SQLSTATE and the constraint, never row values.
    pub fn summary(&self) -> String {
        let (kind, error) = match self {
            Self::Unavailable(error) => ("unavailable", error),
            Self::Other(error) => ("other", error),
            Self::Migrate(error) => return format!("kind=migrate error={error}"),
        };
        match error.as_database_error() {
            Some(db) => format!(
                "kind={kind} sqlstate={} constraint={}",
                db.code().as_deref().unwrap_or("?"),
                db.constraint().unwrap_or("-")
            ),
            None => format!("kind={kind} error={}", client_side(error)),
        }
    }
}

/// A fixed description of a client-side sqlx error (no driver text that could carry values).
fn client_side(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::PoolTimedOut => "pool_timed_out",
        sqlx::Error::PoolClosed => "pool_closed",
        sqlx::Error::Io(_) => "io",
        sqlx::Error::Tls(_) => "tls",
        sqlx::Error::Protocol(_) => "protocol",
        sqlx::Error::WorkerCrashed => "worker_crashed",
        sqlx::Error::RowNotFound => "row_not_found",
        sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => "decode",
        sqlx::Error::Configuration(_) => "configuration",
        _ => "driver",
    }
}

/// Sorts a sqlx error into [`DbError`].
pub(crate) fn classify(error: sqlx::Error) -> DbError {
    let unavailable = match &error {
        sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::WorkerCrashed
        | sqlx::Error::Protocol(_) => true,
        sqlx::Error::Database(db) => db.code().is_some_and(|code| {
            // 08: connection exception; 53: insufficient resources; 57014: statement timeout;
            // 57P01..57P03: shutting down / crashed / cannot connect now; 40001 and 40P01:
            // serialization failure and deadlock (retry).
            code.starts_with("08")
                || code.starts_with("53")
                || matches!(
                    &*code,
                    "57014" | "57P01" | "57P02" | "57P03" | "40001" | "40P01"
                )
        }),
        _ => false,
    };
    if unavailable {
        DbError::Unavailable(error)
    } else {
        DbError::Other(error)
    }
}

/// The database handle: a bounded pool.
#[derive(Debug, Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Builds the pool. Connections open lazily, so this succeeds while the database is down;
    /// the first query reports [`DbError::Unavailable`] until it is back.
    pub fn connect_lazy(url: &str, limits: PoolLimits) -> Result<Self, DbError> {
        let options: PgConnectOptions = url.parse().map_err(DbError::Other)?;
        let statement_ms = limits.statement_timeout.as_millis().to_string();
        let options = options
            .application_name("intent-server")
            .options([
                ("statement_timeout", statement_ms.as_str()),
                ("idle_in_transaction_session_timeout", "10000"),
            ])
            // Statements are never logged: bind values are lead content.
            .disable_statement_logging();
        let pool = PgPoolOptions::new()
            .max_connections(limits.max_connections)
            .min_connections(0)
            .acquire_timeout(limits.acquire_timeout)
            .idle_timeout(Duration::from_secs(300))
            .connect_lazy_with(options);
        Ok(Self { pool })
    }

    /// Applies pending migrations.
    pub async fn migrate(&self) -> Result<(), DbError> {
        MIGRATOR.run(&self.pool).await.map_err(DbError::Migrate)
    }

    /// One round trip; used by the readiness prober.
    pub async fn ping(&self) -> Result<(), DbError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(classify)
    }

    /// Closes every connection (shutdown).
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// The pool, for tests and tools in this workspace.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}
