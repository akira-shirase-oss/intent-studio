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
    assets, config::Config, error::ApiError, health::Health, intake, limiter::Limiter,
    metrics::Metrics, operator, studio,
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
    /// Operator sign-in admission: per address, then a global budget for verifications.
    pub sign_in: crate::operator::SignInGate,
    /// Concurrent password hashes (CPU-bound, run off the async workers).
    pub hash_gate: Arc<Semaphore>,
    /// Sites verified to accept leads (the per-site budget is charged only for these).
    pub sites: crate::sites::SiteDirectory,
    /// Connection counters, kept by the accept loop.
    pub connections: Arc<crate::conn::ConnectionStats>,
    /// The content store, the asset worker and the egress guard (I1).
    pub studio: Studio,
    started: Instant,
}

/// What the studio's untrusted-input routes need. Built at startup by [`crate::serve`].
pub struct Studio {
    /// The content-addressed store; `None` when `INTENT_STORE_DIR` is unset.
    pub store: Option<intent_store::ContentStore>,
    /// Runs the asset worker, one process per file.
    pub worker: intent_render::Driver,
    /// The only way to fetch a client-named URL (D-012).
    pub egress: intent_egress::Guard<intent_egress::SystemResolver, intent_egress::TcpConnector>,
}

impl std::ops::Deref for App {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl App {
    /// Builds the state.
    pub fn new(config: Config, db: Db, health: Health, studio: Studio) -> Self {
        Self(Arc::new(Inner {
            gate: Arc::new(Semaphore::new(config.max_in_flight)),
            address_limiter: Limiter::new(config.address_quota, config.rate_table_entries),
            site_limiter: Limiter::new(config.site_quota, config.rate_table_entries),
            sign_in: crate::operator::SignInGate::new(),
            hash_gate: Arc::new(Semaphore::new(2)),
            connections: Arc::new(crate::conn::ConnectionStats::default()),
            sites: crate::sites::SiteDirectory::new(100_000, std::time::Duration::from_secs(60)),
            metrics: Metrics::default(),
            studio,
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

/// Whether this request may read `/metrics`: the token holder, or (with no token configured) a
/// direct request from the host itself — not one that came through a proxy, which a local
/// reverse proxy would otherwise turn into a public page.
fn may_read_metrics(app: &App, peer: Peer, headers: &axum::http::HeaderMap) -> bool {
    use sha2::{Digest as _, Sha256};
    match &app.config.metrics_token {
        Some(token) => headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            // Compare digests, not the strings: equal-length inputs, no early exit on the secret.
            .is_some_and(|given| {
                Sha256::digest(given.as_bytes()) == Sha256::digest(token.expose().as_bytes())
            }),
        None => peer.ip().is_loopback() && !headers.contains_key("x-forwarded-for"),
    }
}

async fn metrics(
    State(app): State<App>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<Peer>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !may_read_metrics(&app, peer, &headers) {
        return ApiError::NotFound.into_response();
    }
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
            "connections_refused_per_address_total",
            "Connections closed at once because their address held its share.",
            load(&connections.refused_per_address_total),
        ),
        (
            "connections_retired_total",
            "Connections closed at their request cap or maximum age.",
            load(&connections.retired_total),
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

/// Bounds for one group of routes: how long a request may take, and how large its body may be.
macro_rules! bounded {
    ($timeout:expr, $body_bytes:expr) => {
        ServiceBuilder::new()
            .layer(TimeoutLayer::with_status_code(
                StatusCode::SERVICE_UNAVAILABLE,
                $timeout,
            ))
            .layer(RequestBodyLimitLayer::new($body_bytes))
            .layer(DefaultBodyLimit::max($body_bytes))
    };
}

/// I1: the studio's records. Larger bodies (a brief, a page tree); each handler caps its own.
fn studio_routes(app: &App) -> Router<App> {
    Router::new()
        .route(
            "/v1/operator/tenants",
            get(studio::tenants).post(studio::create_tenant),
        )
        .route(
            "/v1/operator/tenants/{tenant}/facts",
            get(studio::facts).post(studio::create_fact),
        )
        .route(
            "/v1/operator/tenants/{tenant}/facts/{fact}/approval",
            post(studio::approve_fact),
        )
        .route(
            "/v1/operator/tenants/{tenant}/facts/{fact}/withdrawal",
            post(studio::withdraw_fact),
        )
        .route(
            "/v1/operator/tenants/{tenant}/briefs",
            get(studio::briefs).post(studio::create_brief),
        )
        .route(
            "/v1/operator/tenants/{tenant}/briefs/{brief}",
            get(studio::brief).put(studio::revise_brief),
        )
        .route(
            "/v1/operator/tenants/{tenant}/briefs/{brief}/spec-check",
            post(studio::spec_check),
        )
        .layer(bounded!(
            app.config.request_timeout,
            studio::SPEC_BODY_BYTES
        ))
}

/// I1: untrusted bytes in — uploads through the asset worker, reference fetches through the
/// egress guard. Longer: an upload, a worker job and a fetch each take seconds.
fn untrusted_routes(app: &App) -> Router<App> {
    let timeout = (app.config.studio.worker_timeout + Duration::from_secs(20))
        .max(intent_egress::DEFAULT_DEADLINE + Duration::from_secs(10));
    Router::new()
        .route(
            "/v1/operator/tenants/{tenant}/assets",
            get(assets::list).post(assets::upload),
        )
        .route(
            "/v1/operator/tenants/{tenant}/assets/{asset}/content",
            get(assets::content),
        )
        .route(
            "/v1/operator/tenants/{tenant}/briefs/{brief}/snapshots",
            get(assets::snapshots).post(assets::fetch_snapshot),
        )
        .layer(bounded!(timeout, assets::UPLOAD_BODY_BYTES))
}

/// The router with its middleware, ready to serve.
pub fn router(app: App) -> Router {
    let request_timeout = app.config.request_timeout;
    let workspace = app.config.workspace_dir.clone();
    // I0: intake, health, metrics, the operator session and inbox, the workspace app. Bodies
    // are at most 16 KiB (D-019).
    let mut spine = Router::new()
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
        .route("/v1/operator/leads", get(operator::inbox));
    if let Some(dir) = workspace {
        let index = dir.join("200.html");
        // Files are served as they are; every other path (a route like `/workspace/briefs`,
        // which the build also has as a directory of sub-routes) gets the app shell, which
        // routes on the client.
        spine = spine.nest_service(
            "/workspace",
            ServeDir::new(dir)
                .append_index_html_on_directories(false)
                .fallback(ServeFile::new(index)),
        );
    }
    let spine = spine.layer(bounded!(request_timeout, MAX_BODY_BYTES));
    let studio = studio_routes(&app);
    let untrusted = untrusted_routes(&app);
    let stack = ServiceBuilder::new()
        .layer(axum_mw::map_request(sanitize_request_id))
        .layer(SetRequestIdLayer::new(REQUEST_ID, MakeRequestUuidV7))
        .layer(PropagateRequestIdLayer::new(REQUEST_ID))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        // Nothing here may be framed, load plugins or rebase relative URLs; the workspace's
        // pages add their own script policy (hashes) in a meta tag. No referrer leaves the
        // workspace (its URLs can name sites and leads). A handler may send a stricter policy
        // of its own (asset content adds `sandbox`), which this then leaves in place.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "frame-ancestors 'none'; base-uri 'none'; object-src 'none'; form-action 'self'",
            ),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
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
        // Timeouts and body caps are per route group (`bounded!`); the fallback reads nothing.
        .layer(CatchPanicLayer::custom(panic_response));
    spine
        .merge(studio)
        .merge(untrusted)
        .fallback(fallback)
        .layer(stack)
        .with_state(app)
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
