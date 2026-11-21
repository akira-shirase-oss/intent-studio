//! A built site bundle: the files a deploy target serves, as one canonical blob.
//!
//! The canonical framing *is* the addressed object: path, length, bytes, in
//! [`BTreeMap`] order, so the same page tree produces byte-identical bundles on
//! any machine — the determinism D-010's binding needs. The store keeps the blob;
//! the check worker decodes it; nothing else interprets these bytes.

use std::collections::BTreeMap;

use crate::content::ContentHash;

/// One file's bound (1 MiB: a generated page is small; assets ride the asset store).
pub const MAX_FILE_BYTES: usize = 1 << 20;
/// A bundle holds at most this many files.
pub const MAX_FILES: usize = 64;
/// The canonical framing's bound: a path's bytes, lengths and contents.
pub const MAX_BUNDLE_BYTES: usize = 8 << 20;
/// A path's bound.
pub const MAX_PATH_BYTES: usize = 200;

/// The bundle's files by serve path (`index.html`, `about/index.html` — relative,
/// no leading slash, `[a-z0-9._/-]`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bundle {
    files: BTreeMap<String, Vec<u8>>,
}

/// Why bytes are not a bundle. Stable codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BundleError {
    /// More than [`MAX_FILES`] entries.
    #[error("too_many_files")]
    TooManyFiles,
    /// A path outside the charset or over [`MAX_PATH_BYTES`].
    #[error("bad_path")]
    BadPath,
    /// A file over [`MAX_FILE_BYTES`].
    #[error("file_too_large")]
    FileTooLarge,
    /// The framing is truncated, trailing or over [`MAX_BUNDLE_BYTES`].
    #[error("malformed")]
    Malformed,
    /// The blob is bigger than the bound.
    #[error("too_large")]
    TooLarge,
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && !path.starts_with('/')
        && !path.contains("..")
        && !path.contains("//")
        && path
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/'))
}

impl Bundle {
    /// An empty bundle.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file; replaces a repeated path. Refuses the bound or a bad path.
    pub fn put(&mut self, path: &str, bytes: Vec<u8>) -> Result<(), BundleError> {
        if !valid_path(path) {
            return Err(BundleError::BadPath);
        }
        if bytes.len() > MAX_FILE_BYTES {
            return Err(BundleError::FileTooLarge);
        }
        if self.files.len() >= MAX_FILES && !self.files.contains_key(path) {
            return Err(BundleError::TooManyFiles);
        }
        self.files.insert(path.to_owned(), bytes);
        Ok(())
    }

    /// The canonical framing: `u32 path_len | path | u64 len | bytes`, ordered by
    /// path — the bytes the store holds and the hash covers.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (path, bytes) in &self.files {
            let len = u32::try_from(path.len()).unwrap_or(u32::MAX);
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(path.as_bytes());
            let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(bytes);
        }
        out
    }

    /// The bundle's content hash over [`Self::encode`].
    pub fn hash(&self) -> ContentHash {
        ContentHash::of(&self.encode())
    }

    /// The files, by path.
    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    /// Untrusted bytes → a bundle, bounds enforced. The check worker's only read
    /// of a bundle; anything outside the framing is refused, not skipped.
    pub fn decode(blob: &[u8]) -> Result<Self, BundleError> {
        if blob.len() > MAX_BUNDLE_BYTES {
            return Err(BundleError::TooLarge);
        }
        let mut bundle = Self::new();
        let mut rest = blob;
        while !rest.is_empty() {
            let Some((len, tail)) = rest.split_at_checked(4) else {
                return Err(BundleError::Malformed);
            };
            let len = u32::from_be_bytes(len.try_into().unwrap_or_default()) as usize;
            let Some((path, tail)) = tail.split_at_checked(len) else {
                return Err(BundleError::Malformed);
            };
            let path = std::str::from_utf8(path).map_err(|_| BundleError::BadPath)?;
            let Some((len, tail)) = tail.split_at_checked(8) else {
                return Err(BundleError::Malformed);
            };
            let len = u64::from_be_bytes(len.try_into().unwrap_or_default());
            let Ok(len) = usize::try_from(len) else {
                return Err(BundleError::TooLarge);
            };
            let Some((bytes, tail)) = tail.split_at_checked(len) else {
                return Err(BundleError::Malformed);
            };
            bundle.put(path, bytes.to_vec())?;
            rest = tail;
        }
        Ok(bundle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_round_trips_and_paths_order_canonically() {
        let mut bundle = Bundle::new();
        bundle.put("b/index.html", b"b".to_vec()).unwrap();
        bundle.put("index.html", b"a".to_vec()).unwrap();
        bundle.put("a.css", b"c".to_vec()).unwrap();
        let encoded = bundle.encode();
        let decoded = Bundle::decode(&encoded).unwrap();
        assert_eq!(decoded, bundle);
        // Canonical order: the decode preserves what the map ordered.
        let paths: Vec<_> = decoded.files().keys().cloned().collect();
        assert_eq!(paths, ["a.css", "b/index.html", "index.html"]);
    }

    #[test]
    fn malformed_and_bounded_blobs_are_refused() {
        for blob in [
            vec![1, 2],                                           // truncated header
            vec![0, 0, 0, 5, b'a'],                               // truncated path
            vec![0, 0, 0, 1, b'x', 0, 0, 0],                      // truncated length
            vec![0, 0, 0, 1, b'x', 0, 0, 0, 0, 0, 0, 0, 9, b'y'], // truncated bytes
        ] {
            assert_eq!(Bundle::decode(&blob), Err(BundleError::Malformed));
        }
        let mut bundle = Bundle::new();
        for bad in [
            "/abs",
            "..",
            "a/../b",
            "UPPER.html",
            "with space",
            "a//b",
            "",
        ] {
            assert_eq!(bundle.put(bad, b"x".to_vec()), Err(BundleError::BadPath));
        }
        assert_eq!(
            bundle.put("big", vec![0; MAX_FILE_BYTES + 1]),
            Err(BundleError::FileTooLarge)
        );
        assert_eq!(
            Bundle::decode(&vec![0; MAX_BUNDLE_BYTES + 1]),
            Err(BundleError::TooLarge)
        );
    }
}
