//! `POST /v1/sites/{site}/leads`: the lead-intake path (D-005, D-019, invariant 5).
//!
//! Cheapest refusals first — shed when full, refuse while the database is down, per-address and
//! per-site limits — then parse, validate, and store the lead with its outbox row under a
//! deadline. Success (201, or 200 for a replay) is sent only after the commit returned; every
//! other path is a refusal that acknowledges nothing, and the client may resend with the same
//! submission id safely.

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use intent_db::{NewLead, StoreOutcome};
use intent_domain::{
    ids::{LeadId, OutboxId, SiteId},
    lead::{LeadForm, Rejection},
    limits::MAX_BODY_BYTES,
    ratelimit::{AddressKey, RetryAfter, client_address},
    submission::{fingerprint, keys_for},
};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    app::{App, Peer},
    error::ApiError,
    metrics::Metrics,
};

/// The acknowledgment. Sent only for a committed lead.
#[derive(Debug, Serialize)]
pub struct Acknowledgment {
    /// The lead.
    pub lead_id: LeadId,
    /// The client's submission id, or the key the server derived.
    pub submission_id: String,
    /// Always `stored`: the lead and its notification outbox row are committed.
    pub status: &'static str,
    /// True when this answers a resubmission of an already stored lead.
    pub duplicate: bool,
}

/// The handler.
pub async fn submit(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path(site): Path<String>,
    headers: HeaderMap,
    // Not read by an extractor: the body is read only after the cheap refusals below, so a
    // client that is shed or over its limit never gets the server to wait for its bytes.
    body: Body,
) -> Result<Response, ApiError> {
    let metrics = &app.metrics;
    let Ok(_permit) = app.gate.clone().try_acquire_owned() else {
        Metrics::inc(&metrics.intake_shed_total);
        return Err(ApiError::Overloaded);
    };
    let site: SiteId = site.parse().map_err(|_| ApiError::UnknownSite)?;
    if !app.health.database_up() {
        Metrics::inc(&metrics.intake_unavailable_total);
        return Err(ApiError::Unavailable(None));
    }

    let forwarded = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
    let address = client_address(peer.ip(), forwarded, &app.config.trusted_proxies);
    let now = app.now_ms();
    if let Err(RetryAfter(secs)) = app.address_limiter.check(AddressKey::of(address), now) {
        Metrics::inc(&metrics.intake_rate_limited_address_total);
        return Err(ApiError::RateLimited {
            retry_after_secs: secs,
        });
    }

    let body = read_body(body).await?;
    let form =
        parse(&headers, &body).inspect_err(|_| Metrics::inc(&metrics.intake_invalid_total))?;
    let lead = form.validate().map_err(|rejection| match rejection {
        Rejection::Honeypot => {
            Metrics::inc(&metrics.intake_honeypot_total);
            ApiError::Rejected
        }
        Rejection::Invalid(fields) => {
            Metrics::inc(&metrics.intake_invalid_total);
            ApiError::Validation(fields)
        }
    })?;

    // The site's budget is spent only by a storable lead for a site that accepts leads: a bot's
    // honeypot post, an invalid form or a made-up site id costs it nothing, and an unknown site
    // never gets a rate-limit row.
    let accepts = within_deadline(&app, app.sites.accepts(&app.db, site)).await?;
    if !accepts {
        return Err(ApiError::UnknownSite);
    }
    if let Err(RetryAfter(secs)) = app.site_limiter.check(site, app.now_ms()) {
        Metrics::inc(&metrics.intake_rate_limited_site_total);
        return Err(ApiError::RateLimited {
            retry_after_secs: secs,
        });
    }

    let received_at = OffsetDateTime::now_utc();
    let content = fingerprint(&lead);
    let keys = keys_for(site, &lead, &content, received_at.unix_timestamp());
    let new = NewLead {
        id: LeadId::from_uuid(Uuid::now_v7()),
        outbox_id: OutboxId::from_uuid(Uuid::now_v7()),
        site,
        key: &keys.primary,
        previous_window: keys.previous_window.as_ref(),
        fingerprint: content,
        lead: &lead,
        received_at,
    };
    // Past the deadline the commit may or may not have happened; either way nothing is
    // acknowledged, and a resend with the same submission id finds the lead if it did.
    let outcome = within_deadline(&app, app.db.store_lead(&new)).await?;
    let submission_id = keys.primary.public().to_owned();
    if !matches!(outcome, StoreOutcome::Created(_)) {
        // Not a new lead (a replay, a conflict, a site closed meanwhile): its token goes back.
        app.site_limiter.refund(site);
    }
    match outcome {
        StoreOutcome::Created(lead_id) => {
            Metrics::inc(&metrics.leads_created_total);
            tracing::debug!(%lead_id, %site, "lead stored");
            Ok(acknowledge(
                StatusCode::CREATED,
                lead_id,
                submission_id,
                false,
            ))
        }
        StoreOutcome::Duplicate(lead_id) => {
            Metrics::inc(&metrics.leads_replayed_total);
            tracing::debug!(%lead_id, %site, "resubmission answered with the stored lead");
            Ok(acknowledge(StatusCode::OK, lead_id, submission_id, true))
        }
        StoreOutcome::Conflict => {
            Metrics::inc(&metrics.intake_conflicts_total);
            Err(ApiError::SubmissionConflict)
        }
        StoreOutcome::UnknownSite => Err(ApiError::UnknownSite),
    }
}

fn acknowledge(
    status: StatusCode,
    lead_id: LeadId,
    submission_id: String,
    duplicate: bool,
) -> Response {
    let mut response = (
        status,
        Json(Acknowledgment {
            lead_id,
            submission_id,
            status: "stored",
            duplicate,
        }),
    )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// Runs a database call under the intake deadline, counting timeouts and outages.
async fn within_deadline<T>(
    app: &App,
    call: impl Future<Output = Result<T, intent_db::DbError>>,
) -> Result<T, ApiError> {
    match tokio::time::timeout(app.config.intake_deadline, call).await {
        Err(_elapsed) => {
            Metrics::inc(&app.metrics.intake_timeouts_total);
            Err(ApiError::Timeout)
        }
        Ok(Err(error)) => {
            if error.is_unavailable() {
                Metrics::inc(&app.metrics.intake_unavailable_total);
            }
            Err(error.into())
        }
        Ok(Ok(value)) => Ok(value),
    }
}

/// Reads a request body of at most [`MAX_BODY_BYTES`]; the request timeout bounds how long.
pub async fn read_body(body: Body) -> Result<Bytes, ApiError> {
    axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|error| {
            let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
            while let Some(current) = cause {
                if current.is::<http_body_util::LengthLimitError>() {
                    return ApiError::PayloadTooLarge;
                }
                cause = current.source();
            }
            ApiError::MalformedBody
        })
}

/// JSON or a no-script form post. Parse errors are never echoed: serde messages can quote input.
fn parse(headers: &HeaderMap, body: &[u8]) -> Result<LeadForm, ApiError> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        });
    match content_type.as_deref() {
        Some("application/json") => {
            serde_json::from_slice(body).map_err(|_| ApiError::MalformedBody)
        }
        Some("application/x-www-form-urlencoded") => {
            serde_urlencoded::from_bytes(body).map_err(|_| ApiError::MalformedBody)
        }
        _ => Err(ApiError::UnsupportedMediaType),
    }
}
