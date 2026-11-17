//! The studio's records: tenants, fact records and briefs (I1, D-056, D-057).
//!
//! Every query takes a tenant scope (D-015), and every write commits with its audit row in one
//! transaction, so an action either happened and is on record or neither. Checks that depend on
//! other rows — a brief's claims are approved facts of its tenant, an approval is for a fact that
//! is not withdrawn — run inside the same transaction, holding the rows they read.

use std::collections::HashMap;

use intent_domain::{
    brief::Brief,
    fact::{ApprovalForm, FactKind, FactLink, FactLookup, FactRecord, NewFact},
    ids::{AssetId, BriefId, FactId, TenantId},
    problem::{Problem, ProblemCode},
};
use sqlx::{Postgres, Row as _, Transaction, postgres::PgRow};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::{AuditEntry, Db, DbError, classify};

/// Most rows any studio list returns.
pub const LIST_MAX: i64 = 500;

/// A tenant: one client business.
#[derive(Debug, Clone)]
pub struct TenantRow {
    /// The tenant.
    pub id: TenantId,
    /// Its name.
    pub name: String,
    /// When it was created.
    pub created_at: OffsetDateTime,
}

/// A stored fact with everything the workspace shows.
#[derive(Debug, Clone)]
pub struct FactRow {
    /// What the validators use.
    pub record: FactRecord,
    /// Its tenant.
    pub tenant_id: TenantId,
    /// Where it came from.
    pub source: String,
    /// Who approved it, if anyone.
    pub approved_by: Option<String>,
    /// When it was recorded.
    pub created_at: OffsetDateTime,
    /// When the current approval was recorded.
    pub approval_recorded_at: Option<OffsetDateTime>,
    /// When it was withdrawn.
    pub withdrawn_at: Option<OffsetDateTime>,
}

/// What a change to a fact did.
#[derive(Debug)]
pub enum FactChange {
    /// Done; the fact as it now stands.
    Done(FactRow),
    /// No such fact for this tenant.
    NotFound,
    /// The fact was withdrawn and cannot be approved.
    Withdrawn,
    /// The approval broke a rule.
    Problems(Vec<Problem>),
}

/// One stored brief revision.
#[derive(Debug, Clone)]
pub struct BriefRow {
    /// The brief.
    pub id: BriefId,
    /// Its tenant.
    pub tenant_id: TenantId,
    /// This revision.
    pub revision: i32,
    /// The newest revision.
    pub latest_revision: i32,
    /// When the brief was created.
    pub created_at: OffsetDateTime,
    /// When this revision was saved.
    pub revised_at: OffsetDateTime,
    /// The validated brief's JSON, exactly as stored.
    pub content: String,
}

/// One brief in a list.
#[derive(Debug, Clone)]
pub struct BriefSummary {
    /// The brief.
    pub id: BriefId,
    /// The newest revision.
    pub latest_revision: i32,
    /// The newest revision's business name.
    pub business_name: String,
    /// When the brief was created.
    pub created_at: OffsetDateTime,
    /// When its newest revision was saved.
    pub updated_at: OffsetDateTime,
}

/// What saving a brief did.
#[derive(Debug)]
pub enum BriefSave {
    /// Saved as this revision.
    Saved {
        /// The brief.
        id: BriefId,
        /// The revision just written.
        revision: i32,
    },
    /// The claims or the logo broke a rule; nothing was written.
    Problems(Vec<Problem>),
    /// Another revision was saved since the one this change was made from.
    Conflict {
        /// The newest revision.
        latest: i32,
    },
    /// No such brief (or tenant) for this tenant.
    NotFound,
}

/// The fact columns every query reads, as a literal so each statement stays `&'static str`.
macro_rules! fact_columns {
    () => {
        "id, tenant_id, kind, statement, source, link_kind, link_value, created_at, approved_by, \
         approved_on, review_by, approval_recorded_at, withdrawn_at"
    };
}

/// A stored value the schema should have made impossible.
fn corrupt(what: &'static str) -> DbError {
    DbError::Other(sqlx::Error::Protocol(what.into()))
}

fn fact_row(row: &PgRow) -> Result<FactRow, DbError> {
    let kind: String = row.try_get("kind").map_err(DbError::Other)?;
    let kind = FactKind::parse(&kind).ok_or_else(|| corrupt("unknown fact kind"))?;
    let link_kind: Option<String> = row.try_get("link_kind").map_err(DbError::Other)?;
    let link_value: Option<String> = row.try_get("link_value").map_err(DbError::Other)?;
    let link = match (link_kind, link_value) {
        (Some(kind), Some(value)) => {
            Some(FactLink::from_parts(&kind, value).ok_or_else(|| corrupt("unknown link kind"))?)
        }
        _ => None,
    };
    let approved_on: Option<Date> = row.try_get("approved_on").map_err(DbError::Other)?;
    let review_by: Option<Date> = row.try_get("review_by").map_err(DbError::Other)?;
    let withdrawn_at: Option<OffsetDateTime> =
        row.try_get("withdrawn_at").map_err(DbError::Other)?;
    Ok(FactRow {
        record: FactRecord {
            id: FactId::from_uuid(row.try_get("id").map_err(DbError::Other)?),
            kind,
            statement: row.try_get("statement").map_err(DbError::Other)?,
            link,
            approval: approved_on.zip(review_by),
            withdrawn: withdrawn_at.is_some(),
        },
        tenant_id: TenantId::from_uuid(row.try_get("tenant_id").map_err(DbError::Other)?),
        source: row.try_get("source").map_err(DbError::Other)?,
        approved_by: row.try_get("approved_by").map_err(DbError::Other)?,
        created_at: row.try_get("created_at").map_err(DbError::Other)?,
        approval_recorded_at: row
            .try_get("approval_recorded_at")
            .map_err(DbError::Other)?,
        withdrawn_at,
    })
}

pub(crate) async fn audit_in(
    tx: &mut Transaction<'_, Postgres>,
    entry: &AuditEntry<'_>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO operator_audit (action, actor, address, request_id, detail)
         VALUES ($1, $2, $3, $4, $5::jsonb)",
    )
    .bind(entry.action)
    .bind(entry.actor)
    .bind(entry.address)
    .bind(entry.request_id)
    .bind(entry.detail)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(classify)
}

impl Db {
    /// Every tenant, newest first (at most [`LIST_MAX`]).
    pub async fn tenants(&self) -> Result<Vec<TenantRow>, DbError> {
        let rows = sqlx::query(
            "SELECT id, name, created_at FROM tenants ORDER BY created_at DESC, id DESC LIMIT $1",
        )
        .bind(LIST_MAX)
        .fetch_all(&self.pool)
        .await
        .map_err(classify)?;
        rows.iter()
            .map(|row| {
                Ok(TenantRow {
                    id: TenantId::from_uuid(row.try_get("id").map_err(DbError::Other)?),
                    name: row.try_get("name").map_err(DbError::Other)?,
                    created_at: row.try_get("created_at").map_err(DbError::Other)?,
                })
            })
            .collect()
    }

    /// Creates a tenant, with its audit row.
    pub async fn create_tenant(
        &self,
        id: TenantId,
        name: &str,
        audit: &AuditEntry<'_>,
    ) -> Result<TenantRow, DbError> {
        let mut tx = self.pool.begin().await.map_err(classify)?;
        let row =
            sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2) RETURNING created_at")
                .bind(id.as_uuid())
                .bind(name)
                .fetch_one(&mut *tx)
                .await
                .map_err(classify)?;
        audit_in(&mut tx, audit).await?;
        tx.commit().await.map_err(classify)?;
        Ok(TenantRow {
            id,
            name: name.to_owned(),
            created_at: row.try_get("created_at").map_err(DbError::Other)?,
        })
    }

    /// Whether the tenant exists.
    pub async fn tenant_exists(&self, tenant: TenantId) -> Result<bool, DbError> {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tenants WHERE id = $1)")
            .bind(tenant.as_uuid())
            .fetch_one(&self.pool)
            .await
            .map_err(classify)
    }

    /// The tenant's facts, newest first (at most [`LIST_MAX`]).
    pub async fn facts(&self, tenant: TenantId) -> Result<Vec<FactRow>, DbError> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            fact_columns!(),
            " FROM facts WHERE tenant_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2"
        ))
        .bind(tenant.as_uuid())
        .bind(LIST_MAX)
        .fetch_all(&self.pool)
        .await
        .map_err(classify)?;
        rows.iter().map(fact_row).collect()
    }

    /// These facts, if they are the tenant's: another tenant's are simply absent.
    pub async fn fact_lookup(
        &self,
        tenant: TenantId,
        ids: &[FactId],
    ) -> Result<FactLookup, DbError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids: Vec<Uuid> = ids.iter().map(FactId::as_uuid).collect();
        let rows = sqlx::query(concat!(
            "SELECT ",
            fact_columns!(),
            " FROM facts WHERE tenant_id = $1 AND id = ANY($2)"
        ))
        .bind(tenant.as_uuid())
        .bind(&ids)
        .fetch_all(&self.pool)
        .await
        .map_err(classify)?;
        rows.iter()
            .map(|row| fact_row(row).map(|f| (f.record.id, f.record)))
            .collect()
    }

    /// Records a new fact; `None` when the tenant does not exist.
    pub async fn create_fact(
        &self,
        tenant: TenantId,
        id: FactId,
        fact: &NewFact,
        audit: &AuditEntry<'_>,
    ) -> Result<Option<FactRow>, DbError> {
        let mut tx = self.pool.begin().await.map_err(classify)?;
        let row = sqlx::query(concat!(
            "INSERT INTO facts (id, tenant_id, kind, statement, source, link_kind, link_value)
             SELECT $1, t.id, $3, $4, $5, $6, $7 FROM tenants t WHERE t.id = $2
             RETURNING ",
            fact_columns!()
        ))
        .bind(id.as_uuid())
        .bind(tenant.as_uuid())
        .bind(fact.kind.as_str())
        .bind(&fact.statement)
        .bind(&fact.source)
        .bind(fact.link.as_ref().map(FactLink::kind))
        .bind(fact.link.as_ref().map(FactLink::value))
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let row = fact_row(&row)?;
        audit_in(&mut tx, audit).await?;
        tx.commit().await.map_err(classify)?;
        Ok(Some(row))
    }

    /// Records an approval (or a re-confirmation) of a fact, validated for its kind on `today`.
    pub async fn approve_fact(
        &self,
        tenant: TenantId,
        fact: FactId,
        form: ApprovalForm,
        today: Date,
        audit: &AuditEntry<'_>,
    ) -> Result<FactChange, DbError> {
        let mut tx = self.pool.begin().await.map_err(classify)?;
        let Some(current) = locked_fact(&mut tx, tenant, fact).await? else {
            return Ok(FactChange::NotFound);
        };
        if current.record.withdrawn {
            return Ok(FactChange::Withdrawn);
        }
        let approval = match form.validate(current.record.kind, today) {
            Ok(approval) => approval,
            Err(problems) => return Ok(FactChange::Problems(problems)),
        };
        let row = sqlx::query(concat!(
            "UPDATE facts SET approved_by = $3, approved_on = $4, review_by = $5,
                              approval_recorded_at = now()
             WHERE tenant_id = $1 AND id = $2
             RETURNING ",
            fact_columns!()
        ))
        .bind(tenant.as_uuid())
        .bind(fact.as_uuid())
        .bind(&approval.approved_by)
        .bind(approval.approved_on)
        .bind(approval.review_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(classify)?;
        let row = fact_row(&row)?;
        audit_in(&mut tx, audit).await?;
        tx.commit().await.map_err(classify)?;
        Ok(FactChange::Done(row))
    }

    /// Withdraws a fact; withdrawing it again changes nothing and answers the same.
    pub async fn withdraw_fact(
        &self,
        tenant: TenantId,
        fact: FactId,
        audit: &AuditEntry<'_>,
    ) -> Result<FactChange, DbError> {
        let mut tx = self.pool.begin().await.map_err(classify)?;
        let Some(current) = locked_fact(&mut tx, tenant, fact).await? else {
            return Ok(FactChange::NotFound);
        };
        if current.record.withdrawn {
            return Ok(FactChange::Done(current));
        }
        let row = sqlx::query(concat!(
            "UPDATE facts SET withdrawn_at = now() WHERE tenant_id = $1 AND id = $2 RETURNING ",
            fact_columns!()
        ))
        .bind(tenant.as_uuid())
        .bind(fact.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(classify)?;
        let row = fact_row(&row)?;
        audit_in(&mut tx, audit).await?;
        tx.commit().await.map_err(classify)?;
        Ok(FactChange::Done(row))
    }

    /// The tenant's briefs, most recently changed first (at most [`LIST_MAX`]).
    pub async fn briefs(&self, tenant: TenantId) -> Result<Vec<BriefSummary>, DbError> {
        let rows = sqlx::query(
            "SELECT b.id, b.latest_revision, r.business_name, b.created_at, b.updated_at
             FROM briefs b
             JOIN brief_revisions r ON r.brief_id = b.id AND r.revision = b.latest_revision
             WHERE b.tenant_id = $1
             ORDER BY b.updated_at DESC, b.id DESC LIMIT $2",
        )
        .bind(tenant.as_uuid())
        .bind(LIST_MAX)
        .fetch_all(&self.pool)
        .await
        .map_err(classify)?;
        rows.iter()
            .map(|row| {
                Ok(BriefSummary {
                    id: BriefId::from_uuid(row.try_get("id").map_err(DbError::Other)?),
                    latest_revision: row.try_get("latest_revision").map_err(DbError::Other)?,
                    business_name: row.try_get("business_name").map_err(DbError::Other)?,
                    created_at: row.try_get("created_at").map_err(DbError::Other)?,
                    updated_at: row.try_get("updated_at").map_err(DbError::Other)?,
                })
            })
            .collect()
    }

    /// One revision of a brief (the newest when `revision` is `None`), if it is the tenant's.
    pub async fn brief(
        &self,
        tenant: TenantId,
        brief: BriefId,
        revision: Option<i32>,
    ) -> Result<Option<BriefRow>, DbError> {
        let row = sqlx::query(
            "SELECT b.id, b.tenant_id, r.revision, b.latest_revision, b.created_at,
                    r.created_at AS revised_at, r.content::text AS content
             FROM briefs b
             JOIN brief_revisions r ON r.brief_id = b.id
                                   AND r.revision = coalesce($3, b.latest_revision)
             WHERE b.tenant_id = $1 AND b.id = $2",
        )
        .bind(tenant.as_uuid())
        .bind(brief.as_uuid())
        .bind(revision)
        .fetch_optional(&self.pool)
        .await
        .map_err(classify)?;
        row.map(|row| {
            Ok(BriefRow {
                id: BriefId::from_uuid(row.try_get("id").map_err(DbError::Other)?),
                tenant_id: TenantId::from_uuid(row.try_get("tenant_id").map_err(DbError::Other)?),
                revision: row.try_get("revision").map_err(DbError::Other)?,
                latest_revision: row.try_get("latest_revision").map_err(DbError::Other)?,
                created_at: row.try_get("created_at").map_err(DbError::Other)?,
                revised_at: row.try_get("revised_at").map_err(DbError::Other)?,
                content: row.try_get("content").map_err(DbError::Other)?,
            })
        })
        .transpose()
    }

    /// Saves a new brief as revision 1, or a new revision of an existing one when `expected` is
    /// the revision the change was made from. Claims and logo are checked inside the
    /// transaction, holding the facts they read.
    pub async fn save_brief(
        &self,
        tenant: TenantId,
        id: BriefId,
        expected: Option<i32>,
        brief: &Brief,
        today: Date,
        audit: &AuditEntry<'_>,
    ) -> Result<BriefSave, DbError> {
        let content =
            serde_json::to_string(brief).map_err(|_| corrupt("brief did not serialize"))?;
        let mut tx = self.pool.begin().await.map_err(classify)?;
        let revision = match expected {
            None => {
                let created = sqlx::query(
                    "INSERT INTO briefs (id, tenant_id) SELECT $1, t.id FROM tenants t WHERE t.id = $2",
                )
                .bind(id.as_uuid())
                .bind(tenant.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(classify)?;
                if created.rows_affected() == 0 {
                    return Ok(BriefSave::NotFound);
                }
                1
            }
            Some(expected) => {
                let latest: Option<i32> = sqlx::query_scalar(
                    "SELECT latest_revision FROM briefs WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
                )
                .bind(tenant.as_uuid())
                .bind(id.as_uuid())
                .fetch_optional(&mut *tx)
                .await
                .map_err(classify)?;
                match latest {
                    None => return Ok(BriefSave::NotFound),
                    Some(latest) if latest != expected => {
                        return Ok(BriefSave::Conflict { latest });
                    }
                    Some(latest) => latest + 1,
                }
            }
        };

        // The claims must be approved, current facts of this tenant, read under a share lock so
        // none is withdrawn or re-approved until this commits.
        let claim_ids: Vec<Uuid> = brief.claims.iter().map(FactId::as_uuid).collect();
        let rows = sqlx::query(concat!(
            "SELECT ",
            fact_columns!(),
            " FROM facts WHERE tenant_id = $1 AND id = ANY($2) FOR SHARE"
        ))
        .bind(tenant.as_uuid())
        .bind(&claim_ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(classify)?;
        let lookup: FactLookup = rows
            .iter()
            .map(|row| fact_row(row).map(|f| (f.record.id, f.record)))
            .collect::<Result<_, _>>()?;
        let mut problems = brief.check_claims(&lookup, today);
        if let Some(logo) = brief.brand.logo
            && !asset_is_tenants(&mut tx, tenant, logo).await?
        {
            problems.push(Problem::new("brand.logo", ProblemCode::AssetUnknown));
        }
        if !problems.is_empty() {
            return Ok(BriefSave::Problems(problems));
        }

        if revision > 1 {
            sqlx::query(
                "UPDATE briefs SET latest_revision = $3, updated_at = now()
                 WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant.as_uuid())
            .bind(id.as_uuid())
            .bind(revision)
            .execute(&mut *tx)
            .await
            .map_err(classify)?;
        }
        sqlx::query(
            "INSERT INTO brief_revisions (tenant_id, brief_id, revision, business_name, content)
             VALUES ($1, $2, $3, $4, $5::jsonb)",
        )
        .bind(tenant.as_uuid())
        .bind(id.as_uuid())
        .bind(revision)
        .bind(&brief.business_name)
        .bind(&content)
        .execute(&mut *tx)
        .await
        .map_err(classify)?;
        sqlx::query(
            "INSERT INTO brief_claims (tenant_id, brief_id, revision, fact_id)
             SELECT $1, $2, $3, unnest($4::uuid[])",
        )
        .bind(tenant.as_uuid())
        .bind(id.as_uuid())
        .bind(revision)
        .bind(&claim_ids)
        .execute(&mut *tx)
        .await
        .map_err(classify)?;
        audit_in(&mut tx, audit).await?;
        tx.commit().await.map_err(classify)?;
        Ok(BriefSave::Saved { id, revision })
    }
}

async fn locked_fact(
    tx: &mut Transaction<'_, Postgres>,
    tenant: TenantId,
    fact: FactId,
) -> Result<Option<FactRow>, DbError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        fact_columns!(),
        " FROM facts WHERE tenant_id = $1 AND id = $2 FOR UPDATE"
    ))
    .bind(tenant.as_uuid())
    .bind(fact.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify)?;
    row.as_ref().map(fact_row).transpose()
}

async fn asset_is_tenants(
    tx: &mut Transaction<'_, Postgres>,
    tenant: TenantId,
    asset: AssetId,
) -> Result<bool, DbError> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM assets WHERE tenant_id = $1 AND id = $2)")
        .bind(tenant.as_uuid())
        .bind(asset.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(classify)
}
