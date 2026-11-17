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
    app::{self, App, Peer},
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

/// Runs the server until a signal, then drains.
pub async fn run(config: Config) -> anyhow::Result<ShutdownOutcome> {
    let signal = os_signal().context("installing signal handlers")?;
    let db = Db::connect_lazy(config.database_url.expose(), config.db)
        .map_err(|e| anyhow::anyhow!("INTENT_DATABASE_URL: {}", e.summary()))?;
    // The notifier gets a pool of its own, so a backlog of retries never queues intake behind it.
    let notifier_db = Db::connect_lazy(
        config.database_url.expose(),
        intent_db::PoolLimits {
            max_connections: config.notify.db_connections,
            ..config.db
        },
    )
    .map_err(|e| anyhow::anyhow!("INTENT_DATABASE_URL: {}", e.summary()))?;
    let provider = provider(&config.notify)?;
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    let health = Health::default();
    let app = App::new(config.clone(), db.clone(), health.clone());

    let cancel = CancellationToken::new();
    let tasks = TaskTracker::new();
    tasks.spawn(run_prober(
        db.clone(),
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

    let router = app::router(app);
    let address = listener.local_addr().context("reading the bound address")?;
    tracing::info!(%address, "listening");
    health.mark_ready();

    let (triggered_tx, triggered_rx) = tokio::sync::oneshot::channel::<()>();
    let drain_health = health.clone();
    let drain_delay = config.drain_delay;
    let stop_accepting = async move {
        signal.await;
        tracing::info!("shutdown signal received: readiness is now false");
        let _ = triggered_tx.send(());
        drain_health.begin_drain();
        tokio::time::sleep(drain_delay).await;
        tracing::info!("drain delay over: no new connections");
    };
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<Peer>(),
    )
    .with_graceful_shutdown(stop_accepting)
    .into_future();
    let grace = config.shutdown_grace;
    let hard_deadline = async move {
        if triggered_rx.await.is_err() {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(drain_delay + grace).await;
    };
    let outcome = tokio::select! {
        result = server => { result.context("serving")?; ShutdownOutcome::Graceful }
        () = hard_deadline => ShutdownOutcome::DeadlineExceeded,
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
    })
    .await;
    tracing::info!(?outcome, "shutdown complete");
    Ok(outcome)
}
