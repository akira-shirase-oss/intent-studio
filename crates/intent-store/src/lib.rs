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
    collections::HashSet,
    fs,
    future::Future,
    io,
    io::Write as _,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, MutexGuard},
    time::Duration,
};

use intent_domain::content::ContentHash;

/// The largest blob the store accepts by default (32 MiB).
pub const DEFAULT_MAX_BLOB_BYTES: usize = 32 * 1024 * 1024;

/// Longest any one store operation may take before the caller gives up on it.
pub const OPERATION_DEADLINE: Duration = Duration::from_secs(10);

/// Roots this process holds open. A POSIX record lock is per-process — it cannot
/// conflict with a second lock by the same process — so the same-process half of
/// "a root is single-store" lives here. `open` claims the key before opening
/// `lock`; that ordering is also what keeps the POSIX close footgun away: any
/// close by this process of a descriptor on that inode drops all of this
/// process's locks on it, so a live store's descriptor must be the only one.
///
/// The key is the root's `(st_dev, st_ino)`, never a path spelling: a second
/// mount point or bind mount of the same directory canonicalizes to a different
/// path but is still the same directory, and must claim the same store (J).
static OPEN_ROOTS: LazyLock<Mutex<HashSet<(u64, u64)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn open_roots() -> MutexGuard<'static, HashSet<(u64, u64)>> {
    OPEN_ROOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
    /// A live store already holds this root — a root is single-store.
    #[error("store root is held by another live store")]
    Locked,
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
    inner: Arc<Inner>,
}

/// What every clone of a store shares — and what actually owns the root.
#[derive(Debug)]
struct Inner {
    root: PathBuf,
    max_blob: usize,
    /// The descriptor the record lock lives against. Held (never read) so long
    /// as any clone lives; `Option` so `Drop` can order close-before-unregister.
    /// No other descriptor may ever be opened or duplicated on this file: a
    /// close on any of this process's descriptors on the inode drops all of
    /// this process's record locks on it.
    lock: Option<fs::File>,
    /// The root's `(st_dev, st_ino)`, the `OPEN_ROOTS` key.
    key: (u64, u64),
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Close first, then unregister. Reversed would let a racing `open`
        // claim the root, take the same-process record lock, then lose it
        // silently when this descriptor's close releases the inode's locks.
        drop(self.lock.take());
        open_roots().remove(&self.key);
    }
}

/// Removes the registry claim on drop unless `disarm` hands it to a store —
/// the error paths in `open` must not leave a root claimed.
struct Claim((u64, u64));

impl Claim {
    fn disarm(self) -> (u64, u64) {
        let key = self.0;
        std::mem::forget(self);
        key
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        open_roots().remove(&self.0);
    }
}

/// Where a blob lives.
fn blob_path(root: &Path, hash: &ContentHash) -> PathBuf {
    let hex = hash.to_string();
    root.join("sha256")
        .join(&hex[0..2])
        .join(&hex[2..4])
        .join(hex)
}

impl ContentStore {
    /// Opens (creating if needed) a store at `root`, takes its exclusive lock, sweeps crash
    /// orphans from `tmp/`, and proves the store is writable by writing and removing a probe
    /// file. A root is single-store, across processes and within this one:
    ///
    /// - Cross-process, `<root>/lock` carries a POSIX `fcntl` record lock. Record locks are
    ///   per-process — unlike `flock`/OFD locks they are *not* inherited by a forked child —
    ///   and they die with the process, so a crash or kill always frees the root.
    /// - Same-process, `OPEN_ROOTS` refuses a second `open` before it touches the file —
    ///   necessary because a same-process record lock cannot conflict, and closing any other
    ///   descriptor on that inode would silently drop the live lock.
    ///
    /// The lock is held for the store's life and released deterministically when the last
    /// clone drops. Blocking: call at startup, not from a request.
    pub fn open(root: impl Into<PathBuf>, max_blob: usize) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(StoreError::Io)?;
        // `metadata` follows links, so a symlinked spelling claims the same directory; a
        // bind-mounted spelling canonicalizes elsewhere but still lands here (J).
        let meta = fs::metadata(&root).map_err(StoreError::Io)?;
        let key = (meta.dev(), meta.ino());
        if !open_roots().insert(key) {
            return Err(StoreError::Locked);
        }
        let claimed = Claim(key);
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("lock"))
            .map_err(StoreError::Io)?;
        match rustix::fs::fcntl_lock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(StoreError::Locked);
            }
            Err(error) => return Err(StoreError::Io(error.into())),
        }
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
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                max_blob,
                lock: Some(lock),
                key: claimed.disarm(),
            }),
        })
    }

    /// The root directory.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Where a blob lives (the tests poke the file directly).
    #[cfg(test)]
    fn path_of(&self, hash: &ContentHash) -> PathBuf {
        blob_path(&self.inner.root, hash)
    }

    /// Stores `bytes` and returns their hash.
    pub async fn put(&self, bytes: Vec<u8>) -> Result<ContentHash, StoreError> {
        if bytes.len() > self.inner.max_blob {
            return Err(StoreError::TooLarge {
                max: self.inner.max_blob,
            });
        }
        self.put_task(bytes).await
    }

    /// The blocking half of `put` as a `'static` future: it carries the root and
    /// cap only, never the lock, so an operation that outlives the deadline (a
    /// wedged blocking task cannot be cancelled) holds nothing the root needs.
    fn put_task(
        &self,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<ContentHash, StoreError>> + Send + 'static {
        let root = self.inner.root.clone();
        async move { run_blocking(move || put_blocking(&root, &bytes)).await }
    }

    /// The blob's bytes, verified against its hash; `None` when it is not stored.
    pub async fn get(&self, hash: ContentHash) -> Result<Option<Vec<u8>>, StoreError> {
        self.get_task(hash).await
    }

    fn get_task(
        &self,
        hash: ContentHash,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, StoreError>> + Send + 'static {
        let path = blob_path(&self.inner.root, &hash);
        let max = self.inner.max_blob;
        async move {
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
}

fn put_blocking(root: &Path, bytes: &[u8]) -> Result<ContentHash, StoreError> {
    #[cfg(test)]
    {
        let gate = PUT_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take_if(|(gated, _, _)| gated.as_path() == root)
            .map(|(_, entered, release)| (entered, release));
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            let _ = release.recv_timeout(Duration::from_secs(30));
        }
    }
    let hash = ContentHash::of(bytes);
    let target = blob_path(root, &hash);
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
    let temp = root
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

async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError> {
    match tokio::time::timeout(OPERATION_DEADLINE, tokio::task::spawn_blocking(work)).await {
        Err(_elapsed) => Err(StoreError::Timeout),
        Ok(Err(join)) => Err(StoreError::Io(io::Error::other(join.to_string()))),
        Ok(Ok(result)) => result,
    }
}

/// The wedged-operation test's rendezvous: the root it applies to (so a parallel
/// test's `put` cannot steal it), the "inside the operation" sender, and the
/// "finish now" receiver — channels, not barriers, so the test's fuse can be a
/// `recv_timeout` rather than a wall-clock sleep.
#[cfg(test)]
type PutGate = (
    PathBuf,
    std::sync::mpsc::SyncSender<()>,
    std::sync::mpsc::Receiver<()>,
);

#[cfg(test)]
static PUT_GATE: LazyLock<Mutex<Option<PutGate>>> = LazyLock::new(|| Mutex::new(None));

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
    fn a_second_open_is_refused_and_sweeps_nothing() {
        // A root is single-store: while one store lives, a second open of the
        // same root must refuse before it touches anything — the debris below
        // could be the live store's in-flight write.
        let root = temp_root();
        let _store = ContentStore::open(&root, 1024).unwrap();
        fs::write(root.join("tmp").join("in-flight"), b"half written").unwrap();
        assert!(matches!(
            ContentStore::open(&root, 1024),
            Err(StoreError::Locked)
        ));
        assert_eq!(
            fs::read(root.join("tmp").join("in-flight")).unwrap(),
            b"half written"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_dropped_store_lets_the_root_open_again() {
        // Closing the store releases the root; a later open sweeps as usual.
        let root = temp_root();
        let store = ContentStore::open(&root, 1024).unwrap();
        drop(store);
        fs::write(root.join("tmp").join("orphan"), b"debris").unwrap();
        let _store = ContentStore::open(&root, 1024).unwrap();
        assert_eq!(fs::read_dir(root.join("tmp")).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_second_spelling_of_the_same_directory_is_refused() {
        // The registry keys on (st_dev, st_ino), so a second path to the same directory —
        // a symlink here, a bind mount in deployment — claims the same store rather than
        // opening a second one whose drop would release the first's lock (J).
        let root = temp_root();
        let link = temp_root();
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let store = ContentStore::open(&root, 1024).unwrap();
        assert!(
            matches!(ContentStore::open(&link, 1024), Err(StoreError::Locked)),
            "a second spelling of an open root got its own store"
        );
        drop(store);
        // The claim died with the store: the other spelling opens cleanly now.
        let _store = ContentStore::open(&link, 1024).unwrap();
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(&link).unwrap();
    }

    #[test]
    fn a_duplicated_lock_descriptor_cannot_hold_the_root() {
        // fork() and try_clone() both hand out a second descriptor on the same open
        // file description — under flock() that duplicate kept the lock alive past
        // drop(store). The record lock is per-process: closing our one descriptor on
        // `lock` releases it no matter how many duplicates exist.
        let root = temp_root();
        let store = ContentStore::open(&root, 1024).unwrap();
        let duplicate = store.inner.lock.as_ref().unwrap().try_clone().unwrap();
        drop(store);
        let store2 = ContentStore::open(&root, 1024).unwrap();
        drop(store2);
        // And the duplicate's own close cannot pretend to hold or steal anything.
        drop(duplicate);
        let _store = ContentStore::open(&root, 1024).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wedged_blocking_operation_cannot_hold_the_root() {
        // An operation that outlives its deadline — a wedged blocking task cannot
        // be cancelled — carries the root and cap only, so dropping the last
        // store releases the root even while the operation still runs.
        let root = temp_root();
        let store = ContentStore::open(&root, 1024).unwrap();
        // The gate keys on the store's own root spelling (`inner.root`, uncanonicalized).
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        *PUT_GATE.lock().unwrap() = Some((root.clone(), entered_tx, release_rx));
        let task = store.put_task(b"wedged".to_vec());
        drop(store);
        let op = tokio::spawn(task);
        // The operation is provably in flight once it signals from inside the gate.
        assert!(
            entered_rx.recv_timeout(Duration::from_secs(10)).is_ok(),
            "operation never reached the gate"
        );
        let _store = ContentStore::open(&root, 1024).unwrap();
        let _ = release_tx.send(());
        op.await.unwrap().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_crashed_holder_releases_the_root() {
        // Child mode: open the root (the lock is the process's), leave crash
        // debris in tmp/, mark itself ready, then wait to be killed.
        if let Ok(root) = std::env::var("INTENT_STORE_CRASH_CHILD") {
            let _store = ContentStore::open(&root, 1024).unwrap();
            fs::write(Path::new(&root).join("tmp").join("orphan"), b"debris").unwrap();
            fs::write(Path::new(&root).join("ready"), b"").unwrap();
            for _ in 0..600 {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            return;
        }

        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .env("INTENT_STORE_CRASH_CHILD", &root)
            .args([
                "--exact",
                "tests::a_crashed_holder_releases_the_root",
                "--nocapture",
            ])
            .spawn()
            .unwrap();
        for _ in 0..3000 {
            if root.join("ready").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // A live process holds the root: a second open is refused.
        let second_open = ContentStore::open(&root, 1024);
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(root.join("ready").exists(), "child never took the lock");
        assert!(matches!(second_open, Err(StoreError::Locked)));
        // The crash released the lock: open succeeds and sweeps the debris.
        let _store = ContentStore::open(&root, 1024).unwrap();
        assert_eq!(fs::read_dir(root.join("tmp")).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn arbitrary_bytes_round_trip_and_stay_idempotent() {
        // Property: any byte string up to the cap stores, reads back whole,
        // and storing it twice is the same single blob.
        let root = temp_root();
        let store = ContentStore::open(&root, 2048).unwrap();
        let mut rng = fastrand::Rng::with_seed(0xCAFE_2026);
        for _ in 0..64 {
            let bytes: Vec<u8> = (0..rng.usize(0..=2048)).map(|_| rng.u8(..)).collect();
            let hash = store.put(bytes.clone()).await.unwrap();
            assert_eq!(store.get(hash).await.unwrap().unwrap(), bytes);
            assert_eq!(store.put(bytes.clone()).await.unwrap(), hash);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn different_bytes_live_under_different_names() {
        // Property: distinct byte strings never share a name or hide each other.
        let root = temp_root();
        let store = ContentStore::open(&root, 2048).unwrap();
        let mut rng = fastrand::Rng::with_seed(0xB10B);
        for _ in 0..32 {
            let a: Vec<u8> = (0..rng.usize(1..=512)).map(|_| rng.u8(..)).collect();
            let b: Vec<u8> = (0..rng.usize(1..=512)).map(|_| rng.u8(..)).collect();
            let (ha, hb) = (
                store.put(a.clone()).await.unwrap(),
                store.put(b.clone()).await.unwrap(),
            );
            assert_eq!(store.get(ha).await.unwrap().unwrap(), a);
            assert_eq!(store.get(hb).await.unwrap().unwrap(), b);
            if a != b {
                assert_ne!(ha, hb);
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn an_impostor_under_a_key_is_rewritten_not_kept() {
        // Property: whatever bytes squat under a hash's name — same length with
        // a byte flipped, truncated, or padded — the read refuses them and
        // re-storing the true bytes repairs the blob.
        let root = temp_root();
        let store = ContentStore::open(&root, 2048).unwrap();
        let mut rng = fastrand::Rng::with_seed(0xFEED);
        for _ in 0..32 {
            let bytes: Vec<u8> = (0..rng.usize(1..=512)).map(|_| rng.u8(..)).collect();
            let hash = store.put(bytes.clone()).await.unwrap();
            let target = store.path_of(&hash);
            match rng.u8(0..3) {
                0 => {
                    let mut impostor = bytes.clone();
                    let i = rng.usize(..impostor.len());
                    impostor[i] ^= 0xFF;
                    fs::write(&target, &impostor).unwrap();
                }
                1 => {
                    fs::write(&target, &bytes[..rng.usize(..bytes.len())]).unwrap();
                }
                _ => {
                    let mut impostor = bytes.clone();
                    impostor.extend_from_slice(&[0; 8]);
                    fs::write(&target, &impostor).unwrap();
                }
            }
            assert!(matches!(store.get(hash).await, Err(StoreError::Corrupt)));
            assert_eq!(store.put(bytes.clone()).await.unwrap(), hash);
            assert_eq!(store.get(hash).await.unwrap().unwrap(), bytes);
        }
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
        // The "restart" ends the old holder first (a root is single-store): re-opening
        // sweeps tmp/ — nothing can be in flight at startup — and the interrupted write
        // can be completed afresh.
        drop(store);
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
