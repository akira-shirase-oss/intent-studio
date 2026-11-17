//! The lead + outbox commit (invariant 5) and idempotent replays.

use intent_domain::{
    ids::{LeadId, OutboxId, SiteId},
    lead::ValidatedLead,
    limits::DERIVED_KEY_WINDOW_SECS,
    submission::{Fingerprint, Replay, StoredKey, replay},
};
use sqlx::Row as _;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Db, DbError, classify};

/// Everything needed to store one lead.
#[derive(Debug)]
pub struct NewLead<'a> {
    /// The new lead's id (UUID v7, chosen by the caller).
    pub id: LeadId,
    /// The new outbox row's id: the provider's idempotency key.
    pub outbox_id: OutboxId,
    /// The site it was submitted to.
    pub site: SiteId,
    /// The key it is stored under.
    pub key: &'a StoredKey,
    /// For derived keys: the previous window's key, checked first (see
    /// [`intent_domain::submission::SubmissionKeys`]).
    pub previous_window: Option<&'a StoredKey>,
    /// The content fingerprint.
    pub fingerprint: Fingerprint,
    /// The validated content.
    pub lead: &'a ValidatedLead,
    /// When the server received it.
    pub received_at: OffsetDateTime,
}

/// What storing did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    /// The lead and its outbox row committed now.
    Created(LeadId),
    /// The same content was already stored under this key: this is that lead.
    Duplicate(LeadId),
    /// The key was already used for different content.
    Conflict,
    /// No such site, or it is not accepting leads.
    UnknownSite,
}

/// Inserts the lead and its outbox row in one statement: both commit, or neither exists. The
/// retention instant comes from the tenant's setting in the same statement.
const INSERT_LEAD_AND_OUTBOX: &str = r"
WITH site AS (
    SELECT s.id, s.tenant_id, t.retention_days
    FROM sites s JOIN tenants t ON t.id = s.tenant_id
    WHERE s.id = $1 AND s.accepting_leads
), lead AS (
    INSERT INTO leads (id, tenant_id, site_id, submission_key, fingerprint, name, email, phone,
                       message, service, area, consent, received_at, retain_until)
    SELECT $2, site.tenant_id, site.id, $3, $4, $5, $6, $7, $8, $9, $10, true, $11,
           $11 + make_interval(days => site.retention_days::int)
    FROM site
    ON CONFLICT (site_id, submission_key) DO NOTHING
    RETURNING id, tenant_id
)
INSERT INTO notification_outbox (id, tenant_id, lead_id, next_attempt_at, created_at)
SELECT $12, lead.tenant_id, lead.id, now(), now() FROM lead
RETURNING lead_id";

impl Db {
    /// Stores a lead with its outbox row, or reports the replay. A [`StoreOutcome::Created`]
    /// return means the commit happened; any error means nothing may be acknowledged.
    pub async fn store_lead(&self, new: &NewLead<'_>) -> Result<StoreOutcome, DbError> {
        if let Some(previous) = new.previous_window
            && let Some(outcome) = self.previous_window_match(new, previous).await?
        {
            return Ok(outcome);
        }
        let lead = new.lead;
        let created = sqlx::query(INSERT_LEAD_AND_OUTBOX)
            .bind(new.site.as_uuid())
            .bind(new.id.as_uuid())
            .bind(new.key.as_str())
            .bind(new.fingerprint.as_bytes().as_slice())
            .bind(lead.name())
            .bind(lead.email())
            .bind(lead.phone())
            .bind(lead.message())
            .bind(lead.service())
            .bind(lead.area())
            .bind(new.received_at)
            .bind(new.outbox_id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(classify)?;
        if created.is_some() {
            return Ok(StoreOutcome::Created(new.id));
        }
        self.explain_no_insert(new).await
    }

    /// A derived key from the previous window that arrived within the window length is the same
    /// lead (two identical no-script submits straddling a boundary).
    async fn previous_window_match(
        &self,
        new: &NewLead<'_>,
        previous: &StoredKey,
    ) -> Result<Option<StoreOutcome>, DbError> {
        let row = sqlx::query(
            "SELECT id, fingerprint FROM leads
             WHERE site_id = $1 AND submission_key = $2
               AND received_at > $3",
        )
        .bind(new.site.as_uuid())
        .bind(previous.as_str())
        .bind(new.received_at - time::Duration::seconds(DERIVED_KEY_WINDOW_SECS))
        .fetch_optional(&self.pool)
        .await
        .map_err(classify)?;
        row.map(|row| replay_outcome(&row, &new.fingerprint))
            .transpose()
    }

    /// Nothing was inserted: the key exists (a replay or a conflict) or the site does not.
    async fn explain_no_insert(&self, new: &NewLead<'_>) -> Result<StoreOutcome, DbError> {
        let existing = sqlx::query(
            "SELECT id, fingerprint FROM leads WHERE site_id = $1 AND submission_key = $2",
        )
        .bind(new.site.as_uuid())
        .bind(new.key.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(classify)?;
        if let Some(row) = existing {
            return replay_outcome(&row, &new.fingerprint);
        }
        // No lead under the key, so the site gate stopped the insert. (A site deleted between the
        // two statements reads the same way, which is the right answer.)
        Ok(StoreOutcome::UnknownSite)
    }
}

fn replay_outcome(
    row: &sqlx::postgres::PgRow,
    incoming: &Fingerprint,
) -> Result<StoreOutcome, DbError> {
    let id: Uuid = row.try_get("id").map_err(DbError::Other)?;
    let stored: Vec<u8> = row.try_get("fingerprint").map_err(DbError::Other)?;
    let Some(stored) = Fingerprint::from_slice(&stored) else {
        return Err(DbError::Other(sqlx::Error::Decode(
            "stored fingerprint is not 32 bytes".into(),
        )));
    };
    Ok(match replay(&stored, incoming) {
        Replay::SameLead => StoreOutcome::Duplicate(LeadId::from_uuid(id)),
        Replay::Conflict => StoreOutcome::Conflict,
    })
}
