//! Content addresses (D-016): the SHA-256 of a blob's bytes names it in the store.

use std::{fmt, str::FromStr};

use sha2::{Digest as _, Sha256};

/// The SHA-256 of some bytes. `Display` is the full 64-digit hex (for APIs and paths); `Debug`
/// shows only a prefix, so a hash never lands whole in a log line by accident.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// The hash of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Wraps a stored digest.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Wraps a digest read back from storage; `None` unless it is 32 bytes.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        <[u8; 32]>::try_from(bytes).ok().map(Self)
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({}…)", hex::encode(&self.0[..4]))
    }
}

/// Not 64 lowercase hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a content hash")]
pub struct NotAHash;

impl FromStr for ContentHash {
    type Err = NotAHash;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 64
            || !s
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(NotAHash);
        }
        let bytes = hex::decode(s).map_err(|_| NotAHash)?;
        Self::from_slice(&bytes).ok_or(NotAHash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_round_trip_and_debug_is_short() {
        let hash = ContentHash::of(b"abc");
        let text = hash.to_string();
        assert_eq!(
            text,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(text.parse::<ContentHash>().unwrap(), hash);
        assert_eq!(format!("{hash:?}"), "ContentHash(ba7816bf…)");
        assert!(
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
                .parse::<ContentHash>()
                .is_err()
        );
        assert!("../etc".parse::<ContentHash>().is_err());
    }
}
