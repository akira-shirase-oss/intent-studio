//! Bounded rate-limit tables (D-019): token buckets keyed by site or client address, at most
//! `capacity` keys, least recently used evicted first.
//!
//! The table is split into shards, each behind a `std::sync::Mutex` held for a few integer
//! operations and never across an `.await`. Each shard is its own LRU with `capacity / shards`
//! entries, which approximates one global LRU. An evicted key comes back with a full bucket: the
//! bound trades exactness under a flood of distinct keys for memory that cannot grow.

use std::{
    hash::{BuildHasher, Hash},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use hashlink::LruCache;
use intent_domain::ratelimit::{Quota, RetryAfter, TokenBucket};

const SHARDS: usize = 16;

/// A bounded table of token buckets.
pub struct Limiter<K> {
    quota: Quota,
    shards: Vec<Mutex<LruCache<K, TokenBucket>>>,
    hasher: std::collections::hash_map::RandomState,
    evictions: AtomicU64,
    refusals: AtomicU64,
}

impl<K: Hash + Eq + Copy> Limiter<K> {
    /// A table of at most `capacity` keys (rounded up to a multiple of the shard count).
    pub fn new(quota: Quota, capacity: usize) -> Self {
        let per_shard = capacity.div_ceil(SHARDS).max(1);
        Self {
            quota,
            shards: (0..SHARDS)
                .map(|_| Mutex::new(LruCache::new(per_shard)))
                .collect(),
            hasher: std::collections::hash_map::RandomState::new(),
            evictions: AtomicU64::new(0),
            refusals: AtomicU64::new(0),
        }
    }

    /// Takes one token for `key` at `now_ms`.
    pub fn check(&self, key: K, now_ms: u64) -> Result<(), RetryAfter> {
        let shard = &self.shards[self.shard_of(&key)];
        let result = {
            let mut table = shard
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(bucket) = table.get_mut(&key) {
                bucket.try_take(self.quota, now_ms)
            } else {
                let mut bucket = TokenBucket::full(self.quota, now_ms);
                let result = bucket.try_take(self.quota, now_ms);
                if table.len() >= table.capacity() {
                    self.evictions.fetch_add(1, Ordering::Relaxed);
                }
                // `insert` evicts the least recently used entry when the shard is full.
                table.insert(key, bucket);
                result
            }
        };
        if result.is_err() {
            self.refusals.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// Gives back the token a request took, if its key is still held.
    pub fn refund(&self, key: K) {
        let shard = &self.shards[self.shard_of(&key)];
        let mut table = shard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(bucket) = table.get_mut(&key) {
            bucket.refund(self.quota);
        }
    }

    fn shard_of(&self, key: &K) -> usize {
        // The modulo keeps the index in range whatever the hash.
        usize::try_from(self.hasher.hash_one(key) % SHARDS as u64).unwrap_or(0)
    }

    /// Keys currently held.
    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|s| {
                s.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .len()
            })
            .sum()
    }

    /// Whether no key is held.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Keys evicted to stay within the bound.
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    /// Takes refused.
    pub fn refusals(&self) -> u64 {
        self.refusals.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_limited_to_its_quota() {
        let limiter = Limiter::new(Quota::per_minute(5).unwrap(), 1_000);
        for _ in 0..5 {
            assert!(limiter.check(1_u32, 0).is_ok());
        }
        assert_eq!(limiter.check(1_u32, 0), Err(RetryAfter(12)));
        assert!(
            limiter.check(2_u32, 0).is_ok(),
            "other keys are independent"
        );
        assert_eq!(limiter.refusals(), 1);
    }

    #[test]
    fn a_refund_gives_one_token_back() {
        let limiter = Limiter::new(Quota::per_minute(1).unwrap(), 1_000);
        assert!(limiter.check(7_u32, 0).is_ok());
        assert!(limiter.check(7_u32, 0).is_err());
        limiter.refund(7);
        assert!(limiter.check(7_u32, 0).is_ok());
        limiter.refund(8); // an unknown key: nothing is created
        assert_eq!(limiter.len(), 1);
    }

    #[test]
    fn the_table_never_grows_past_its_bound() {
        let limiter = Limiter::new(Quota::per_minute(5).unwrap(), 1_600);
        for key in 0..100_000_u32 {
            let _ = limiter.check(key, 0);
        }
        assert!(limiter.len() <= 1_600, "{}", limiter.len());
        assert!(limiter.evictions() >= 100_000 - 1_600);
    }

    #[test]
    fn a_busy_key_survives_a_flood_of_new_keys_in_its_shard() {
        // The busy key is touched between every new key, so it is never least recently used.
        let limiter = Limiter::new(Quota::per_minute(1_000_000).unwrap(), 160);
        let _ = limiter.check(u32::MAX, 0);
        for key in 0..10_000_u32 {
            let _ = limiter.check(key, 0);
            let _ = limiter.check(u32::MAX, 0);
        }
        let shard = &limiter.shards[limiter.shard_of(&u32::MAX)];
        assert!(shard.lock().unwrap().contains_key(&u32::MAX));
    }
}
