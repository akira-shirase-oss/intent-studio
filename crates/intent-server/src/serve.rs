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
    let app = App::new(config.clone(), db.clone(), health.clone(), studio);

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
    let router = app::router(app);
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
