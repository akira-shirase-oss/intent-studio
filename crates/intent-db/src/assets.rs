//! Assets and reference snapshots (I1, D-016, D-060, D-062): who owns which stored bytes, under
//! which licence, from where. The bytes are in the content-addressed store; these rows are the
//! authority on what they are and may be used for.

use intent_domain::{
    content::ContentHash,
    ids::{AssetId, BriefId, SnapshotId, TenantId},
};
use sqlx::{Row as _, postgres::PgRow};
use time::OffsetDateTime;

use crate::{AuditEntry, Db, DbError, classify, studio::audit_in};

/// An asset to record, after the worker produced its output and both blobs are stored.
#[derive(Debug, Clone)]
pub struct NewAsset<'a> {
    /// The asset.
    pub id: AssetId,
    /// Its tenant.
    pub tenant: TenantId,
    /// The upload, exactly as received.
    pub original: ContentHash,
    /// Its size.
    pub original_bytes: i64,
    /// The media type it was uploaded as.
    pub original_media_type: &'a str,
    /// Its file name, when the browser sent a usable one.
    pub original_name: Option<&'a str>,
    /// What the worker produced.
    pub derived: ContentHash,
    /// Its size.
    pub bytes: i64,
    /// Its media type.
    pub media_type: &'a str,
    /// Pixel width, for images.
    pub width: Option<i32>,
    /// Pixel height, for images.
    pub height: Option<i32>,
    /// What the sanitizer dropped.
    pub removed_parts: i32,
    /// The licence it may be used under.
    pub licence: &'a str,
    /// Where it came from.
    pub source: &'a str,
}

/// A stored asset.
#[derive(Debug, Clone)]
pub struct AssetRow {
    /// The asset.
    pub id: AssetId,
    /// Its tenant.
    pub tenant_id: TenantId,
    /// The original upload.
    pub original: ContentHash,
    /// Its size.
    pub original_bytes: i64,
    /// Its media type.
    pub original_media_type: String,
    /// Its file name.
    pub original_name: Option<String>,
    /// The worker's output, which is what is served.
    pub derived: ContentHash,
    /// Its size.
    pub bytes: i64,
    /// Its media type.
    pub media_type: String,
    /// Pixel width.
    pub width: Option<i32>,
    /// Pixel height.
    pub height: Option<i32>,
    /// What the sanitizer dropped.
    pub removed_parts: i32,
    /// The licence.
    pub licence: String,
    /// The source.
    pub source: String,
    /// When it was recorded.
    pub created_at: OffsetDateTime,
}

/// One reference fetch, stored or refused.
#[derive(Debug, Clone)]
pub struct SnapshotRow {
    /// The snapshot.
    pub id: SnapshotId,
    /// The brief whose reference it is.
    pub brief_id: BriefId,
    /// The address asked for.
    pub url: String,
    /// `stored` or `refused`.
    pub outcome: String,
    /// Why it was refused (the egress code).
    pub refusal: Option<String>,
    /// Where the last redirect led.
    pub final_url: Option<String>,
    /// The stored bytes.
    pub hash: Option<ContentHash>,
    /// Their size.
    pub bytes: Option<i64>,
    /// Their declared media type.
    pub media_type: Option<String>,
    /// When it was fetched.
    pub fetched_at: OffsetDateTime,
}

/// A fetch outcome to record.
#[derive(Debug, Clone)]
pub enum NewSnapshot<'a> {
    /// The bytes are stored under `hash`.
    Stored {
        /// The address after redirects.
        final_url: &'a str,
        /// The blob.
        hash: ContentHash,
        /// Its size.
        bytes: i64,
        /// The declared media type.
        media_type: Option<&'a str>,
    },
    /// The guard or the fetch refused, with this code.
    Refused {
        /// The egress code.
        code: &'a str,
    },
}

fn hash_of(row: &PgRow, column: &str) -> Result<ContentHash, DbError> {
    let bytes: Vec<u8> = row.try_get(column).map_err(DbError::Other)?;
    ContentHash::from_slice(&bytes).ok_or_else(|| {
        DbError::Other(sqlx::Error::Protocol(
            "a stored hash is not 32 bytes".into(),
        ))
    })
}

const ASSET_COLUMNS: &str = "id, tenant_id, original_sha256, original_bytes, original_media_type, \
     original_name, sha256, bytes, media_type, width, height, removed_parts, licence, source, \
     created_at";

fn asset_row(row: &PgRow) -> Result<AssetRow, DbError> {
    Ok(AssetRow {
        id: AssetId::from_uuid(row.try_get("id").map_err(DbError::Other)?),
        tenant_id: TenantId::from_uuid(row.try_get("tenant_id").map_err(DbError::Other)?),
        original: hash_of(row, "original_sha256")?,
        original_bytes: row.try_get("original_bytes").map_err(DbError::Other)?,
        original_media_type: row.try_get("original_media_type").map_err(DbError::Other)?,
        original_name: row.try_get("original_name").map_err(DbError::Other)?,
        derived: hash_of(row, "sha256")?,
        bytes: row.try_get("bytes").map_err(DbError::Other)?,
        media_type: row.try_get("media_type").map_err(DbError::Other)?,
        width: row.try_get("width").map_err(DbError::Other)?,
        height: row.try_get("height").map_err(DbError::Other)?,
        removed_parts: row.try_get("removed_parts").map_err(DbError::Other)?,
        licence: row.try_get("licence").map_err(DbError::Other)?,
        source: row.try_get("source").map_err(DbError::Other)?,
        created_at: row.try_get("created_at").map_err(DbError::Other)?,
    })
}

fn snapshot_row(row: &PgRow) -> Result<SnapshotRow, DbError> {
    let hash: Option<Vec<u8>> = row.try_get("sha256").map_err(DbError::Other)?;
    Ok(SnapshotRow {
        id: SnapshotId::from_uuid(row.try_get("id").map_err(DbError::Other)?),
        brief_id: BriefId::from_uuid(row.try_get("brief_id").map_err(DbError::Other)?),
        url: row.try_get("url").map_err(DbError::Other)?,
        outcome: row.try_get("outcome").map_err(DbError::Other)?,
        refusal: row.try_get("refusal").map_err(DbError::Other)?,
        final_url: row.try_get("final_url").map_err(DbError::Other)?,
        hash: hash.as_deref().and_then(ContentHash::from_slice),
        bytes: row.try_get("bytes").map_err(DbError::Other)?,
        media_type: row.try_get("media_type").map_err(DbError::Other)?,
        fetched_at: row.try_get("fetched_at").map_err(DbError::Other)?,
    })
}

impl Db {
    /// Records an asset with its audit row; `None` when the tenant does not exist.
    pub async fn create_asset(
        &self,
        asset: &NewAsset<'_>,
        audit: &AuditEntry<'_>,
    ) -> Result<Option<AssetRow>, DbError> {
        let mut tx = self.pool.begin().await.map_err(classify)?;
        let sql = format!(
            "INSERT INTO assets (id, tenant_id, original_sha256, original_bytes,
                                 original_media_type, original_name, sha256, bytes, media_type,
                                 width, height, removed_parts, licence, source)
             SELECT $1, t.id, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14
             FROM tenants t WHERE t.id = $2
             RETURNING {ASSET_COLUMNS}"
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(asset.id.as_uuid())
            .bind(asset.tenant.as_uuid())
            .bind(asset.original.as_bytes().as_slice())
            .bind(asset.original_bytes)
            .bind(asset.original_media_type)
            .bind(asset.original_name)
            .bind(asset.derived.as_bytes().as_slice())
            .bind(asset.bytes)
            .bind(asset.media_type)
            .bind(asset.width)
            .bind(asset.height)
            .bind(asset.removed_parts)
            .bind(asset.licence)
            .bind(asset.source)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let row = asset_row(&row)?;
        audit_in(&mut tx, audit).await?;
        tx.commit().await.map_err(classify)?;
        Ok(Some(row))
    }

    /// The tenant's assets, newest first (at most [`crate::LIST_MAX`]).
    pub async fn assets(&self, tenant: TenantId) -> Result<Vec<AssetRow>, DbError> {
        let sql = format!(
            "SELECT {ASSET_COLUMNS} FROM assets WHERE tenant_id = $1
             ORDER BY created_at DESC, id DESC LIMIT $2"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(tenant.as_uuid())
            .bind(crate::LIST_MAX)
            .fetch_all(&self.pool)
            .await
            .map_err(classify)?;
        rows.iter().map(asset_row).collect()
    }

    /// One asset, if it is the tenant's.
    pub async fn asset(&self, tenant: TenantId, id: AssetId) -> Result<Option<AssetRow>, DbError> {
        let sql = format!("SELECT {ASSET_COLUMNS} FROM assets WHERE tenant_id = $1 AND id = $2");
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(tenant.as_uuid())
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(classify)?;
        row.as_ref().map(asset_row).transpose()
    }

    /// Records a reference fetch, stored or refused, with its audit row.
    pub async fn record_snapshot(
        &self,
        tenant: TenantId,
        brief: BriefId,
        id: SnapshotId,
        url: &str,
        outcome: &NewSnapshot<'_>,
        audit: &AuditEntry<'_>,
    ) -> Result<Option<SnapshotRow>, DbError> {
        let mut tx = self.pool.begin().await.map_err(classify)?;
        let (kind, refusal, final_url, hash, bytes, media_type) = match outcome {
            NewSnapshot::Stored {
                final_url,
                hash,
                bytes,
                media_type,
            } => (
                "stored",
                None,
                Some(*final_url),
                Some(hash.as_bytes().to_vec()),
                Some(*bytes),
                *media_type,
            ),
            NewSnapshot::Refused { code } => ("refused", Some(*code), None, None, None, None),
        };
        let row = sqlx::query(
            "INSERT INTO reference_snapshots (id, tenant_id, brief_id, url, outcome, refusal,
                                              final_url, sha256, bytes, media_type)
             SELECT $1, b.tenant_id, b.id, $4, $5, $6, $7, $8, $9, $10
             FROM briefs b WHERE b.tenant_id = $2 AND b.id = $3
             RETURNING id, brief_id, url, outcome, refusal, final_url, sha256, bytes, media_type,
                       fetched_at",
        )
        .bind(id.as_uuid())
        .bind(tenant.as_uuid())
        .bind(brief.as_uuid())
        .bind(url)
        .bind(kind)
        .bind(refusal)
        .bind(final_url)
        .bind(hash)
        .bind(bytes)
        .bind(media_type)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let row = snapshot_row(&row)?;
        audit_in(&mut tx, audit).await?;
        tx.commit().await.map_err(classify)?;
        Ok(Some(row))
    }

    /// A brief's reference snapshots, newest first (at most [`crate::LIST_MAX`]).
    pub async fn snapshots(
        &self,
        tenant: TenantId,
        brief: BriefId,
    ) -> Result<Vec<SnapshotRow>, DbError> {
        let rows = sqlx::query(
            "SELECT id, brief_id, url, outcome, refusal, final_url, sha256, bytes, media_type,
                    fetched_at
             FROM reference_snapshots WHERE tenant_id = $1 AND brief_id = $2
             ORDER BY fetched_at DESC, id DESC LIMIT $3",
        )
        .bind(tenant.as_uuid())
        .bind(brief.as_uuid())
        .bind(crate::LIST_MAX)
        .fetch_all(&self.pool)
        .await
        .map_err(classify)?;
        rows.iter().map(snapshot_row).collect()
    }
}
