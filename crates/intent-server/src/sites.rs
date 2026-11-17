//! Which sites accept leads, so the per-site budget is charged only for real sites (F2).
//!
//! A bounded cache of sites verified to accept leads, each entry trusted for `ttl`; a miss
//! costs one primary-key lookup. Unknown and closed sites are never cached and never get a
//! rate-limit row, so random site ids cannot grow any table. The store statement stays the
//! authority: a site closed within the TTL is still refused there.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use intent_db::{Db, DbError};
use intent_domain::ids::SiteId;

/// The cache.
pub struct SiteDirectory {
    verified: Mutex<HashMap<SiteId, Instant>>,
    capacity: usize,
    ttl: Duration,
}

impl SiteDirectory {
    /// At most `capacity` sites, each trusted for `ttl`.
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            verified: Mutex::new(HashMap::new()),
            capacity,
            ttl,
        }
    }

    /// Whether `site` exists and accepts leads.
    pub async fn accepts(&self, db: &Db, site: SiteId) -> Result<bool, DbError> {
        let fresh = {
            let verified = self
                .verified
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            verified
                .get(&site)
                .is_some_and(|at| at.elapsed() < self.ttl)
        };
        if fresh {
            return Ok(true);
        }
        let accepts = db.site_accepts_leads(site).await? == Some(true);
        let mut verified = self
            .verified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if accepts {
            if verified.len() >= self.capacity {
                // Bounded: start over rather than grow (sites are few; this is a backstop).
                verified.clear();
            }
            verified.insert(site, Instant::now());
        } else {
            verified.remove(&site);
        }
        Ok(accepts)
    }
}
