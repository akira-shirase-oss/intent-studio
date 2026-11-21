//! The browser profile (docs/BROWSER-CONFINEMENT.md): a hostile bundle is
//! rendered under a fresh set of namespaces — no network, a read-only bundle
//! plus a private tmpfs for a filesystem, bounded CPU/memory/processes/output,
//! the whole process group killed on deadline. Linux-only: on any other host
//! the job refuses as `Unsupported` and the tests are CI's.

use std::path::PathBuf;
use std::time::Duration;

/// How one confined browser job may end.
#[derive(Debug, thiserror::Error)]
pub enum BrowserRefusal {
    /// The platform cannot give the job namespaces (every non-Linux host).
    #[error("browser confinement is Linux-only")]
    Unsupported,
    /// The namespace wrapper could not be spawned.
    #[error("confinement wrapper unavailable")]
    Spawn,
    /// The job ran past its deadline; the whole process group was killed.
    #[error("browser job timed out")]
    Deadline,
    /// The job's output exceeded its bound.
    #[error("browser job output exceeded its bound")]
    TooLarge,
    /// The job (or the confinement around it) died without a usable answer.
    #[error("browser job crashed")]
    Crashed,
}

/// One confined render/check job. The hole flags exist for the mutation proof:
/// each names the attack class its guard defeats, each is `false` in any real
/// build, and only `INTENT_MUTATION` under `mutation-seams` can set one.
#[derive(Debug, Clone)]
pub struct BrowserJob {
    /// The program to run inside the confinement (a browser, or the probe).
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// The bundle directory bound read-only inside the job's mount namespace.
    pub bundle_dir: PathBuf,
    /// Wall-clock limit before the process group is killed.
    pub deadline: Duration,
    /// `RLIMIT_CPU` seconds for the whole tree.
    pub cpu_secs: u64,
    /// `RLIMIT_AS` bytes for the whole tree.
    pub mem_bytes: u64,
    /// `RLIMIT_NPROC` for the whole tree.
    pub max_procs: u64,
    /// `RLIMIT_FSIZE` bytes — screenshots and reports live on the job's tmpfs.
    pub max_file_bytes: u64,
    /// Bytes of stdout the driver will read before cutting the job off.
    pub max_output_bytes: u64,
    /// The armed holes — a field a shipped build cannot even name: the flags
    /// exist only under `mutation-seams`, so no code path outside the proofs
    /// can carry one (N7 of the browser-confinement review).
    #[cfg(feature = "mutation-seams")]
    pub holes: BrowserHoles,
}

/// The six mutation holes — one per wall [`run`] must hold (the drill table).
/// Exists only under `mutation-seams`; every accessor below answers `false`
/// for a flag a shipped build does not have.
#[cfg(feature = "mutation-seams")]
#[derive(Debug, Clone, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the seam bag maps one flag per attack class, by design"
)]
pub struct BrowserHoles {
    /// `browser-allow-net`: no network namespace.
    pub allow_net: bool,
    /// `browser-host-mounts`: no private mount namespace — the host filesystem.
    pub host_mounts: bool,
    /// `browser-unbounded-procs`: no process-count limit.
    pub unbounded_procs: bool,
    /// `browser-unbounded-mem`: no address-space limit.
    pub unbounded_mem: bool,
    /// `browser-no-deadline`: the driver never kills the job.
    pub no_deadline: bool,
    /// `browser-unbounded-output`: stdout is read without a bound.
    pub unbounded_output: bool,
}

impl Default for BrowserJob {
    fn default() -> Self {
        Self {
            program: String::new(),
            args: Vec::new(),
            bundle_dir: PathBuf::new(),
            deadline: Duration::from_secs(30),
            cpu_secs: 30,
            mem_bytes: 4 << 30,
            max_procs: 128,
            max_file_bytes: 32 << 20,
            max_output_bytes: 4 << 20,
            #[cfg(feature = "mutation-seams")]
            holes: BrowserHoles::default(),
        }
    }
}

/// The hole accessors — the only way [`run`] and [`confined_command`] read a
/// flag. In a shipped build each answer is the constant `false` a missing
/// field cannot lie about. Linux-only like their callers.
#[cfg(target_os = "linux")]
#[allow(
    clippy::unused_self,
    reason = "a shipped build's flag is the constant false — self exists only for the seam bag"
)]
impl BrowserJob {
    fn allow_net(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.allow_net;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }

    fn host_mounts(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.host_mounts;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }

    fn unbounded_procs(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.unbounded_procs;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }

    fn unbounded_mem(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.unbounded_mem;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }

    fn no_deadline(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.no_deadline;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }

    fn unbounded_output(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.unbounded_output;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }
}

/// The confined spawn: `unshare(1)` builds the namespaces around the exec'd
/// browser — user (so the mounts are allowed without privilege), net (no route,
/// no DNS), mount, ipc and pid; `--fork --kill-child` keeps the tree inside.
/// Inside, a jail root on a private tmpfs — the bundle and the system trees the
/// browser links against, bound read-only — then `pivot_root` so no host path
/// exists. `ulimit` applies the resource bounds to the tree (the workspace
/// forbids unsafe code; a shell builtin needs none). Job parameters reach the
/// shell only as positional parameters, never inside its string.
#[cfg(target_os = "linux")]
fn confined_command(job: &BrowserJob) -> tokio::process::Command {
    use std::process::Stdio;
    use tokio::process::Command;

    let mut flags: Vec<&str> = vec!["--user", "--map-current-user"];
    if !job.allow_net() {
        flags.push("--net");
    }
    if !job.host_mounts() {
        flags.push("--mount");
    }
    flags.extend(["--ipc", "--pid", "--fork", "--kill-child"]);
    let inner = if job.host_mounts() {
        "cd \"$1\"; prog=\"$2\"; shift 2; exec \"$prog\" \"$@\"".to_owned()
    } else {
        concat!(
            "mount -t tmpfs -o nosuid,nodev tmpfs /tmp;",
            "NR=/tmp/jail; mkdir -p \"$NR/bundle\" \"$NR/tmp\" \"$NR/old\";",
            "for d in usr lib lib64 bin sbin; do",
            "  [ -d \"/$d\" ] && mkdir -p \"$NR/$d\" && mount --bind -o ro \"/$d\" \"$NR/$d\";",
            "done;",
            "mount --bind -o ro \"$1\" \"$NR/bundle\";",
            "mount -t tmpfs -o nosuid,nodev tmpfs \"$NR/tmp\";",
            "cd \"$NR\" && pivot_root . old || exit 86;",
            "umount -l /old || exit 87;",
            "cd /; prog=\"$2\"; shift 2; exec \"$prog\" \"$@\""
        )
        .to_owned()
    };
    let limits = format!(
        "ulimit -t {cpu};{as_} ulimit -u {procs};ulimit -f {files};",
        cpu = job.cpu_secs,
        as_ = if job.unbounded_mem() {
            String::new()
        } else {
            format!("ulimit -v {};", job.mem_bytes / 1024)
        },
        procs = if job.unbounded_procs() {
            "unlimited".to_owned()
        } else {
            job.max_procs.to_string()
        },
        files = (job.max_file_bytes / 512).max(1),
    );
    let mut cmd = Command::new("unshare");
    cmd.args(&flags)
        .arg("sh")
        .arg("-c")
        .arg(format!("{limits}{inner}"))
        .arg("sh")
        .arg(&job.bundle_dir)
        .arg(&job.program)
        .args(&job.args)
        // Its own process group: the deadline kill reaches the whole tree.
        .process_group(0)
        .env_clear()
        .env("HOME", "/tmp")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd
}

/// Runs `job` confined and returns its stdout (Linux).
///
/// # Errors
/// Every way out is a `BrowserRefusal` — a confined job never answers from a
/// crash, a timeout, or a bound it exceeded.
#[cfg(target_os = "linux")]
pub async fn run(job: &BrowserJob) -> Result<Vec<u8>, BrowserRefusal> {
    use tokio::io::AsyncReadExt;

    let (out_cap, deadline_on) = (
        if job.unbounded_output() {
            u64::MAX
        } else {
            job.max_output_bytes + 1
        },
        !job.no_deadline(),
    );
    let mut child = confined_command(job)
        .spawn()
        .map_err(|_| BrowserRefusal::Spawn)?;
    let stdout = child.stdout.take().ok_or(BrowserRefusal::Spawn)?;
    let body = async {
        let mut out = Vec::new();
        let read = stdout.take(out_cap).read_to_end(&mut out).await;
        match read {
            Ok(_) if !job.unbounded_output() && out.len() as u64 > job.max_output_bytes => {
                kill_tree(&mut child).await;
                Err(BrowserRefusal::TooLarge)
            }
            Ok(_) => match child.wait().await {
                Ok(status) if status.success() => Ok(out),
                _ => {
                    kill_tree(&mut child).await;
                    Err(BrowserRefusal::Crashed)
                }
            },
            Err(_) => {
                kill_tree(&mut child).await;
                Err(BrowserRefusal::Crashed)
            }
        }
    };
    if !deadline_on {
        return body.await;
    }
    if let Ok(result) = tokio::time::timeout(job.deadline, body).await {
        result
    } else {
        kill_tree(&mut child).await;
        Err(BrowserRefusal::Deadline)
    }
}

/// Ends the job's whole process group (the wrapper is its group leader — the
/// pid-namespace teardown then reaps anything left), then the direct child: a
/// browser's renderer processes cannot outlive the job.
#[cfg(target_os = "linux")]
async fn kill_tree(child: &mut tokio::process::Child) {
    if let Some(id) = child.id() {
        let _ = tokio::process::Command::new("kill")
            .args(["-s", "KILL", "--", &format!("-{}", id.cast_signed())])
            .status()
            .await;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Runs `job` confined — every non-Linux host refuses as `Unsupported`: the
/// namespaces the profile needs do not exist there (CI proves it on Linux).
///
/// # Errors
/// Always `BrowserRefusal::Unsupported`.
#[cfg(not(target_os = "linux"))]
#[expect(
    clippy::unused_async,
    reason = "the signature must match the Linux confinement"
)]
pub async fn run(job: &BrowserJob) -> Result<Vec<u8>, BrowserRefusal> {
    let _ = job;
    Err(BrowserRefusal::Unsupported)
}
