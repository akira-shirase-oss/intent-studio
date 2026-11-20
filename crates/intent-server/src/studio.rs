//! The studio API (I1): tenants, fact records, briefs and the page-tree check.
//!
//! Every route is the operator's, behind the session (D-015), and answers `401` before it reads
//! a byte of the body. Writes are JSON only (a cross-site form cannot send JSON without a
//! preflight), and each commits with its audit row. Nothing here logs record content: the
//! records are the client's business, and the only things logged are refusal codes (D-029).

use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use intent_db::{BriefRow, BriefSave, FactChange, FactRow};
use intent_domain::{
    brief::BriefForm,
    claims::LINT_VERSION,
    fact::{ApprovalForm, FactForm, format_date},
    ids::{BriefId, FactId, TenantId},
    page_tree::{self, COMPONENT_SET, PageTree, SpecContext},
    problem::{Problem, ProblemCode, checked_text},
    refusal::RefusalCode,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::{
    app::{App, Peer},
    error::ApiError,
    intake::read_body_capped,
    operator::{Caller, no_store, require_operator},
};

/// Largest tenant, fact or approval body.
pub const SMALL_BODY_BYTES: usize = 16 * 1024;
/// Largest brief body (a full brief is well under half of this).
pub const BRIEF_BODY_BYTES: usize = 128 * 1024;
/// Largest page-tree body.
pub const SPEC_BODY_BYTES: usize = 256 * 1024;

/// The server's calendar day (UTC): the only clock the fact rules see (D-056).
pub fn today() -> time::Date {
    OffsetDateTime::now_utc().date()
}

fn ts(t: OffsetDateTime) -> Option<String> {
    t.format(&Rfc3339).ok()
}

/// 401 first, then JSON only, then the capped body.
async fn operator_json(
    app: &App,
    peer: Peer,
    headers: &HeaderMap,
    body: Body,
    cap: usize,
) -> Result<axum::body::Bytes, ApiError> {
    require_operator(app, peer, headers).await?;
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !is_json {
        return Err(ApiError::UnsupportedMediaType);
    }
    read_body_capped(body, cap).await
}

/// Parses JSON into `T`; a shape error is one `malformed` problem at its path (never the serde
/// message, which can quote input).
fn parse<T: DeserializeOwned>(bytes: &[u8], code: RefusalCode) -> Result<T, ApiError> {
    let malformed = |path: String| ApiError::Problems {
        code,
        problems: vec![Problem::new(path, ProblemCode::Malformed)],
    };
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value: T = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let path = error.path().to_string();
        // A path is built from the operator's own keys; keep it bounded all the same.
        malformed(if path == "." {
            String::new()
        } else {
            path.chars().take(200).collect()
        })
    })?;
    // Trailing bytes after the value are malformed too.
    deserializer.end().map_err(|_| malformed(String::new()))?;
    Ok(value)
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

fn fact_json(fact: &FactRow, today: time::Date) -> Value {
    let record = &fact.record;
    let status = record.status(today);
    json!({
        "id": record.id,
        "tenant_id": fact.tenant_id,
        "kind": record.kind.as_str(),
        "statement": record.statement,
        "source": fact.source,
        "link": record.link.as_ref().map(|l| json!({ "kind": l.kind(), "value": l.value(), "href": l.href() })),
        "status": status.as_str(),
        "approved_by": fact.approved_by,
        "approved_on": record.approval.map(|(on, _)| format_date(on)),
        "review_by": record.approval.map(|(_, by)| format_date(by)),
        "review_days": record.kind.review_days(),
        "created_at": ts(fact.created_at),
        "approval_recorded_at": fact.approval_recorded_at.and_then(ts),
        "withdrawn_at": fact.withdrawn_at.and_then(ts),
    })
}

/// `GET /v1/operator/tenants`.
pub async fn tenants(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_operator(&app, peer, &headers).await?;
    let tenants = app.db.tenants().await?;
    let detail = json!({ "rows": tenants.len() }).to_string();
    Caller::of(&app, peer, &headers)
        .audit(&app, "tenants.read", "operator", &detail)
        .await?;
    let body = json!({
        "tenants": tenants.iter().map(|t| json!({ "id": t.id, "name": t.name, "created_at": ts(t.created_at) })).collect::<Vec<_>>(),
    });
    Ok(no_store(Json(body).into_response()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewTenant {
    name: Option<String>,
}

/// `POST /v1/operator/tenants` with `{"name": "..."}`.
pub async fn create_tenant(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let bytes = operator_json(&app, peer, &headers, body, SMALL_BODY_BYTES).await?;
    let form: NewTenant = parse(&bytes, RefusalCode::TenantInvalid)?;
    let mut problems = intent_domain::problem::Problems::new();
    let name = checked_text(
        &mut problems,
        "name",
        form.name.as_deref(),
        200,
        false,
        true,
    );
    let Some(name) = name else {
        return Err(ApiError::Problems {
            code: RefusalCode::TenantInvalid,
            problems: problems.into_vec(),
        });
    };
    let id = TenantId::from_uuid(Uuid::now_v7());
    let caller = Caller::of(&app, peer, &headers);
    let detail = json!({ "tenant": id }).to_string();
    let row = app
        .db
        .create_tenant(id, &name, &caller.entry("tenant.create", &detail))
        .await?;
    let body = json!({ "id": row.id, "name": row.name, "created_at": ts(row.created_at) });
    Ok(no_store((StatusCode::CREATED, Json(body)).into_response()))
}

/// `GET /v1/operator/tenants/{tenant}/facts`.
pub async fn facts(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_operator(&app, peer, &headers).await?;
    let tenant = tenant_id(&tenant)?;
    require_tenant(&app, tenant).await?;
    let rows = app.db.facts(tenant).await?;
    let detail = json!({ "tenant": tenant, "rows": rows.len() }).to_string();
    Caller::of(&app, peer, &headers)
        .audit(&app, "facts.read", "operator", &detail)
        .await?;
    let today = today();
    let body = json!({
        "today": format_date(today),
        "facts": rows.iter().map(|f| fact_json(f, today)).collect::<Vec<_>>(),
        "truncated": i64::try_from(rows.len()).is_ok_and(|n| n >= intent_db::LIST_MAX),
    });
    Ok(no_store(Json(body).into_response()))
}

/// `POST /v1/operator/tenants/{tenant}/facts`.
pub async fn create_fact(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let bytes = operator_json(&app, peer, &headers, body, SMALL_BODY_BYTES).await?;
    let tenant = tenant_id(&tenant)?;
    let form: FactForm = parse(&bytes, RefusalCode::FactInvalid)?;
    let fact = form.validate().map_err(|problems| ApiError::Problems {
        code: RefusalCode::FactInvalid,
        problems,
    })?;
    let id = FactId::from_uuid(Uuid::now_v7());
    let caller = Caller::of(&app, peer, &headers);
    let detail = json!({ "tenant": tenant, "fact": id, "kind": fact.kind.as_str() }).to_string();
    let row = app
        .db
        .create_fact(tenant, id, &fact, &caller.entry("fact.create", &detail))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(no_store(
        (StatusCode::CREATED, Json(fact_json(&row, today()))).into_response(),
    ))
}

fn fact_change(change: FactChange) -> Result<Response, ApiError> {
    match change {
        FactChange::Done(row) => Ok(no_store(Json(fact_json(&row, today())).into_response())),
        FactChange::NotFound => Err(ApiError::NotFound),
        FactChange::Withdrawn => Err(ApiError::FactWithdrawn),
        FactChange::Problems(problems) => Err(ApiError::Problems {
            code: RefusalCode::ApprovalInvalid,
            problems,
        }),
    }
}

/// `POST /v1/operator/tenants/{tenant}/facts/{fact}/approval`.
pub async fn approve_fact(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path((tenant, fact)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let bytes = operator_json(&app, peer, &headers, body, SMALL_BODY_BYTES).await?;
    let tenant = tenant_id(&tenant)?;
    let fact: FactId = fact.parse().map_err(|_| ApiError::NotFound)?;
    let form: ApprovalForm = parse(&bytes, RefusalCode::ApprovalInvalid)?;
    let caller = Caller::of(&app, peer, &headers);
    let detail = json!({ "tenant": tenant, "fact": fact }).to_string();
    let change = app
        .db
        .approve_fact(
            tenant,
            fact,
            form,
            today(),
            &caller.entry("fact.approve", &detail),
        )
        .await?;
    fact_change(change)
}

/// `POST /v1/operator/tenants/{tenant}/facts/{fact}/withdrawal` (no body).
pub async fn withdraw_fact(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path((tenant, fact)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_operator(&app, peer, &headers).await?;
    let tenant = tenant_id(&tenant)?;
    let fact: FactId = fact.parse().map_err(|_| ApiError::NotFound)?;
    let caller = Caller::of(&app, peer, &headers);
    let detail = json!({ "tenant": tenant, "fact": fact }).to_string();
    let change = app
        .db
        .withdraw_fact(tenant, fact, &caller.entry("fact.withdraw", &detail))
        .await?;
    fact_change(change)
}

/// `GET /v1/operator/tenants/{tenant}/briefs`.
pub async fn briefs(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_operator(&app, peer, &headers).await?;
    let tenant = tenant_id(&tenant)?;
    require_tenant(&app, tenant).await?;
    let rows = app.db.briefs(tenant).await?;
    let detail = json!({ "tenant": tenant, "rows": rows.len() }).to_string();
    Caller::of(&app, peer, &headers)
        .audit(&app, "briefs.read", "operator", &detail)
        .await?;
    let body = json!({
        "briefs": rows.iter().map(|b| json!({
            "id": b.id,
            "business_name": b.business_name,
            "latest_revision": b.latest_revision,
            "created_at": ts(b.created_at),
            "updated_at": ts(b.updated_at),
        })).collect::<Vec<_>>(),
    });
    Ok(no_store(Json(body).into_response()))
}

fn brief_save(save: BriefSave, status: StatusCode) -> Result<Response, ApiError> {
    match save {
        BriefSave::Saved { id, revision } => Ok(no_store(
            (status, Json(json!({ "id": id, "revision": revision }))).into_response(),
        )),
        BriefSave::Problems(problems) => Err(ApiError::Problems {
            code: RefusalCode::BriefInvalid,
            problems,
        }),
        BriefSave::Conflict { latest } => Err(ApiError::RevisionConflict { latest }),
        BriefSave::NotFound => Err(ApiError::NotFound),
    }
}

/// `POST /v1/operator/tenants/{tenant}/briefs` with the brief: revision 1.
pub async fn create_brief(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let bytes = operator_json(&app, peer, &headers, body, BRIEF_BODY_BYTES).await?;
    let tenant = tenant_id(&tenant)?;
    let form: BriefForm = parse(&bytes, RefusalCode::BriefInvalid)?;
    let brief = form.validate().map_err(|problems| ApiError::Problems {
        code: RefusalCode::BriefInvalid,
        problems,
    })?;
    let id = BriefId::from_uuid(Uuid::now_v7());
    let caller = Caller::of(&app, peer, &headers);
    let detail =
        json!({ "tenant": tenant, "brief": id, "revision": 1, "claims": brief.claims.len() })
            .to_string();
    let save = app
        .db
        .save_brief(
            tenant,
            id,
            None,
            &brief,
            today(),
            &caller.entry("brief.create", &detail),
        )
        .await?;
    brief_save(save, StatusCode::CREATED)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    expected_revision: i32,
    brief: BriefForm,
}

/// `PUT /v1/operator/tenants/{tenant}/briefs/{brief}` with `{"expected_revision": n, "brief": {...}}`.
pub async fn revise_brief(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path((tenant, brief)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let bytes = operator_json(&app, peer, &headers, body, BRIEF_BODY_BYTES).await?;
    let tenant = tenant_id(&tenant)?;
    let id: BriefId = brief.parse().map_err(|_| ApiError::NotFound)?;
    let Revision {
        expected_revision,
        brief,
    } = parse(&bytes, RefusalCode::BriefInvalid)?;
    let brief = brief.validate().map_err(|problems| ApiError::Problems {
        code: RefusalCode::BriefInvalid,
        problems: problems
            .into_iter()
            .map(|p| Problem {
                path: intent_domain::problem::field("brief", &p.path),
                ..p
            })
            .collect(),
    })?;
    let caller = Caller::of(&app, peer, &headers);
    let detail = json!({
        "tenant": tenant, "brief": id, "from_revision": expected_revision, "claims": brief.claims.len()
    })
    .to_string();
    let save = app
        .db
        .save_brief(
            tenant,
            id,
            Some(expected_revision),
            &brief,
            today(),
            &caller.entry("brief.revise", &detail),
        )
        .await?;
    brief_save(save, StatusCode::OK)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionParam {
    revision: Option<i32>,
}

fn brief_json(row: &BriefRow) -> Result<Value, ApiError> {
    let content: Value = serde_json::from_str(&row.content)
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("a stored brief is not JSON")))?;
    Ok(json!({
        "id": row.id,
        "tenant_id": row.tenant_id,
        "revision": row.revision,
        "latest_revision": row.latest_revision,
        "created_at": ts(row.created_at),
        "revised_at": ts(row.revised_at),
        "brief": content,
    }))
}

/// Loads a brief revision and re-validates it: a stored brief is never trusted as-is.
async fn load_brief(
    app: &App,
    tenant: TenantId,
    id: BriefId,
    revision: Option<i32>,
) -> Result<(BriefRow, intent_domain::brief::Brief), ApiError> {
    let row = app
        .db
        .brief(tenant, id, revision)
        .await?
        .ok_or(ApiError::NotFound)?;
    let brief = serde_json::from_str::<BriefForm>(&row.content)
        .ok()
        .and_then(|form| form.validate().ok())
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("a stored brief no longer validates")))?;
    Ok((row, brief))
}

/// `GET /v1/operator/tenants/{tenant}/briefs/{brief}?revision=n` (the newest without it).
pub async fn brief(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path((tenant, brief)): Path<(String, String)>,
    headers: HeaderMap,
    params: Result<Query<RevisionParam>, QueryRejection>,
) -> Result<Response, ApiError> {
    require_operator(&app, peer, &headers).await?;
    let tenant = tenant_id(&tenant)?;
    let id: BriefId = brief.parse().map_err(|_| ApiError::NotFound)?;
    let Query(params) = params.map_err(|_| ApiError::MalformedBody)?;
    let (row, _) = load_brief(&app, tenant, id, params.revision).await?;
    let detail = json!({ "tenant": tenant, "brief": id, "revision": row.revision }).to_string();
    Caller::of(&app, peer, &headers)
        .audit(&app, "brief.read", "operator", &detail)
        .await?;
    Ok(no_store(Json(brief_json(&row)?).into_response()))
}

/// `POST /v1/operator/tenants/{tenant}/briefs/{brief}/spec-check?revision=n` with a page tree:
/// `200` when it validates against the brief and the tenant's facts today, else
/// `422 spec_invalid` with every problem by path. Nothing is stored (D-055).
pub async fn spec_check(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    Path((tenant, brief)): Path<(String, String)>,
    headers: HeaderMap,
    params: Result<Query<RevisionParam>, QueryRejection>,
    body: Body,
) -> Result<Response, ApiError> {
    let bytes = operator_json(&app, peer, &headers, body, SPEC_BODY_BYTES).await?;
    let tenant = tenant_id(&tenant)?;
    let id: BriefId = brief.parse().map_err(|_| ApiError::NotFound)?;
    let Query(params) = params.map_err(|_| ApiError::MalformedBody)?;
    let tree: PageTree = parse(&bytes, RefusalCode::SpecInvalid)?;
    let (row, brief) = load_brief(&app, tenant, id, params.revision).await?;
    let mut wanted = tree.referenced_facts();
    wanted.extend(brief.claims.iter().copied());
    let facts = app.db.fact_lookup(tenant, &wanted).await?;
    let problems = page_tree::validate(
        &tree,
        SpecContext {
            brief: &brief,
            facts: &facts,
            today: today(),
        },
    );
    let detail = json!({
        "tenant": tenant, "brief": id, "revision": row.revision, "problems": problems.len()
    })
    .to_string();
    Caller::of(&app, peer, &headers)
        .audit(&app, "spec.check", "operator", &detail)
        .await?;
    if !problems.is_empty() {
        return Err(ApiError::Problems {
            code: RefusalCode::SpecInvalid,
            problems,
        });
    }
    Ok(no_store(
        Json(json!({
            "valid": true,
            "brief_revision": row.revision,
            "component_set": COMPONENT_SET,
            "lint": LINT_VERSION,
            "facts_checked": tree.referenced_facts().len(),
        }))
        .into_response(),
    ))
}
