//! Candidates and check records (I2, D-010, D-022).
//!
//! A candidate is one built bundle: its SHA-256, the validated tree it came
//! from, and the exact builder/model versions. A check record is the suite's
//! evidence against one bundle — written with the candidate's own hash, bound
//! to it by a composite foreign key so a record can never silently describe a
//! different build. Promotion's use of the pair (refuse without a passing
//! record for the exact hash) is I3; the binding itself is enforced here, at
//! the schema.

use intent_domain::{
    content::ContentHash,
    ids::{BriefId, TenantId},
};
use sqlx::Row as _;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{AuditEntry, Db, DbError, classify, studio::audit_in};

/// A stored candidate.
#[derive(Debug, Clone)]
pub struct CandidateRow {
    /// The candidate.
    pub id: Uuid,
    /// Whose site it is for.
    pub tenant_id: TenantId,
    /// The brief it was generated for.
    pub brief_id: BriefId,
    /// The bundle's SHA-256 over its canonical framing.
    pub bundle_sha256: ContentHash,
    /// The page tree's own SHA-256 (D-010); `None` only on pre-0010 rows.
    pub page_tree_sha256: Option<ContentHash>,
    /// The validated page tree that produced it.
    pub tree: serde_json::Value,
    /// The component set (`quote-request@0`).
    pub component_set: String,
    /// `baseline@1` or the generation job's builder version.
    pub builder: String,
    /// The model, when one built it; `None` for the deterministic baseline.
    pub model: Option<String>,
    /// When it was recorded.
    pub created_at: OffsetDateTime,
}

/// A new candidate: what [`Db::insert_candidate`] writes.
#[derive(Debug)]
pub struct NewCandidate<'a> {
    /// The candidate id (minted at the edge, v7).
    pub id: Uuid,
    /// Whose site it is for.
    pub tenant: TenantId,
    /// The brief it was generated for.
    pub brief: BriefId,
    /// The validated page tree.
    pub tree: &'a serde_json::Value,
    /// The bundle's hash.
    pub bundle_sha256: ContentHash,
    /// `baseline@1` or the generation job's builder version.
    pub builder: &'a str,
    /// The model, when one built it.
    pub model: Option<&'a str>,
}

/// A new check record: what [`Db::insert_check_record`] writes.
#[derive(Debug)]
pub struct NewCheckRecord<'a> {
    /// The record id.
    pub id: Uuid,
    /// The candidate checked.
    pub candidate_id: Uuid,
    /// The hash the results describe — the candidate's own.
    pub bundle_sha256: ContentHash,
    /// The suite (`d-022@0`).
    pub suite: &'a str,
    /// `static`, `chromium` or `webkit`.
    pub engine: &'a str,
    /// `static` or a viewport like `360x800`.
    pub viewport: &'a str,
    /// Per-check outcomes — the verdict is derived from these, never passed
    /// in (I2-4: a caller cannot claim a pass the evidence does not hold).
    pub results: &'a serde_json::Value,
}

/// A stored check record.
#[derive(Debug, Clone)]
pub struct CheckRecordRow {
    /// The record.
    pub id: Uuid,
    /// The candidate checked.
    pub candidate_id: Uuid,
    /// The candidate's tenant — the record's scope (I2-7).
    pub tenant_id: TenantId,
    /// The bundle the results describe — the candidate's own hash, always.
    pub bundle_sha256: ContentHash,
    /// The suite and its version (`d-022@0`).
    pub suite: String,
    /// `static`, `chromium` or `webkit`.
    pub engine: String,
    /// `static` or a viewport like `360x800`.
    pub viewport: String,
    /// Per-check outcomes as stored.
    pub results: serde_json::Value,
    /// `pass`, `fail` or `pending`.
    pub verdict: String,
    /// When the suite ran.
    pub ran_at: OffsetDateTime,
}

fn content_hash(row: &sqlx::postgres::PgRow, column: &str) -> Result<ContentHash, DbError> {
    let bytes: Vec<u8> = row.try_get(column).map_err(DbError::Other)?;
    ContentHash::from_slice(&bytes)
        .ok_or_else(|| DbError::Other(sqlx::Error::Protocol("bad sha256".into())))
}

fn candidate_row(row: &sqlx::postgres::PgRow) -> Result<CandidateRow, DbError> {
    Ok(CandidateRow {
        id: row.try_get("id").map_err(DbError::Other)?,
        tenant_id: TenantId::from_uuid(row.try_get("tenant_id").map_err(DbError::Other)?),
        brief_id: BriefId::from_uuid(row.try_get("brief_id").map_err(DbError::Other)?),
        bundle_sha256: content_hash(row, "bundle_sha256")?,
        page_tree_sha256: row
            .try_get::<Option<Vec<u8>>, _>("page_tree_sha256")
            .map_err(DbError::Other)?
            .and_then(|b| ContentHash::from_slice(&b)),
        tree: row.try_get("tree").map_err(DbError::Other)?,
        component_set: row.try_get("component_set").map_err(DbError::Other)?,
        builder: row.try_get("builder").map_err(DbError::Other)?,
        model: row.try_get("model").map_err(DbError::Other)?,
        created_at: row.try_get("created_at").map_err(DbError::Other)?,
    })
}

fn check_row(row: &sqlx::postgres::PgRow) -> Result<CheckRecordRow, DbError> {
    Ok(CheckRecordRow {
        id: row.try_get("id").map_err(DbError::Other)?,
        candidate_id: row.try_get("candidate_id").map_err(DbError::Other)?,
        tenant_id: TenantId::from_uuid(row.try_get("tenant_id").map_err(DbError::Other)?),
        bundle_sha256: content_hash(row, "bundle_sha256")?,
        suite: row.try_get("suite").map_err(DbError::Other)?,
        engine: row.try_get("engine").map_err(DbError::Other)?,
        viewport: row.try_get("viewport").map_err(DbError::Other)?,
        results: row.try_get("results").map_err(DbError::Other)?,
        verdict: row.try_get("verdict").map_err(DbError::Other)?,
        ran_at: row.try_get("ran_at").map_err(DbError::Other)?,
    })
}

impl Db {
    /// Records a candidate. `None` when the brief is not this tenant's — the
    /// scope is part of the insert, not a separate check.
    pub async fn insert_candidate(
        &self,
        new: &NewCandidate<'_>,
        audit: &AuditEntry<'_>,
    ) -> Result<Option<CandidateRow>, DbError> {
        // The tree hash is derived, never supplied — the provenance column
        // names the exact bytes the approval binds (D-010).
        let tree_bytes = serde_json::to_vec(new.tree)
            .map_err(|_| DbError::Other(sqlx::Error::Protocol("tree not json".into())))?;
        let tree_hash = ContentHash::of(&tree_bytes);
        let mut tx = self.pool.begin().await.map_err(classify)?;
        let row = sqlx::query(
            "INSERT INTO candidates
             (id, tenant_id, brief_id, bundle_sha256, page_tree_sha256, tree,
              component_set, builder, model)
             SELECT $1, t.id, b.id, $4, $5, $6, $7, $8, $9
             FROM tenants t JOIN briefs b ON b.tenant_id = t.id AND b.id = $3
             WHERE t.id = $2
             RETURNING id, tenant_id, brief_id, bundle_sha256, page_tree_sha256, tree,
                       component_set, builder, model, created_at",
        )
        .bind(new.id)
        .bind(new.tenant.as_uuid())
        .bind(new.brief.as_uuid())
        .bind(new.bundle_sha256.as_bytes().as_slice())
        .bind(tree_hash.as_bytes().as_slice())
        .bind(new.tree)
        .bind(intent_domain::page_tree::COMPONENT_SET)
        .bind(new.builder)
        .bind(new.model)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let row = candidate_row(&row)?;
        audit_in(&mut tx, audit).await?;
        tx.commit().await.map_err(classify)?;
        Ok(Some(row))
    }

    /// The candidate, or `None` (another tenant's is absent, not an error).
    pub async fn candidate(
        &self,
        tenant: TenantId,
        id: Uuid,
    ) -> Result<Option<CandidateRow>, DbError> {
        let row = sqlx::query(
            "SELECT id, tenant_id, brief_id, bundle_sha256, page_tree_sha256, tree,
                    component_set, builder, model, created_at
             FROM candidates WHERE id = $1 AND tenant_id = $2",
        )
        .bind(id)
        .bind(tenant.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(classify)?;
        row.as_ref().map(candidate_row).transpose()
    }

    /// Records one backend cell's results against a candidate. `bundle_sha256`
    /// is the hash the caller believes ran; the composite foreign key makes a
    /// wrong answer a refused write, not a quiet mismatch (D-010). The verdict
    /// is derived here from the results themselves — unparseable or evidence-
    /// free results are refused (I2-4).
    pub async fn insert_check_record(
        &self,
        new: &NewCheckRecord<'_>,
        audit: &AuditEntry<'_>,
    ) -> Result<CheckRecordRow, DbError> {
        let results: Vec<intent_domain::checks::CheckResult> =
            serde_json::from_value(new.results.clone()).map_err(|_| {
                DbError::Other(sqlx::Error::Protocol(
                    "results not a check-results array".into(),
                ))
            })?;
        if results.is_empty() {
            return Err(DbError::Other(sqlx::Error::Protocol(
                "results empty".into(),
            )));
        }
        let verdict = intent_domain::checks::CheckVerdict::of_results(&results);
        // The tenant is selected from the candidate, never supplied — a record
        // can only ever land under its candidate's own scope (I2-7).
        let mut tx = self.pool.begin().await.map_err(classify)?;
        let row = sqlx::query(
            "INSERT INTO check_records
             (id, candidate_id, tenant_id, bundle_sha256, suite, engine, viewport,
              results, verdict)
             SELECT $1, c.id, c.tenant_id, $3, $4, $5, $6, $7, $8
             FROM candidates c WHERE c.id = $2
             RETURNING id, candidate_id, tenant_id, bundle_sha256, suite, engine,
                       viewport, results, verdict, ran_at",
        )
        .bind(new.id)
        .bind(new.candidate_id)
        .bind(new.bundle_sha256.as_bytes().as_slice())
        .bind(new.suite)
        .bind(new.engine)
        .bind(new.viewport)
        .bind(new.results)
        .bind(verdict.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(classify)?;
        let row = check_row(&row)?;
        audit_in(&mut tx, audit).await?;
        tx.commit().await.map_err(classify)?;
        Ok(row)
    }

    /// A candidate's check records, newest first.
    pub async fn check_records(&self, candidate_id: Uuid) -> Result<Vec<CheckRecordRow>, DbError> {
        let rows = sqlx::query(
            "SELECT id, candidate_id, tenant_id, bundle_sha256, suite, engine, viewport,
                    results, verdict, ran_at
             FROM check_records
             WHERE candidate_id = $1 ORDER BY ran_at DESC, id DESC",
        )
        .bind(candidate_id)
        .fetch_all(&self.pool)
        .await
        .map_err(classify)?;
        rows.iter().map(check_row).collect()
    }
}
