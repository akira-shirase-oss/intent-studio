//! Abuse limits as pure arithmetic (D-019): token buckets, the client address and its key.
//!
//! Time is a parameter (milliseconds on a monotonic clock), so every rule is tested without
//! sleeping. Token arithmetic is integer: one token is [`UNITS_PER_TOKEN`] units and a bucket
//! with a quota of `q` leads per minute refills `q` units per millisecond, so no float rounding
//! can let an extra lead through.

use std::net::IpAddr;

/// Units in one token: one minute in milliseconds.
pub const UNITS_PER_TOKEN: u64 = 60_000;

/// Tokens per window; also the burst. A per-minute quota refills `tokens` units per
/// millisecond; a longer window refills `tokens * UNITS_PER_TOKEN` per `window_ms`, in
/// integer arithmetic that errs on the side of granting less.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    tokens: u32,
    window_ms: u64,
}

impl Quota {
    /// A quota of at least one token per minute.
    pub fn per_minute(tokens: u32) -> Option<Self> {
        (tokens > 0).then_some(Self {
            tokens,
            window_ms: 60_000,
        })
    }

    /// A quota of at least one token per day (24 hours).
    pub fn per_day(tokens: u32) -> Option<Self> {
        (tokens > 0).then_some(Self {
            tokens,
            window_ms: 86_400_000,
        })
    }

    /// Tokens in the window.
    pub const fn get(self) -> u32 {
        self.tokens
    }

    const fn capacity(self) -> u64 {
        self.tokens as u64 * UNITS_PER_TOKEN
    }

    /// Units refilled over `elapsed_ms` (truncation loses less than one unit).
    fn refill(self, elapsed_ms: u64) -> u64 {
        let units = u128::from(elapsed_ms) * u128::from(self.tokens) * u128::from(UNITS_PER_TOKEN)
            / u128::from(self.window_ms);
        u64::try_from(units).unwrap_or(u64::MAX)
    }

    /// Milliseconds until `missing` units have refilled (rounded up).
    fn wait_ms(self, missing: u64) -> u64 {
        let rate_num = u128::from(self.tokens) * u128::from(UNITS_PER_TOKEN);
        let wait = (u128::from(missing) * u128::from(self.window_ms)).div_ceil(rate_num);
        u64::try_from(wait).unwrap_or(u64::MAX)
    }
}

/// A refused take: retry after this many whole seconds (rounded up, at least 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryAfter(pub u64);

/// One key's bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenBucket {
    level: u64,
    updated_ms: u64,
}

impl TokenBucket {
    /// A full bucket at `now_ms`.
    pub const fn full(quota: Quota, now_ms: u64) -> Self {
        Self {
            level: quota.capacity(),
            updated_ms: now_ms,
        }
    }

    /// Takes one token, or says how long until one is available. A clock that goes backwards
    /// refills nothing.
    pub fn try_take(&mut self, quota: Quota, now_ms: u64) -> Result<(), RetryAfter> {
        let elapsed = now_ms.saturating_sub(self.updated_ms);
        let refill = quota.refill(elapsed);
        self.level = self.level.saturating_add(refill).min(quota.capacity());
        self.updated_ms = self.updated_ms.max(now_ms);
        if self.level >= UNITS_PER_TOKEN {
            self.level -= UNITS_PER_TOKEN;
            return Ok(());
        }
        let missing = UNITS_PER_TOKEN - self.level;
        let wait_ms = quota.wait_ms(missing);
        Err(RetryAfter(wait_ms.div_ceil(1_000).max(1)))
    }
}

impl TokenBucket {
    /// Gives back one token taken for a request that turned out not to count (a replay of a
    /// stored lead, a conflict), never above the burst.
    pub fn refund(&mut self, quota: Quota) {
        self.level = self
            .level
            .saturating_add(UNITS_PER_TOKEN)
            .min(quota.capacity());
    }
}

/// The address a request is limited by.
///
/// The peer address, unless the peer is a configured proxy: then `X-Forwarded-For` is read from
/// the right, skipping configured proxies, and the first other address is the client. A header
/// that does not parse falls back to the peer, so a broken proxy limits itself, not a stranger.
pub fn client_address(
    peer: IpAddr,
    forwarded_for: Option<&str>,
    trusted_proxies: &[IpAddr],
) -> IpAddr {
    let peer = canonical(peer);
    if !trusted_proxies.contains(&peer) {
        return peer;
    }
    let Some(header) = forwarded_for else {
        return peer;
    };
    for hop in header.rsplit(',') {
        let Ok(address) = hop.trim().parse::<IpAddr>().map(canonical) else {
            return peer;
        };
        if !trusted_proxies.contains(&address) {
            return address;
        }
    }
    peer
}

/// IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are the IPv4 address.
fn canonical(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 @ IpAddr::V4(_) => v4,
    }
}

/// The rate-limit key for an address: the whole IPv4 address, or the IPv6 /64 a single
/// subscriber usually controls (otherwise one host could rotate through 2^64 keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressKey {
    /// An IPv4 address.
    V4(u32),
    /// The upper 64 bits of an IPv6 address.
    V6Prefix(u64),
}

impl AddressKey {
    /// The key for an address.
    pub fn of(address: IpAddr) -> Self {
        match canonical(address) {
            IpAddr::V4(v4) => Self::V4(u32::from(v4)),
            IpAddr::V6(v6) => {
                let [a, b, c, d, ..] = v6.segments();
                Self::V6Prefix(
                    (u64::from(a) << 48)
                        | (u64::from(b) << 32)
                        | (u64::from(c) << 16)
                        | u64::from(d),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn five() -> Quota {
        Quota::per_minute(5).unwrap()
    }

    #[test]
    fn a_full_bucket_allows_the_burst_then_refuses() {
        let mut bucket = TokenBucket::full(five(), 0);
        for _ in 0..5 {
            assert_eq!(bucket.try_take(five(), 0), Ok(()));
        }
        // One token refills every 12 s at 5 a minute.
        assert_eq!(bucket.try_take(five(), 0), Err(RetryAfter(12)));
    }

    #[test]
    fn tokens_refill_at_the_quota_rate() {
        let mut bucket = TokenBucket::full(five(), 0);
        for _ in 0..5 {
            bucket.try_take(five(), 0).unwrap();
        }
        assert_eq!(bucket.try_take(five(), 11_999), Err(RetryAfter(1)));
        assert_eq!(bucket.try_take(five(), 12_000), Ok(()));
        assert!(bucket.try_take(five(), 12_000).is_err());
        // A long idle period refills only up to the burst.
        let later = 12_000 + 3_600_000;
        for _ in 0..5 {
            assert_eq!(bucket.try_take(five(), later), Ok(()));
        }
        assert!(bucket.try_take(five(), later).is_err());
    }

    #[test]
    fn a_daily_quota_refills_at_its_own_rate() {
        let quota = Quota::per_day(100).unwrap();
        let mut bucket = TokenBucket::full(quota, 0);
        for _ in 0..100 {
            assert_eq!(bucket.try_take(quota, 0), Ok(()));
        }
        // 100 a day is one token every 864 s; the retry hint names the real wait.
        let Err(RetryAfter(wait)) = bucket.try_take(quota, 0) else {
            panic!("the 101st take should be refused")
        };
        assert_eq!(wait, 864);
        // A refused look spends the accrual up to its instant: at 863_999 ms the level is
        // one sub-token unit short, and the remaining unit lands inside the next 15 ms.
        assert!(bucket.try_take(quota, 863_999).is_err());
        assert_eq!(bucket.try_take(quota, 864_015), Ok(()));
        // Two days of idle refill only the burst, never more.
        let later = 2 * 86_400_000;
        for _ in 0..100 {
            assert_eq!(bucket.try_take(quota, later), Ok(()));
        }
        assert!(bucket.try_take(quota, later).is_err());
    }

    #[test]
    fn sixty_a_minute_is_one_a_second_sustained() {
        let quota = Quota::per_minute(60).unwrap();
        let mut bucket = TokenBucket::full(quota, 0);
        let mut granted = 0;
        // Over ten minutes, one request every 100 ms: 60 burst + 600 refilled.
        for tick in 0..=6_000_u64 {
            if bucket.try_take(quota, tick * 100).is_ok() {
                granted += 1;
            }
        }
        assert_eq!(granted, 60 + 600);
    }

    #[test]
    fn a_clock_going_backwards_refills_nothing() {
        let mut bucket = TokenBucket::full(five(), 10_000);
        for _ in 0..5 {
            bucket.try_take(five(), 10_000).unwrap();
        }
        assert!(bucket.try_take(five(), 0).is_err());
        assert!(bucket.try_take(five(), 10_000).is_err());
    }

    #[test]
    fn a_refund_returns_one_token_and_never_exceeds_the_burst() {
        let mut bucket = TokenBucket::full(five(), 0);
        for _ in 0..5 {
            bucket.try_take(five(), 0).unwrap();
        }
        bucket.refund(five());
        assert_eq!(bucket.try_take(five(), 0), Ok(()));
        assert!(bucket.try_take(five(), 0).is_err());
        let mut full = TokenBucket::full(five(), 0);
        full.refund(five());
        for _ in 0..5 {
            full.try_take(five(), 0).unwrap();
        }
        assert!(
            full.try_take(five(), 0).is_err(),
            "a refund on a full bucket adds nothing"
        );
    }

    #[test]
    fn a_zero_quota_cannot_be_built() {
        assert_eq!(Quota::per_minute(0), None);
    }

    #[test]
    fn the_peer_is_the_client_unless_it_is_a_trusted_proxy() {
        let peer: IpAddr = "203.0.113.9".parse().unwrap();
        let proxy: IpAddr = "127.0.0.1".parse().unwrap();
        // Untrusted peer: its header is ignored, so a client cannot pick its own key.
        assert_eq!(client_address(peer, Some("198.51.100.1"), &[proxy]), peer);
        // Trusted peer: the rightmost untrusted hop.
        assert_eq!(
            client_address(proxy, Some("198.51.100.1, 203.0.113.9"), &[proxy]),
            peer
        );
        assert_eq!(
            client_address(proxy, Some("198.51.100.1, 127.0.0.1"), &[proxy]),
            "198.51.100.1".parse::<IpAddr>().unwrap()
        );
        // Missing or unparsable header: the proxy itself.
        assert_eq!(client_address(proxy, None, &[proxy]), proxy);
        assert_eq!(
            client_address(proxy, Some("1.2.3.4, junk"), &[proxy]),
            proxy
        );
        // Only proxies in the header: the peer.
        assert_eq!(client_address(proxy, Some("127.0.0.1"), &[proxy]), proxy);
    }

    #[test]
    fn mapped_addresses_are_ipv4_and_ipv6_is_keyed_by_its_64_bit_prefix() {
        let mapped: IpAddr = "::ffff:198.51.100.7".parse().unwrap();
        let plain: IpAddr = "198.51.100.7".parse().unwrap();
        assert_eq!(AddressKey::of(mapped), AddressKey::of(plain));
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(AddressKey::of(a), AddressKey::of(b));
        assert_ne!(AddressKey::of(a), AddressKey::of(c));
        let proxy: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert_eq!(
            client_address(proxy, Some("198.51.100.7"), &["127.0.0.1".parse().unwrap()]),
            plain
        );
    }
}
