//! The operator inbox: leads newest first with their notification state. The inbox is the
//! truth about leads; a notification is only a copy (invariant 5).

use intent_domain::{
    ids::{LeadId, SiteId, TenantId},
    outbox::OutboxState,
    state::LeadState,
};
use sqlx::Row as _;
use time::OffsetDateTime;

use crate::{Db, DbError, classify};

/// A page request.
#[derive(Debug, Clone, Copy)]
pub struct InboxQuery {
    /// Only this site's leads.
    pub site: Option<SiteId>,
    /// Leads older than this one (keyset pagination).
    pub before: Option<LeadId>,
    /// Page size (the caller bounds it).
    pub limit: u16,
}

/// One inbox row.
#[derive(Clone)]
pub struct InboxLead {
    /// The lead.
    pub id: LeadId,
    /// Its tenant.
    pub tenant_id: TenantId,
    /// Its site.
    pub site_id: SiteId,
    /// The site's name.
    pub site_name: String,
    /// When it arrived.
    pub received_at: OffsetDateTime,
    /// When it will be deleted.
    pub retain_until: OffsetDateTime,
    /// Content.
    pub name: String,
    /// Content.
    pub email: Option<String>,
    /// Content.
    pub phone: Option<String>,
    /// Content.
    pub service: Option<String>,
    /// Content.
    pub area: Option<String>,
    /// Content.
    pub message: String,
    /// A preview lead (D-027).
    pub is_test: bool,
    /// The lead's state, from its outbox row.
    pub state: LeadState,
    /// Attempts made so far.
    pub attempts: u8,
    /// Attempts that may have reached the provider unconfirmed.
    pub uncertain_attempts: u8,
    /// The last attempt's error code.
    pub last_error: Option<String>,
    /// When the provider accepted it.
    pub delivered_at: Option<OffsetDateTime>,
    /// When the next attempt is due (pending rows).
    pub next_attempt_at: Option<OffsetDateTime>,
}

impl std::fmt::Debug for InboxLead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxLead")
            .field("id", &self.id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

const PAGE: &str = r"
SELECT l.id, l.tenant_id, l.site_id, s.name AS site_name, l.received_at, l.retain_until,
       l.name, l.email, l.phone, l.service, l.area, l.message, l.is_test,
       o.state, o.attempts, o.uncertain_attempts, o.last_error, o.delivered_at,
       CASE WHEN o.state = 'pending' THEN o.next_attempt_at END AS next_attempt_at
FROM leads l
JOIN sites s ON s.id = l.site_id
LEFT JOIN LATERAL (
    SELECT state, attempts, uncertain_attempts, last_error, delivered_at, next_attempt_at
    FROM notification_outbox WHERE lead_id = l.id ORDER BY created_at DESC LIMIT 1
) o ON true
WHERE ($1::uuid IS NULL OR l.site_id = $1)
  AND ($2::uuid IS NULL OR (l.received_at, l.id) < (SELECT received_at, id FROM leads WHERE id = $2))
ORDER BY l.received_at DESC, l.id DESC
LIMIT $3";

impl Db {
    /// One page of the inbox.
    pub async fn inbox_page(&self, query: InboxQuery) -> Result<Vec<InboxLead>, DbError> {
        let rows = sqlx::query(PAGE)
            .bind(query.site.map(|s| s.as_uuid()))
            .bind(query.before.map(|l| l.as_uuid()))
            .bind(i64::from(query.limit))
            .fetch_all(&self.pool)
            .await
            .map_err(classify)?;
        rows.iter()
            .map(|row| -> Result<InboxLead, sqlx::Error> {
                let state: Option<String> = row.try_get("state")?;
                let attempts: Option<i16> = row.try_get("attempts")?;
                let uncertain: Option<i16> = row.try_get("uncertain_attempts")?;
                let attempts = attempts.and_then(|a| u8::try_from(a).ok()).unwrap_or(0);
                let outbox = state.as_deref().and_then(OutboxState::parse);
                Ok(InboxLead {
                    id: LeadId::from_uuid(row.try_get("id")?),
                    tenant_id: TenantId::from_uuid(row.try_get("tenant_id")?),
                    site_id: SiteId::from_uuid(row.try_get("site_id")?),
                    site_name: row.try_get("site_name")?,
                    received_at: row.try_get("received_at")?,
                    retain_until: row.try_get("retain_until")?,
                    name: row.try_get("name")?,
                    email: row.try_get("email")?,
                    phone: row.try_get("phone")?,
                    service: row.try_get("service")?,
                    area: row.try_get("area")?,
                    message: row.try_get("message")?,
                    is_test: row.try_get("is_test")?,
                    // A lead always has its outbox row (same statement); `Stored` is the safe
                    // reading if one were ever missing.
                    state: outbox.map_or(LeadState::Stored, |o| LeadState::of_stored(o, attempts)),
                    attempts,
                    uncertain_attempts: uncertain.and_then(|a| u8::try_from(a).ok()).unwrap_or(0),
                    last_error: row.try_get("last_error")?,
                    delivered_at: row.try_get("delivered_at")?,
                    next_attempt_at: row.try_get("next_attempt_at")?,
                })
            })
            .collect::<Result<_, _>>()
            .map_err(DbError::Other)
    }

    /// Records a cross-tenant operator action (D-015). `detail` is JSON with ids and counts only.
    pub async fn audit(&self, action: &str, detail: &str) -> Result<(), DbError> {
        sqlx::query("INSERT INTO operator_audit (action, detail) VALUES ($1, $2::jsonb)")
            .bind(action)
            .bind(detail)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(classify)
    }
}
