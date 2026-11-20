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
    lead::{ClientSubmissionId, LeadForm, Rejection},
    limits::MAX_BODY_BYTES,
    ratelimit::{AddressKey, RetryAfter, client_address},
    submission::{StoredKey, SubmissionKeys, fingerprint, keys_for},
};
use serde::Serialize;
use std::net::IpAddr;
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

    let address = client_address_of(
        &headers,
        peer.ip(),
        &app.config.trusted_proxies,
        app.config.trust_any_forwarded(),
    );
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
    let mut keys = keys_for(site, &lead, &content, received_at.unix_timestamp());
    if app.config.mutation_is(crate::config::Mutation::NoLeadDedup) {
        // The proof's seam: a fresh key per submission means the `(site, key)` unique
        // constraint can never fire — every retry is stored as its own lead.
        if let Ok(id) = ClientSubmissionId::parse(&format!("m{}", Uuid::now_v7().simple())) {
            keys = SubmissionKeys {
                primary: StoredKey::client(&id),
                previous_window: None,
            };
        }
    }
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

/// The client address for rate limiting: every `X-Forwarded-For` line, joined in order and read
/// from the right under the trusted-proxy rule. A proxy may add its own line instead of
/// appending to the client's, so reading only the first line would let a client pick its own
/// address. A line that is not text makes the header untrustworthy: the peer is used.
pub fn client_address_of(
    headers: &HeaderMap,
    peer: IpAddr,
    trusted: &[IpAddr],
    trust_any: bool,
) -> IpAddr {
    if trust_any {
        // Mutation seam: believe the client-supplied hop outright, so address rotation
        // moves the limiter key — the attack this proves is the header being trusted.
        for line in headers.get_all("x-forwarded-for") {
            if let Ok(text) = line.to_str()
                && let Some(first) = text.split(',').next()
                && let Ok(ip) = first.trim().parse::<IpAddr>()
            {
                return ip;
            }
        }
    }
    let mut joined = String::new();
    for line in headers.get_all("x-forwarded-for") {
        let Ok(text) = line.to_str() else {
            return client_address(peer, Some("unreadable"), trusted);
        };
        if !joined.is_empty() {
            joined.push(',');
        }
        joined.push_str(text);
    }
    client_address(
        peer,
        (!joined.is_empty()).then_some(joined.as_str()),
        trusted,
    )
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
    read_body_capped(body, MAX_BODY_BYTES).await
}

/// Reads a request body of at most `cap` bytes; the request timeout bounds how long.
pub async fn read_body_capped(body: Body, cap: usize) -> Result<Bytes, ApiError> {
    axum::body::to_bytes(body, cap).await.map_err(|error| {
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

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(lines: &[&[u8]]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for line in lines {
            map.append("x-forwarded-for", HeaderValue::from_bytes(line).unwrap());
        }
        map
    }

    #[test]
    fn every_forwarded_for_line_is_read_right_to_left() {
        let proxy: IpAddr = "127.0.0.1".parse().unwrap();
        let real: IpAddr = "10.60.0.1".parse().unwrap();
        let two = headers(&[b"10.50.0.9", b"10.60.0.1"]);
        assert_eq!(client_address_of(&two, proxy, &[proxy], false), real);
        let one = headers(&[b"10.50.0.9, 10.60.0.1"]);
        assert_eq!(client_address_of(&one, proxy, &[proxy], false), real);
        assert_eq!(
            client_address_of(&HeaderMap::new(), proxy, &[proxy], false),
            proxy
        );
        // A line that is not text: the header is not believed.
        let garbage = headers(&[b"10.50.0.9", b"\xff\xfe"]);
        assert_eq!(client_address_of(&garbage, proxy, &[proxy], false), proxy);
        // An untrusted peer's header is ignored altogether.
        let stranger: IpAddr = "203.0.113.7".parse().unwrap();
        assert_eq!(client_address_of(&two, stranger, &[proxy], false), stranger);
    }
}
