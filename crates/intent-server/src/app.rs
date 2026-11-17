//! Application state, routes and the middleware stack.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, MatchedPath, Request, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    middleware as axum_mw,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use intent_db::Db;
use intent_domain::{ids::SiteId, limits::MAX_BODY_BYTES, ratelimit::AddressKey};
use serde_json::json;
use tokio::sync::Semaphore;
use tower::ServiceBuilder;
use tower_http::{
    catch_panic::CatchPanicLayer,
    limit::RequestBodyLimitLayer,
    request_id::{MakeRequestId, PropagateRequestIdLayer, RequestId, SetRequestIdLayer},
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::{DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::{
    config::Config, error::ApiError, health::Health, intake, limiter::Limiter, metrics::Metrics,
    operator,
};

/// The request-id header.
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Shared state; cloning is one reference-count increment.
#[derive(Clone)]
pub struct App(Arc<Inner>);

/// What every handler can reach.
pub struct Inner {
    /// Validated configuration.
    pub config: Config,
    /// The database.
    pub db: Db,
    /// Liveness and readiness.
    pub health: Health,
    /// Counters.
    pub metrics: Metrics,
    /// In-flight requests; full means shed.
    pub gate: Arc<Semaphore>,
    /// Per-address lead limit.
    pub address_limiter: Limiter<AddressKey>,
    /// Per-site lead limit.
    pub site_limiter: Limiter<SiteId>,
    /// Per-address operator sign-in attempts.
    pub sign_in_limiter: Limiter<AddressKey>,
    /// Concurrent password hashes (CPU-bound, run off the async workers).
    pub hash_gate: Arc<Semaphore>,
    /// Sites verified to accept leads (the per-site budget is charged only for these).
    pub sites: crate::sites::SiteDirectory,
    /// Connection counters, kept by the accept loop.
    pub connections: Arc<crate::conn::ConnectionStats>,
    started: Instant,
}

impl std::ops::Deref for App {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl App {
    /// Builds the state.
    pub fn new(config: Config, db: Db, health: Health) -> Self {
        let sign_in_quota =
            intent_domain::ratelimit::Quota::per_minute(5).unwrap_or(config.address_quota);
        Self(Arc::new(Inner {
            gate: Arc::new(Semaphore::new(config.max_in_flight)),
            address_limiter: Limiter::new(config.address_quota, config.rate_table_entries),
            site_limiter: Limiter::new(config.site_quota, config.rate_table_entries),
            sign_in_limiter: Limiter::new(sign_in_quota, 10_000),
            hash_gate: Arc::new(Semaphore::new(2)),
            connections: Arc::new(crate::conn::ConnectionStats::default()),
            sites: crate::sites::SiteDirectory::new(100_000, std::time::Duration::from_secs(60)),
            metrics: Metrics::default(),
            started: Instant::now(),
            config,
            db,
            health,
        }))
    }

    /// Milliseconds on the process's monotonic clock (for token buckets).
    pub fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Request ids are UUID v7: sortable by time.
#[derive(Clone, Copy)]
struct MakeRequestUuidV7;

impl MakeRequestId for MakeRequestUuidV7 {
    fn make_request_id<B>(&mut self, _request: &axum::http::Request<B>) -> Option<RequestId> {
        HeaderValue::from_str(&uuid::Uuid::now_v7().to_string())
            .ok()
            .map(RequestId::new)
    }
}

/// A client-supplied request id is kept only if it is short and plain, so it cannot flood or
/// forge log lines.
async fn sanitize_request_id(mut request: Request) -> Request {
    let acceptable = request.headers().get(&REQUEST_ID).is_some_and(|v| {
        let bytes = v.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 64
            && bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
    });
    if !acceptable {
        request.headers_mut().remove(&REQUEST_ID);
    }
    request
}

/// The request span: method, route template and request id — never the query string, a body or
/// a header value.
fn make_span(request: &axum::http::Request<axum::body::Body>) -> tracing::Span {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str);
    let request_id = request
        .headers()
        .get(&REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    tracing::info_span!("request", method = %request.method(), route, request_id)
}

fn panic_response(_payload: Box<dyn std::any::Any + Send + 'static>) -> Response {
    // The payload may hold internals; it is not echoed.
    ApiError::Internal(anyhow::anyhow!("a handler panicked")).into_response()
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn readyz(State(app): State<App>) -> Response {
    let body = Json(json!({
        "ready": app.health.is_ready(),
        "lifecycle": app.health.lifecycle(),
        "database": if app.health.database_up() { "up" } else { "down" },
    }));
    if app.health.is_ready() {
        body.into_response()
    } else {
        let mut response = (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(1));
        response
    }
}

async fn metrics(State(app): State<App>) -> Response {
    let in_flight = app
        .config
        .max_in_flight
        .saturating_sub(app.gate.available_permits());
    let connections = &app.connections;
    let load = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
    let counters = [
        (
            "connections_accepted_total",
            "Connections accepted.",
            load(&connections.accepted_total),
        ),
        (
            "connections_write_stalled_total",
            "Connections closed because the client stopped reading.",
            load(&connections.write_stalled_total),
        ),
    ];
    let text = app.metrics.render(
        &counters,
        &[
            (
                "connections_open",
                "Connections open now.",
                load(&connections.open),
            ),
            (
                "in_flight",
                "Requests holding an intake permit.",
                u64::try_from(in_flight).unwrap_or(0),
            ),
            (
                "rate_table_address_keys",
                "Keys in the per-address table.",
                u64::try_from(app.address_limiter.len()).unwrap_or(0),
            ),
            (
                "rate_table_address_evictions",
                "Per-address keys evicted to stay bounded.",
                app.address_limiter.evictions(),
            ),
            (
                "rate_table_site_keys",
                "Keys in the per-site table.",
                u64::try_from(app.site_limiter.len()).unwrap_or(0),
            ),
            (
                "database_up",
                "1 when the last database probe succeeded.",
                u64::from(app.health.database_up()),
            ),
        ],
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], text).into_response()
}

async fn fallback() -> ApiError {
    ApiError::NotFound
}

/// The router with its middleware, ready to serve.
pub fn router(app: App) -> Router {
    let request_timeout = app.config.request_timeout;
    let workspace = app.config.workspace_dir.clone();
    let mut router = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/v1/sites/{site}/leads", post(intake::submit))
        .route(
            "/v1/operator/session",
            post(operator::sign_in)
                .get(operator::session)
                .delete(operator::sign_out),
        )
        .route("/v1/operator/leads", get(operator::inbox))
        .fallback(fallback);
    if let Some(dir) = workspace {
        let index = dir.join("200.html");
        router = router.nest_service(
            "/workspace",
            ServeDir::new(dir).fallback(ServeFile::new(index)),
        );
    }
    let stack = ServiceBuilder::new()
        .layer(axum_mw::map_request(sanitize_request_id))
        .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuidV7))
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(make_span)
                .on_request(())
                .on_response(DefaultOnResponse::new().level(Level::INFO))
                // Refusals are logged once by `ApiError` at a level by fault; tower-http would
                // log every 5xx (an honest 503 included) again as an ERROR.
                .on_failure(()),
        )
        .layer(axum_mw::from_fn(crate::error::envelope))
        .layer(CatchPanicLayer::custom(panic_response))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            request_timeout,
        ))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));
    router.layer(stack).with_state(app)
}

/// The address the server listens on, and the connect-info type its handlers extract.
pub type Peer = SocketAddr;

/// Drain timing for [`crate::serve`].
#[derive(Debug, Clone, Copy)]
pub struct ShutdownTiming {
    /// Readiness false before the listener stops.
    pub drain_delay: Duration,
    /// Longest wait for in-flight requests after it stops.
    pub grace: Duration,
}
