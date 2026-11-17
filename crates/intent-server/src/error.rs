//! One error type and one envelope for every refusal (rust-mastery chapter 12, concept 6).
//!
//! `{"error": {"code": "...", "message": "...", "request_id": "...", "fields": [...]}}`. The
//! code is the stable [`RefusalCode`]; the message is fixed text, never input; `fields` lists
//! the refused fields and codes, never values. Each error is logged once, when it becomes a
//! response, at a level by fault — and never with lead content.

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use intent_db::DbError;
use intent_domain::{lead::FieldError, refusal::RefusalCode};
use serde_json::json;

/// Everything a handler can refuse with.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// 400.
    #[error("the body is not a valid lead submission")]
    MalformedBody,
    /// 413.
    #[error("the body exceeds the size limit")]
    PayloadTooLarge,
    /// 415.
    #[error("send application/json or application/x-www-form-urlencoded")]
    UnsupportedMediaType,
    /// 422 with the refused fields.
    #[error("one or more fields failed validation")]
    Validation(Vec<FieldError>),
    /// 400, deliberately unspecific (the honeypot).
    #[error("the submission was rejected")]
    Rejected,
    /// 404.
    #[error("no such site, or it is not accepting leads")]
    UnknownSite,
    /// 429 with `Retry-After`.
    #[error("too many submissions; retry later")]
    RateLimited { retry_after_secs: u64 },
    /// 409.
    #[error("this submission id was already used for different content")]
    SubmissionConflict,
    /// 503 with `Retry-After`: the database cannot take it. Nothing was acknowledged.
    #[error(
        "the lead could not be saved right now; it was not acknowledged, retry with the same submission id"
    )]
    Unavailable(#[source] Option<DbError>),
    /// 503 with `Retry-After`: shed before doing any work.
    #[error("the server is at capacity; retry shortly")]
    Overloaded,
    /// 503 with `Retry-After`: the deadline passed before the outcome was known.
    #[error(
        "the request did not finish in time; it was not acknowledged, retry with the same submission id"
    )]
    Timeout,
    /// 401.
    #[error("sign in as the operator first")]
    Unauthorized,
    /// 503: operator sign-in is not configured on this server.
    #[error("operator sign-in is not configured")]
    SignInDisabled,
    /// 404.
    #[error("not found")]
    NotFound,
    /// 500. The cause goes to the log; the client sees only the code.
    #[error("internal error")]
    Internal(#[source] anyhow::Error),
}

impl ApiError {
    /// The stable code.
    pub fn code(&self) -> RefusalCode {
        match self {
            Self::MalformedBody => RefusalCode::MalformedBody,
            Self::PayloadTooLarge => RefusalCode::PayloadTooLarge,
            Self::UnsupportedMediaType => RefusalCode::UnsupportedMediaType,
            Self::Validation(_) => RefusalCode::ValidationFailed,
            Self::Rejected => RefusalCode::Rejected,
            Self::UnknownSite => RefusalCode::UnknownSite,
            Self::RateLimited { .. } => RefusalCode::RateLimited,
            Self::SubmissionConflict => RefusalCode::SubmissionConflict,
            Self::Unavailable(_) | Self::SignInDisabled => RefusalCode::Unavailable,
            Self::Overloaded => RefusalCode::Overloaded,
            Self::Timeout => RefusalCode::Timeout,
            Self::Unauthorized => RefusalCode::Unauthorized,
            Self::NotFound => RefusalCode::NotFound,
            Self::Internal(_) => RefusalCode::Internal,
        }
    }

    /// The HTTP status.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::MalformedBody | Self::Rejected => StatusCode::BAD_REQUEST,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::UnknownSite | Self::NotFound => StatusCode::NOT_FOUND,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::SubmissionConflict => StatusCode::CONFLICT,
            Self::Unavailable(_) | Self::Overloaded | Self::Timeout | Self::SignInDisabled => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn retry_after(&self) -> Option<u64> {
        match self {
            Self::RateLimited { retry_after_secs } => Some(*retry_after_secs),
            Self::Unavailable(_) => Some(2),
            Self::Overloaded | Self::Timeout => Some(1),
            _ => None,
        }
    }

    /// Logs once, by fault. Database errors log their log-safe summary only.
    fn log(&self) {
        let code = self.code().as_str();
        match self {
            Self::Internal(cause) => {
                tracing::error!(code, error = %format!("{cause:#}"), "request failed");
            }
            Self::Unavailable(Some(db)) => {
                tracing::warn!(code, db = %db.summary(), "lead not stored: database unavailable");
            }
            Self::Unavailable(None) | Self::Overloaded | Self::Timeout => {
                tracing::warn!(code, "request refused");
            }
            _ => tracing::debug!(code, "request refused"),
        }
    }
}

impl From<DbError> for ApiError {
    fn from(error: DbError) -> Self {
        if error.is_unavailable() {
            Self::Unavailable(Some(error))
        } else {
            // The summary, not the driver's message: no row value can reach the log.
            Self::Internal(anyhow::anyhow!("database: {}", error.summary()))
        }
    }
}

/// The public part of an error, kept in response extensions so the envelope middleware can add
/// the request id.
#[derive(Debug, Clone)]
pub struct PublicError {
    code: &'static str,
    message: String,
    fields: Option<serde_json::Value>,
}

impl PublicError {
    fn from_status(status: StatusCode) -> Self {
        let (code, message) = match status {
            StatusCode::METHOD_NOT_ALLOWED => (RefusalCode::MethodNotAllowed, "method not allowed"),
            StatusCode::PAYLOAD_TOO_LARGE => (
                RefusalCode::PayloadTooLarge,
                "the body exceeds the size limit",
            ),
            StatusCode::NOT_FOUND => (RefusalCode::NotFound, "not found"),
            StatusCode::REQUEST_TIMEOUT
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT => (
                RefusalCode::Timeout,
                "the request did not finish in time; it was not acknowledged",
            ),
            s if s.is_client_error() => (RefusalCode::MalformedBody, "the request was refused"),
            _ => (RefusalCode::Internal, "internal error"),
        };
        Self {
            code: code.as_str(),
            message: message.to_owned(),
            fields: None,
        }
    }
}

fn render(status: StatusCode, public: &PublicError, request_id: Option<&str>) -> Response {
    let mut error = json!({
        "code": public.code,
        "message": public.message,
        "request_id": request_id,
    });
    if let Some(fields) = &public.fields {
        error["fields"] = fields.clone();
    }
    let mut response = (status, axum::Json(json!({ "error": error }))).into_response();
    response.extensions_mut().insert(public.clone());
    response
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.log();
        let fields = match &self {
            Self::Validation(errors) => Some(
                errors
                    .iter()
                    .map(|e| json!({ "field": e.field.as_str(), "code": e.code.as_str() }))
                    .collect::<serde_json::Value>(),
            ),
            _ => None,
        };
        let public = PublicError {
            code: self.code().as_str(),
            message: self.to_string(),
            fields,
        };
        let mut response = render(self.status(), &public, None);
        if let Some(secs) = self.retry_after() {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        response
    }
}

/// Re-renders every error response with the request id, and gives errors produced by layers
/// (405, 413, timeouts, panics) the same envelope.
pub async fn envelope(request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get(crate::app::REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let response = next.run(request).await;
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return response;
    }
    let public = match response.extensions().get::<PublicError>() {
        Some(public) => public.clone(),
        None if is_json(&response) => return response,
        None => PublicError::from_status(status),
    };
    let (mut parts, _) = response.into_parts();
    let fresh = render(status, &public, request_id.as_deref());
    let (fresh_parts, body) = fresh.into_parts();
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.extensions = fresh_parts.extensions;
    if status == StatusCode::SERVICE_UNAVAILABLE && !parts.headers.contains_key(header::RETRY_AFTER)
    {
        parts
            .headers
            .insert(header::RETRY_AFTER, HeaderValue::from(1));
    }
    Response::from_parts(parts, Body::new(body))
}

fn is_json(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
}
