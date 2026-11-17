//! Demo data for development, drills and load runs: one tenant and `n` sites with ids anyone can
//! compute (`00000000-0000-4000-9000-<index as 12 hex digits>`), so a k6 script can address them.

use intent_domain::ids::{SiteId, TenantId};
use uuid::Uuid;

use crate::{Db, DbError, classify};

/// The demo tenant.
pub const DEMO_TENANT: TenantId =
    TenantId::from_uuid(Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0001));

/// The demo site with this index.
pub const fn demo_site(index: u32) -> SiteId {
    SiteId::from_uuid(Uuid::from_u128(
        0x0000_0000_0000_4000_9000_0000_0000_0000 | index as u128,
    ))
}

impl Db {
    /// Creates the demo tenant and sites `0..sites` if they do not exist.
    pub async fn seed_demo(&self, sites: u32) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO tenants (id, name) VALUES ($1, 'Demo tenant') ON CONFLICT (id) DO NOTHING",
        )
        .bind(DEMO_TENANT.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        let mut start = 0;
        while start < sites {
            let end = sites.min(start.saturating_add(5_000));
            let ids: Vec<Uuid> = (start..end).map(|i| demo_site(i).as_uuid()).collect();
            let names: Vec<String> = (start..end).map(|i| format!("Demo site {i}")).collect();
            let recipients: Vec<String> = (start..end)
                .map(|i| format!("owner+{i}@example.invalid"))
                .collect();
            sqlx::query(
                "INSERT INTO sites (id, tenant_id, name, notify_to)
                 SELECT id, $1, name, notify_to
                 FROM unnest($2::uuid[], $3::text[], $4::text[]) AS s (id, name, notify_to)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(DEMO_TENANT.as_uuid())
            .bind(&ids)
            .bind(&names)
            .bind(&recipients)
            .execute(&self.pool)
            .await
            .map_err(classify)?;
            start = end;
        }
        Ok(())
    }

    /// Whether a site exists (`Some`) and accepts leads (`true`).
    pub async fn site_accepts_leads(&self, site: SiteId) -> Result<Option<bool>, DbError> {
        sqlx::query_scalar("SELECT accepting_leads FROM sites WHERE id = $1")
            .bind(site.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(classify)
    }

    /// Stops or resumes a site's intake (used by tests of the unknown/closed-site refusal).
    pub async fn set_accepting(&self, site: SiteId, accepting: bool) -> Result<(), DbError> {
        sqlx::query("UPDATE sites SET accepting_leads = $2 WHERE id = $1")
            .bind(site.as_uuid())
            .bind(accepting)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(classify)
    }
}
