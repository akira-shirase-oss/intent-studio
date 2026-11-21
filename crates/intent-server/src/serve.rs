//! Startup and graceful shutdown (rust-mastery chapter 12, concept 10).
//!
//! Signals are installed before anything else, so a registration failure is a startup error.
//! On SIGTERM or SIGINT: readiness goes false, the listener keeps accepting for the drain delay,
//! then stops; in-flight requests get the grace period; then the notifier and the prober stop
//! (attempts in flight finish within their deadline or their leases simply expire) and the pool
//! closes. A lead is acknowledged only after its commit, so stopping at any point loses nothing
//! that was acknowledged.

use std::{future::Future, time::Duration};

use anyhow::Context as _;
use intent_db::Db;
use intent_notify::{Provider, Recorder, WebhookProvider};
use tokio::net::TcpListener;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    app::{self, App},
    config::{Config, NotifyConfig, ProviderConfig},
    health::{Health, run_prober},
    notifier::Notifier,
};

/// How serving ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// Every in-flight request finished.
    Graceful,
    /// The grace period ran out first.
    DeadlineExceeded,
}

/// Builds the configured provider adapter.
pub fn provider(config: &NotifyConfig) -> anyhow::Result<Provider> {
    Ok(match &config.provider {
        ProviderConfig::Webhook { url } => {
            Provider::Webhook(WebhookProvider::new(url).context("INTENT_NOTIFY_WEBHOOK_URL")?)
        }
        ProviderConfig::Recorder => Provider::Recorder(Recorder::default()),
    })
}

/// SIGTERM or SIGINT, registered now.
pub fn os_signal() -> std::io::Result<impl Future<Output = ()> + Send + 'static> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
    })
}

/// Opens the content store (proving it writable), checks the worker exists and builds the egress
/// guard. A configured store that cannot be used stops startup: a server that would accept an
/// upload and fail to keep it is worse than one that says so at once.
pub fn studio(config: &Config) -> anyhow::Result<crate::app::Studio> {
    let settings = &config.studio;
    let store = match &settings.store_dir {
        None => None,
        Some(dir) => {
            let store = intent_store::ContentStore::open(dir, intent_store::DEFAULT_MAX_BLOB_BYTES)
                .map_err(|e| anyhow::anyhow!("INTENT_STORE_DIR {}: {e}", dir.display()))?;
            anyhow::ensure!(
                settings.worker_bin.is_file(),
                "INTENT_WORKER_BIN: no worker at {} (build `intent-worker`, or set the path)",
                settings.worker_bin.display()
            );
            Some(store)
        }
    };
    let worker = intent_render::Driver::new(
        settings.worker_bin.clone(),
        settings.worker_timeout,
        settings.worker_concurrency,
    );
    let egress = intent_egress::Guard::system()
        .map_err(|e| anyhow::anyhow!("building the egress guard's TLS configuration: {e}"))?;
    Ok(crate::app::Studio {
        store,
        worker,
        egress,
    })
}

/// `check-config`'s studio pass: everything `studio()` proves *except* the live store lock,
/// which the running server holds through an upgrade — a held lock is the old server's, not
/// a configuration problem. The store root must exist or be creatable under a writable
/// ancestor and writable itself, the same proof `open` makes without the side effects.
pub fn check_studio(config: &Config) -> anyhow::Result<()> {
    let settings = &config.studio;
    if let Some(dir) = &settings.store_dir {
        check_store_dir(dir)?;
        anyhow::ensure!(
            settings.worker_bin.is_file(),
            "INTENT_WORKER_BIN: no worker at {} (build `intent-worker`, or set the path)",
            settings.worker_bin.display()
        );
    }
    intent_egress::Guard::system()
        .map_err(|e| anyhow::anyhow!("building the egress guard's TLS configuration: {e}"))?;
    Ok(())
}

/// The store-root half of [`check_studio`].
fn check_store_dir(dir: &std::path::Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        !dir.exists() || dir.is_dir(),
        "INTENT_STORE_DIR {}: not a directory",
        dir.display()
    );
    // `open` creates the whole tree; here, prove the nearest existing ancestor is writable.
    let mut probe = dir;
    while !probe.is_dir() {
        probe = probe.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "INTENT_STORE_DIR {}: no existing directory above it",
                dir.display()
            )
        })?;
    }
    // A live store's tmp/ is the write area and is swept on every open, so a probe left
    // there by a crash cleans itself up.
    let probe_dir = if probe.join("tmp").is_dir() {
        probe.join("tmp")
    } else {
        probe.to_path_buf()
    };
    let marker = probe_dir.join(format!("probe-check-{}", uuid::Uuid::new_v4().simple()));
    std::fs::write(&marker, b"")
        .and_then(|()| std::fs::remove_file(&marker))
        .map_err(|e| {
            anyhow::anyhow!(
                "INTENT_STORE_DIR {}: not writable: {e}",
                probe_dir.display()
            )
        })
}

/// The session table's pool: small, like the prober's (D-047), so a flood of shaped-but-random
/// cookies turns into bounded writes here — never into intake's pool.
fn sessions_db(config: &Config) -> anyhow::Result<Db> {
    Db::connect_lazy(
        config.database_url.expose(),
        intent_db::PoolLimits {
            max_connections: 4,
            acquire_timeout: Duration::from_secs(1),
            statement_timeout: Duration::from_secs(1),
        },
    )
    .map_err(|e| anyhow::anyhow!("INTENT_DATABASE_URL: {}", e.summary()))
}

/// The sign-out pool of one: nothing unauthenticated reaches it — cookie floods hit
/// `sessions`, sign-in floods the budgets — so an operator's delete is never queued
/// behind either and the honest 503 stays rare (D-052 for deletion, D-069).
fn signout_db(config: &Config) -> anyhow::Result<Db> {
    Db::connect_lazy(
        config.database_url.expose(),
        intent_db::PoolLimits {
            max_connections: 1,
            acquire_timeout: Duration::from_secs(1),
            statement_timeout: Duration::from_secs(1),
        },
    )
    .map_err(|e| anyhow::anyhow!("INTENT_DATABASE_URL: {}", e.summary()))
}

/// The in-memory session state the audit table still knows: the addresses the
/// known window still covers. Runs before `mark_ready` so the first request sees
/// the restored state (N10).
async fn restore_session_memory(config: &Config, app: &App) {
    // Addresses that produced a session inside the known window keep their day
    // budget through a deploy — an empty set after every restart is the D-052
    // lockout again. Only sign-ins under the CURRENT credential seed the set:
    // after a rotation, a thief's last sign-in under the old password earns the
    // address no day budget — the trade-off a rotation buys (D-069).
    let Some(hash) = &config.operator_password_hash else {
        tracing::info!("no operator credential configured — known sign-in addresses not seeded");
        return;
    };
    match app
        .sessions
        .recent_signer_addresses(
            Duration::from_millis(crate::operator::KNOWN_ADDRESS_WINDOW_MS),
            &crate::operator::credential_fingerprint(hash),
        )
        .await
    {
        Ok(rows) => {
            let count = rows.len();
            let now = app.now_ms();
            let wall_now_ms = u64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX);
            for (address, at) in &rows {
                if let Ok(ip) = address.parse::<std::net::IpAddr>() {
                    // The seed keeps the audit's own window — `at + window`, never a fresh
                    // `now + window` — so a restart inside the window ages it out on the
                    // original clock rather than renewing it forever (D).
                    let until_ms = u64::try_from(at.unix_timestamp_nanos() / 1_000_000)
                        .unwrap_or(0)
                        .saturating_add(crate::operator::KNOWN_ADDRESS_WINDOW_MS);
                    app.sign_in.mark_address_within(
                        intent_domain::ratelimit::AddressKey::of(ip),
                        now,
                        until_ms.saturating_sub(wall_now_ms),
                    );
                }
            }
            tracing::info!(count, "seeded known sign-in addresses");
        }
        Err(error) => {
            tracing::warn!(%error, "known sign-in addresses not seeded — the day budget applies");
        }
    }
}

/// Runs the server until a signal, then drains.
pub async fn run(config: Config) -> anyhow::Result<ShutdownOutcome> {
    let signal = os_signal().context("installing signal handlers")?;
    let db = Db::connect_lazy(config.database_url.expose(), config.db)
        .map_err(|e| anyhow::anyhow!("INTENT_DATABASE_URL: {}", e.summary()))?;
    // The notifier gets a pool of its own, so a backlog of retries never queues intake behind it.
    let notifier_db = Db::connect_lazy(
        config.database_url.expose(),
        config.notify.pool_limits(config.db),
    )
    .map_err(|e| anyhow::anyhow!("INTENT_DATABASE_URL: {}", e.summary()))?;
    let provider = provider(&config.notify)?;
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    let health = Health::default();
    let studio = studio(&config)?;
    let app = App::new(
        config.clone(),
        db.clone(),
        sessions_db(&config)?,
        signout_db(&config)?,
        health.clone(),
        studio,
    );

    let cancel = CancellationToken::new();
    let tasks = TaskTracker::new();
    // The prober has a connection of its own: readiness measures the database, not intake's
    // load. Borrowing from intake's pool, a saturated server would probe "down" and then refuse
    // everything (D-047).
    let probe_db = Db::connect_lazy(
        config.database_url.expose(),
        intent_db::PoolLimits {
            max_connections: 1,
            acquire_timeout: Duration::from_secs(1),
            statement_timeout: Duration::from_secs(1),
        },
    )
    .map_err(|e| anyhow::anyhow!("INTENT_DATABASE_URL: {}", e.summary()))?;
    tasks.spawn(run_prober(
        probe_db.clone(),
        health.clone(),
        config.db_probe_interval,
        Duration::from_secs(1),
        cancel.clone(),
    ));
    tasks.spawn(
        Notifier {
            app: app.clone(),
            db: notifier_db.clone(),
            provider,
        }
        .run(cancel.clone()),
    );

    let app_connections = app.connections.clone();
    let router = app::router(app.clone());
    restore_session_memory(&config, &app).await;
    let address = listener.local_addr().context("reading the bound address")?;
    tracing::info!(%address, "listening");
    health.mark_ready();

    let stop = CancellationToken::new();
    let accepting = tokio::spawn(crate::conn::serve(
        listener,
        router,
        config.connections,
        config.trusted_proxies.clone().into(),
        app_connections,
        stop.clone(),
    ));
    signal.await;
    tracing::info!("shutdown signal received: readiness is now false");
    health.begin_drain();
    tokio::time::sleep(config.drain_delay).await;
    tracing::info!("drain delay over: no new connections");
    stop.cancel();
    let connections = accepting.await.context("the accept loop failed")?;
    let outcome = if tokio::time::timeout(config.shutdown_grace, connections.wait())
        .await
        .is_ok()
    {
        ShutdownOutcome::Graceful
    } else {
        ShutdownOutcome::DeadlineExceeded
    };

    cancel.cancel();
    tasks.close();
    let tail = config.notify.attempt_deadline + Duration::from_secs(3);
    if tokio::time::timeout(tail, tasks.wait()).await.is_err() {
        tracing::warn!("background tasks did not stop in time");
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        db.close().await;
        notifier_db.close().await;
        probe_db.close().await;
    })
    .await;
    tracing::info!(?outcome, "shutdown complete");
    Ok(outcome)
}
