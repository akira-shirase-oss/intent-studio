//! Assets and reference snapshots (I1): the two ways untrusted bytes enter the studio.
//!
//! An upload is checked by the asset worker in a process of its own (D-011, D-062) before
//! anything is stored; the original and the worker's output are both kept, with the licence and
//! the source (invariant 6). A reference snapshot is fetched only through the egress guard
//! (D-012, D-060), outside any transaction, and its bytes are kept, not parsed (D-055). Both
//! record what happened — a refusal included — and audit it. Neither logs a byte of content or
//! a hash (the log scanner treats a 64-digit hex run as a secret shape).

use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, Multipart, Path, State, multipart::MultipartRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use intent_db::{AssetRow, NewAsset, NewSnapshot, SnapshotRow};
use intent_domain::{
    brief::BriefForm,
    ids::{AssetId, BriefId, SnapshotId, TenantId},
    problem::{Problems, checked_text},
    refusal::RefusalCode,
};
use intent_egress::Purpose;
use intent_render::{AssetKind, WorkerRefusal};
use intent_store::{ContentStore, StoreError};
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::{
    app::{App, Peer},
    error::ApiError,
    intake::read_body_capped,
    operator::{Caller, no_store, require_operator},
};

/// Largest upload body: the worker's input cap plus room for the form's other fields.
pub const UPLOAD_BODY_BYTES: usize = intent_render::MAX_INPUT_BYTES + 64 * 1024;

/// A header a cross-site form cannot send without a CORS preflight, which this server never
/// answers: the upload's defence on top of the `SameSite=Strict` session cookie.
pub const UPLOAD_HEADER: &str = "x-intent-upload";

fn ts(t: OffsetDateTime) -> Option<String> {
    t.format(&Rfc3339).ok()
}

fn store(app: &App) -> Result<&ContentStore, ApiError> {
    app.studio.store.as_ref().ok_or(ApiError::StoreUnavailable)
}

fn store_error(error: &StoreError) -> ApiError {
    match error {
        StoreError::TooLarge { .. } => ApiError::AssetRefused {
            reason: "too_large",
        },
        StoreError::Corrupt => ApiError::Internal(anyhow::anyhow!("a stored blob is corrupt")),
        StoreError::Io(_) | StoreError::Timeout => {
            tracing::warn!(error = %error, "content store failed");
            ApiError::StoreUnavailable
        }
    }
}

fn tenant_id(raw: &str) -> Result<TenantId, ApiError> {
    raw.parse().map_err(|_| ApiError::NotFound)
}

async fn require_tenant(app: &App, tenant: TenantId) -> Result<(), ApiError> {
    if app.db.tenant_exists(tenant).await? {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

fn asset_json(asset: &AssetRow) -> Value {
    json!({
        "id": asset.id,
        "tenant_id": asset.tenant_id,
        "media_type": asset.media_type,
        "bytes": asset.bytes,
        "width": asset.width,
        "height": asset.height,
        "removed_parts": asset.removed_parts,
        "sha256": asset.derived.to_string(),
        "original": {
            "name": asset.original_name,
            "media_type": asset.original_media_type,
            "bytes": asset.original_bytes,
            "sha256": asset.original.to_string(),
        },
        "licence": asset.licence,
        "source": asset.source,
        "created_at": ts(asset.created_at),
    })
}

/// The last path component of a browser-sent file name, if it is plain text of a sane length.
fn file_name(raw: Option<&str>) -> Option<String> {
    let base = raw?.rsplit(['/', '\\']).next()?;
    intent_domain::problem::text(Some(base), 200, false)
        .ok()
        .flatten()
}

/// The kind an upload asks for: its media type, or for the generic types browsers send for
/// fonts, its extension.
fn kind_of(media_type: Option<&str>, name: Option<&str>) -> Option<AssetKind> {
    if let Some(kind) = media_type.and_then(AssetKind::from_media_type) {
        return Some(kind);
    }
    let generic = media_type.is_none_or(|t| {
        matches!(
            t.split(';').next().unwrap_or_default().trim(),
            "" | "application/octet-stream"
        )
    });
    let extension = name?.rsplit('.').next()?.to_ascii_lowercase();
    match extension.as_str() {
        "ttf" | "otf" if generic => Some(AssetKind::Font),
        _ => None,
    }
}

fn worker_refusal(refusal: WorkerRefusal) -> ApiError {
    match refusal {
        WorkerRefusal::WorkerBusy => ApiError::Overloaded,
        other => ApiError::AssetRefused {
            reason: other.as_str(),
        },
    }
}

/// `POST /v1/operator/tenants/{tenant}/assets`: a `multipart/form-data` body with `file`,
/// `licence` and `source`, and the [`UPLOAD_HEADER`].
pub async fn upload(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Response, ApiError> {
    require_operator(&app, &headers).await?;
    if !headers.contains_key(UPLOAD_HEADER) {
        return Err(ApiError::UnsupportedMediaType);
    }
    let tenant = tenant_id(&tenant)?;
    require_tenant(&app, tenant).await?;
    let store = store(&app)?;
    let multipart = multipart.map_err(|_| ApiError::UnsupportedMediaType)?;

    let Upload {
        licence,
        source,
        name,
        media_type,
        bytes,
    } = read_upload(multipart).await?;
    let kind = kind_of(media_type.as_deref(), name.as_deref()).ok_or(ApiError::AssetRefused {
        reason: "unsupported_type",
    })?;

    // The worker, in a process of its own, decides; nothing is stored unless it accepts.
    let processed = app
        .studio
        .worker
        .run(kind, &bytes)
        .await
        .map_err(worker_refusal)?;
    let original_bytes = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let derived_bytes = i64::try_from(processed.output.len()).unwrap_or(i64::MAX);
    let original = store
        .put(bytes.to_vec())
        .await
        .map_err(|e| store_error(&e))?;
    let derived = store
        .put(processed.output)
        .await
        .map_err(|e| store_error(&e))?;

    let id = AssetId::from_uuid(Uuid::now_v7());
    let caller = Caller::of(&app, peer, &headers);
    let detail = json!({
        "tenant": tenant, "asset": id, "kind": kind.as_str(),
        "bytes": original_bytes, "removed_parts": processed.removed_parts,
    })
    .to_string();
    let media_type_text = media_type.unwrap_or_else(|| "application/octet-stream".into());
    let row = app
        .db
        .create_asset(
            &NewAsset {
                id,
                tenant,
                original,
                original_bytes,
                original_media_type: media_type_text.get(..100).unwrap_or(&media_type_text),
                original_name: name.as_deref(),
                derived,
                bytes: derived_bytes,
                media_type: &processed.media_type,
                width: processed.width.and_then(|w| i32::try_from(w).ok()),
                height: processed.height.and_then(|h| i32::try_from(h).ok()),
                removed_parts: i32::try_from(processed.removed_parts).unwrap_or(i32::MAX),
                licence: &licence,
                source: &source,
            },
            &caller.entry("asset.upload", &detail),
        )
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(no_store(
        (StatusCode::CREATED, Json(asset_json(&row))).into_response(),
    ))
}

/// The form's fields, each checked.
struct Upload {
    licence: String,
    source: String,
    name: Option<String>,
    media_type: Option<String>,
    bytes: axum::body::Bytes,
}

async fn read_upload(mut multipart: Multipart) -> Result<Upload, ApiError> {
    let (mut licence, mut source, mut file) = (None, None, None);
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| multipart_error(&e))?
    {
        match field.name() {
            Some("licence") => licence = Some(field.text().await.map_err(|e| multipart_error(&e))?),
            Some("source") => source = Some(field.text().await.map_err(|e| multipart_error(&e))?),
            Some("file") if file.is_none() => {
                let name = file_name(field.file_name());
                let media_type = field.content_type().map(str::to_owned);
                let bytes = field.bytes().await.map_err(|e| multipart_error(&e))?;
                file = Some((name, media_type, bytes));
            }
            _ => return Err(ApiError::MalformedBody),
        }
    }
    let mut problems = Problems::new();
    let licence = checked_text(
        &mut problems,
        "licence",
        licence.as_deref(),
        200,
        false,
        true,
    );
    let source = checked_text(&mut problems, "source", source.as_deref(), 500, true, true);
    if file.as_ref().is_none_or(|(_, _, bytes)| bytes.is_empty()) {
        problems.add("file", intent_domain::problem::ProblemCode::Required);
    }
    match (licence, source, file) {
        (Some(licence), Some(source), Some((name, media_type, bytes))) if problems.is_empty() => {
            Ok(Upload {
                licence,
                source,
                name,
                media_type,
                bytes,
            })
        }
        _ => Err(ApiError::Problems {
            code: RefusalCode::AssetRefused,
            problems: problems.into_vec(),
        }),
    }
}

fn multipart_error(error: &axum::extract::multipart::MultipartError) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::PayloadTooLarge
    } else {
        ApiError::MalformedBody
    }
}

/// `GET /v1/operator/tenants/{tenant}/assets`.
pub async fn list(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_operator(&app, &headers).await?;
    let tenant = tenant_id(&tenant)?;
    require_tenant(&app, tenant).await?;
    let rows = app.db.assets(tenant).await?;
    let detail = json!({ "tenant": tenant, "rows": rows.len() }).to_string();
    Caller::of(&app, peer, &headers)
        .audit(&app, "assets.read", "operator", &detail)
        .await?;
    let body = json!({
        "store": app.studio.store.is_some(),
        "assets": rows.iter().map(asset_json).collect::<Vec<_>>(),
    });
    Ok(no_store(Json(body).into_response()))
}

/// `GET /v1/operator/tenants/{tenant}/assets/{asset}/content`: the worker's output, served so it
/// can never run as a page: a sandboxing policy, no sniffing, fonts as downloads.
pub async fn content(
    State(app): State<App>,
    Path((tenant, asset)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_operator(&app, &headers).await?;
    let tenant = tenant_id(&tenant)?;
    let asset: AssetId = asset.parse().map_err(|_| ApiError::NotFound)?;
    let row = app
        .db
        .asset(tenant, asset)
        .await?
        .ok_or(ApiError::NotFound)?;
    let bytes = store(&app)?
        .get(row.derived)
        .await
        .map_err(|e| store_error(&e))?
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("an asset's blob is missing")))?;
    let mut response = Body::from(bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&row.media_type).map_err(|_| {
            ApiError::Internal(anyhow::anyhow!("a stored media type is not a header"))
        })?,
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; sandbox; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400, immutable"),
    );
    let disposition = if row.media_type.starts_with("font/") {
        "attachment"
    } else {
        "inline"
    };
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static(disposition),
    );
    Ok(response)
}

fn snapshot_json(row: &SnapshotRow) -> Value {
    json!({
        "id": row.id,
        "brief_id": row.brief_id,
        "url": row.url,
        "outcome": row.outcome,
        "refusal": row.refusal,
        "final_url": row.final_url,
        "sha256": row.hash.map(|h| h.to_string()),
        "bytes": row.bytes,
        "media_type": row.media_type,
        "fetched_at": ts(row.fetched_at),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotRequest {
    /// The reference's index in the brief's newest revision.
    reference: usize,
}

/// `POST /v1/operator/tenants/{tenant}/briefs/{brief}/snapshots` with `{"reference": i}`: fetch
/// the brief's `i`-th reference through the egress guard and keep its bytes. A refusal is
/// recorded too, and answered `422 fetch_refused` with the guard's code.
pub async fn fetch_snapshot(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path((tenant, brief)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    require_operator(&app, &headers).await?;
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !is_json {
        return Err(ApiError::UnsupportedMediaType);
    }
    let bytes = read_body_capped(body, 1024).await?;
    let tenant = tenant_id(&tenant)?;
    let brief_id: BriefId = brief.parse().map_err(|_| ApiError::NotFound)?;
    let request: SnapshotRequest =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::MalformedBody)?;
    let store = store(&app)?;
    let row = app
        .db
        .brief(tenant, brief_id, None)
        .await?
        .ok_or(ApiError::NotFound)?;
    let brief = serde_json::from_str::<BriefForm>(&row.content)
        .ok()
        .and_then(|form| form.validate().ok())
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("a stored brief no longer validates")))?;
    let reference = brief
        .references
        .get(request.reference)
        .ok_or(ApiError::NotFound)?;

    // Outside any transaction (invariant 2): the guard decides, then the outcome is recorded.
    let outcome = app.studio.egress.fetch(&reference.url, Purpose::Page).await;
    let id = SnapshotId::from_uuid(Uuid::now_v7());
    let caller = Caller::of(&app, peer, &headers);
    let (snapshot, refusal) = match outcome {
        Ok(fetched) => {
            let size = i64::try_from(fetched.body.len()).unwrap_or(i64::MAX);
            let hash = store.put(fetched.body).await.map_err(|e| store_error(&e))?;
            let detail = json!({
                "tenant": tenant, "brief": brief_id, "snapshot": id, "outcome": "stored",
                "bytes": size, "redirects": fetched.redirects.len(),
            })
            .to_string();
            let final_url = fetched.final_url.to_string();
            let row = app
                .db
                .record_snapshot(
                    tenant,
                    brief_id,
                    id,
                    &reference.url,
                    &NewSnapshot::Stored {
                        final_url: &final_url,
                        hash,
                        bytes: size,
                        media_type: fetched.content_type.as_deref(),
                    },
                    &caller.entry("snapshot.fetch", &detail),
                )
                .await?;
            (row, None)
        }
        Err(refusal) => {
            let code = refusal.as_str();
            let detail = json!({
                "tenant": tenant, "brief": brief_id, "snapshot": id, "outcome": "refused",
                "refusal": code,
            })
            .to_string();
            let row = app
                .db
                .record_snapshot(
                    tenant,
                    brief_id,
                    id,
                    &reference.url,
                    &NewSnapshot::Refused { code },
                    &caller.entry("snapshot.fetch", &detail),
                )
                .await?;
            (row, Some(code))
        }
    };
    let snapshot = snapshot.ok_or(ApiError::NotFound)?;
    if let Some(reason) = refusal {
        return Err(ApiError::FetchRefused { reason });
    }
    Ok(no_store(
        (StatusCode::CREATED, Json(snapshot_json(&snapshot))).into_response(),
    ))
}

/// `GET /v1/operator/tenants/{tenant}/briefs/{brief}/snapshots`.
pub async fn snapshots(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path((tenant, brief)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_operator(&app, &headers).await?;
    let tenant = tenant_id(&tenant)?;
    let brief: BriefId = brief.parse().map_err(|_| ApiError::NotFound)?;
    if app.db.brief(tenant, brief, None).await?.is_none() {
        return Err(ApiError::NotFound);
    }
    let rows = app.db.snapshots(tenant, brief).await?;
    let detail = json!({ "tenant": tenant, "brief": brief, "rows": rows.len() }).to_string();
    Caller::of(&app, peer, &headers)
        .audit(&app, "snapshots.read", "operator", &detail)
        .await?;
    Ok(no_store(
        Json(json!({
            "store": app.studio.store.is_some(),
            "snapshots": rows.iter().map(snapshot_json).collect::<Vec<_>>(),
        }))
        .into_response(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_file_names_are_reduced_to_a_plain_base_name() {
        assert_eq!(
            file_name(Some("C:\\Users\\pat\\logo.svg")),
            Some("logo.svg".into())
        );
        assert_eq!(file_name(Some("../../etc/passwd")), Some("passwd".into()));
        assert_eq!(file_name(Some("evil\u{202E}gvs.png")), None);
        assert_eq!(file_name(Some("")), None);
        assert_eq!(file_name(None), None);
    }

    #[test]
    fn kinds_come_from_the_media_type_or_a_font_extension() {
        assert_eq!(
            kind_of(Some("image/png"), Some("x.svg")),
            Some(AssetKind::Png)
        );
        assert_eq!(
            kind_of(Some("application/octet-stream"), Some("Inter.TTF")),
            Some(AssetKind::Font)
        );
        assert_eq!(kind_of(None, Some("a.otf")), Some(AssetKind::Font));
        // An extension never upgrades a declared, different type.
        assert_eq!(kind_of(Some("text/html"), Some("a.ttf")), None);
        assert_eq!(kind_of(Some("image/webp"), Some("a.webp")), None);
    }
}
