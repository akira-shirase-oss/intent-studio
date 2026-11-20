//! The content-addressed store (D-016): asset originals, worker outputs and reference snapshots
//! as immutable blobs named by their SHA-256, under `<root>/sha256/ab/cd/<64 hex>`.
//!
//! - **Atomic and durable.** A blob is written to `<root>/tmp/` under a fresh name, flushed to
//!   disk, renamed into place and its directory flushed, so a crash leaves either the whole blob
//!   or nothing under its name (a stray temporary file at worst).
//! - **Idempotent.** Storing bytes that are already there changes nothing.
//! - **Verified.** Every read re-hashes the bytes; a blob that no longer matches its name is
//!   [`StoreError::Corrupt`], never served.
//! - **Bounded.** No blob is larger than the store's cap, and every operation runs off the async
//!   workers with a deadline.
//!
//! PostgreSQL holds who a blob belongs to and what may be done with it; the store holds only
//! bytes and never decides access.

use std::{
    fs, io,
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

use intent_domain::content::ContentHash;

/// The largest blob the store accepts by default (32 MiB).
pub const DEFAULT_MAX_BLOB_BYTES: usize = 32 * 1024 * 1024;

/// Longest any one store operation may take before the caller gives up on it.
pub const OPERATION_DEADLINE: Duration = Duration::from_secs(10);

/// Why a store operation failed. Messages carry no bytes and no full hash.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The blob is larger than the cap.
    #[error("blob larger than {max} bytes")]
    TooLarge {
        /// The cap.
        max: usize,
    },
    /// A stored blob no longer hashes to its name.
    #[error("stored blob does not match its hash")]
    Corrupt,
    /// The filesystem refused (full, read-only, permissions, gone).
    #[error("store I/O failed")]
    Io(#[source] io::Error),
    /// The operation did not finish before its deadline.
    #[error("store operation timed out")]
    Timeout,
}

/// The store.
#[derive(Debug, Clone)]
pub struct ContentStore {
    root: PathBuf,
    max_blob: usize,
}

impl ContentStore {
    /// Opens (creating if needed) a store at `root`, sweeps crash orphans from `tmp/`, and
    /// proves the store is writable by writing and removing a probe file. The sweep assumes
    /// nothing else has this root open — a second live store would lose its in-flight writes.
    /// Blocking: call at startup, not from a request.
    pub fn open(root: impl Into<PathBuf>, max_blob: usize) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(root.join("sha256")).map_err(StoreError::Io)?;
        let tmp = root.join("tmp");
        fs::create_dir_all(&tmp).map_err(StoreError::Io)?;
        // tmp/ holds only in-flight writes, and at startup nothing is in flight: whatever
        // is here is a crash's orphan, swept before the store serves (D-061).
        for entry in fs::read_dir(&tmp).map_err(StoreError::Io)? {
            let entry = entry.map_err(StoreError::Io)?;
            let path = entry.path();
            if path.is_dir() {
                fs::remove_dir_all(&path).map_err(StoreError::Io)?;
            } else {
                fs::remove_file(&path).map_err(StoreError::Io)?;
            }
        }
        let probe = tmp.join(format!("probe-{}", uuid::Uuid::new_v4().simple()));
        fs::write(&probe, b"probe").map_err(StoreError::Io)?;
        fs::remove_file(&probe).map_err(StoreError::Io)?;
        Ok(Self { root, max_blob })
    }

    /// The root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a blob lives.
    fn path_of(&self, hash: &ContentHash) -> PathBuf {
        let hex = hash.to_string();
        self.root
            .join("sha256")
            .join(&hex[0..2])
            .join(&hex[2..4])
            .join(hex)
    }

    /// Stores `bytes` and returns their hash.
    pub async fn put(&self, bytes: Vec<u8>) -> Result<ContentHash, StoreError> {
        if bytes.len() > self.max_blob {
            return Err(StoreError::TooLarge { max: self.max_blob });
        }
        let store = self.clone();
        run_blocking(move || store.put_blocking(&bytes)).await
    }

    fn put_blocking(&self, bytes: &[u8]) -> Result<ContentHash, StoreError> {
        let hash = ContentHash::of(bytes);
        let target = self.path_of(&hash);
        // The idempotent-write fast path skips the write only while the stored bytes still
        // verify: a torn or damaged file with the same length is rewritten below, never
        // trusted by its name and length.
        if let Ok(meta) = fs::metadata(&target)
            && meta.len() == bytes.len() as u64
            && fs::read(&target).is_ok_and(|stored| ContentHash::of(&stored) == hash)
        {
            return Ok(hash);
        }
        let dir = target
            .parent()
            .unwrap_or_else(|| unreachable!("invariant: a blob path has a parent"));
        fs::create_dir_all(dir).map_err(StoreError::Io)?;
        let temp = self
            .root
            .join("tmp")
            .join(uuid::Uuid::new_v4().simple().to_string());
        let written = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temp, &target)?;
            fs::File::open(dir)?.sync_all()
        })();
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(StoreError::Io(error));
        }
        Ok(hash)
    }

    /// The blob's bytes, verified against its hash; `None` when it is not stored.
    pub async fn get(&self, hash: ContentHash) -> Result<Option<Vec<u8>>, StoreError> {
        let path = self.path_of(&hash);
        let max = self.max_blob;
        run_blocking(move || {
            let bytes = match fs::metadata(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(StoreError::Io(error)),
                Ok(meta) if meta.len() > max as u64 => return Err(StoreError::Corrupt),
                Ok(_) => fs::read(&path).map_err(StoreError::Io)?,
            };
            if ContentHash::of(&bytes) == hash {
                Ok(Some(bytes))
            } else {
                Err(StoreError::Corrupt)
            }
        })
        .await
    }
}

async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError> {
    match tokio::time::timeout(OPERATION_DEADLINE, tokio::task::spawn_blocking(work)).await {
        Err(_elapsed) => Err(StoreError::Timeout),
        Ok(Err(join)) => Err(StoreError::Io(io::Error::other(join.to_string()))),
        Ok(Ok(result)) => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "intent-store-test-{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[tokio::test]
    async fn a_blob_round_trips_once_and_is_verified_on_read() {
        let root = temp_root();
        let store = ContentStore::open(&root, 1024).unwrap();
        let hash = store.put(b"logo bytes".to_vec()).await.unwrap();
        assert_eq!(hash, ContentHash::of(b"logo bytes"));
        // Idempotent: the same bytes again are the same blob.
        assert_eq!(store.put(b"logo bytes".to_vec()).await.unwrap(), hash);
        assert_eq!(store.get(hash).await.unwrap().unwrap(), b"logo bytes");
        assert!(
            store
                .get(ContentHash::of(b"absent"))
                .await
                .unwrap()
                .is_none()
        );
        // Nothing is left in tmp.
        assert_eq!(fs::read_dir(root.join("tmp")).unwrap().count(), 0);

        // A blob altered on disk is refused, not served.
        fs::write(store.path_of(&hash), b"tampered!!").unwrap();
        assert!(matches!(store.get(hash).await, Err(StoreError::Corrupt)));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn blobs_over_the_cap_are_refused() {
        let root = temp_root();
        let store = ContentStore::open(&root, 8).unwrap();
        assert!(matches!(
            store.put(vec![0; 9]).await,
            Err(StoreError::TooLarge { max: 8 })
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_store_that_cannot_be_written_does_not_open() {
        // A file where the root directory should be.
        let root = temp_root();
        fs::write(&root, b"not a directory").unwrap();
        assert!(ContentStore::open(&root, 8).is_err());
        fs::remove_file(root).unwrap();
    }

    #[tokio::test]
    async fn a_crash_orphan_in_tmp_is_never_served_and_is_swept_on_open() {
        // A crash between the flush and the rename leaves a complete write under tmp/,
        // plus whatever other debris. It is not a blob: its name under sha256/ was never
        // made, so a read of its hash finds nothing.
        let root = temp_root();
        let store = ContentStore::open(&root, 1024).unwrap();
        let hash = ContentHash::of(b"crashed write");
        fs::write(root.join("tmp").join("a1b2c3d4"), b"crashed write").unwrap();
        fs::create_dir_all(root.join("tmp").join("stray-dir")).unwrap();
        assert!(store.get(hash).await.unwrap().is_none());
        // Re-opening after the crash sweeps tmp/ — nothing can be in flight at startup —
        // and the interrupted write can be completed afresh.
        let store = ContentStore::open(&root, 1024).unwrap();
        assert_eq!(fs::read_dir(root.join("tmp")).unwrap().count(), 0);
        assert_eq!(store.put(b"crashed write".to_vec()).await.unwrap(), hash);
        assert_eq!(store.get(hash).await.unwrap().unwrap(), b"crashed write");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_torn_blob_is_corrupt_on_read_and_repaired_by_put() {
        // Disk damage or a torn write that kept the length: the name claims the hash
        // but the bytes are not it. The read refuses it, and re-storing the right
        // bytes repairs the blob — a same-length file must not skip the write.
        let root = temp_root();
        let store = ContentStore::open(&root, 1024).unwrap();
        let bytes = b"a payload to tear";
        let hash = store.put(bytes.to_vec()).await.unwrap();
        let mut torn = bytes.to_vec();
        torn[0] = b'X';
        fs::write(store.path_of(&hash), &torn).unwrap();
        assert!(matches!(store.get(hash).await, Err(StoreError::Corrupt)));
        assert_eq!(store.put(bytes.to_vec()).await.unwrap(), hash);
        assert_eq!(store.get(hash).await.unwrap().unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_shortened_blob_is_corrupt_and_repaired_too() {
        // Truncated debris is also refused on read and healed by re-storing.
        let root = temp_root();
        let store = ContentStore::open(&root, 1024).unwrap();
        let bytes = b"longer than this";
        let hash = store.put(bytes.to_vec()).await.unwrap();
        fs::write(store.path_of(&hash), b"short").unwrap();
        assert!(matches!(store.get(hash).await, Err(StoreError::Corrupt)));
        assert_eq!(store.put(bytes.to_vec()).await.unwrap(), hash);
        assert_eq!(store.get(hash).await.unwrap().unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }
}
