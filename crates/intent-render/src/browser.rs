//! The browser profile (docs/BROWSER-CONFINEMENT.md): a hostile bundle is
//! rendered under a fresh set of namespaces — no network, a read-only bundle
//! plus a private tmpfs for a filesystem, bounded CPU/memory/processes/output,
//! the whole process group killed on deadline. Linux-only: on any other host
//! the job refuses as `Unsupported` and the tests are CI's.
//!
//! A jail that cannot start must say *why*: the inner shell emits a
//! `JAIL-STAGE <name>` marker before each step the kernel may refuse, and
//! [`run`] reports the last marker received plus the shell's own error lines
//! as [`BrowserRefusal::JailStart`]. "Crashed" alone is a payload's death,
//! never a silent jail refusal.

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
    /// The jail itself could not be built: `stage` is the step the kernel
    /// refused (`unshare(user)`, `idmap`, `unshare(net/mount/ipc/pid)`,
    /// `limits`, `mount`, `pivot_root`) and `detail` the inner shell's last
    /// error lines — bounded, never the payload's own output.
    #[error("jail start failed at {stage}: {detail}")]
    JailStart {
        /// The stage named by the last `JAIL-STAGE` marker.
        stage: String,
        /// The shell's error lines, clipped.
        detail: String,
    },
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

/// The hole accessors — the only way [`run`] and the script builders read a
/// flag. In a shipped build each answer is the constant `false` a missing
/// field cannot lie about. Each method is compiled only where a caller
/// exists: `run` is Linux-only, the script builders Linux and tests.
#[allow(
    clippy::unused_self,
    reason = "a shipped build's flag is the constant false — self exists only for the seam bag"
)]
impl BrowserJob {
    #[cfg(target_os = "linux")]
    fn allow_net(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.allow_net;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }

    #[cfg(target_os = "linux")]
    fn host_mounts(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.host_mounts;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }

    #[cfg(any(target_os = "linux", test))]
    fn unbounded_procs(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.unbounded_procs;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }

    #[cfg(any(target_os = "linux", test))]
    fn unbounded_mem(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.unbounded_mem;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }

    #[cfg(target_os = "linux")]
    fn no_deadline(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.no_deadline;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }

    #[cfg(target_os = "linux")]
    fn unbounded_output(&self) -> bool {
        #[cfg(feature = "mutation-seams")]
        return self.holes.unbounded_output;
        #[cfg(not(feature = "mutation-seams"))]
        return false;
    }
}

/// The stage names the jail's marker protocol may report: a `JAIL-STAGE`
/// line is written to stderr immediately before the step it names, so the
/// last marker received identifies the step the kernel refused. `exec` is
/// last on purpose — a failure after it is the payload's crash, not the
/// jail's refusal.
#[cfg(any(target_os = "linux", test))]
const STAGES: &[&str] = &[
    "unshare(user)",
    "idmap",
    "unshare(net/mount/ipc/pid)",
    "limits",
    "mount",
    "pivot_root",
    "exec",
];

/// The line prefix the inner shells emit before each stage.
#[cfg(any(target_os = "linux", test))]
const MARKER: &str = "JAIL-STAGE ";

/// The map files `--map-root-user` writes inside the outer `unshare` — when
/// its error names one, the refused stage is `idmap`, not `unshare(user)`.
#[cfg(any(target_os = "linux", test))]
const MAP_FILES: [&str; 3] = [
    "/proc/self/uid_map",
    "/proc/self/setgroups",
    "/proc/self/gid_map",
];

/// Bytes of the job's stderr kept: enough for the marker protocol plus the
/// shell's error lines, bounded so a payload's own noise cannot grow it.
#[cfg(target_os = "linux")]
const STDERR_CAP: u64 = 16 << 10;

/// Bytes of stderr detail carried into [`BrowserRefusal::JailStart`].
#[cfg(any(target_os = "linux", test))]
const DETAIL_CAP: usize = 600;

/// The namespace-launcher flags for the outer `unshare`: the user namespace
/// plus `--map-root-user`, which performs the id-map step itself —
/// `uid_map` ("0 euid 1"), then `setgroups` deny, then `gid_map` — in the
/// order kernels since Ubuntu's hardening require. A child cannot write its
/// own `setgroups` while its uid is still unmapped (`proc_setgroups_open`
/// refuses, `EACCES`), and an unprivileged parent cannot write a child's
/// either: `map_write` requires the *file opener's* creds to hold
/// `CAP_SYS_ADMIN` over the new namespace, which only the task inside it (or
/// a setuid helper such as `newuidmap`) has. Letting `unshare` do it keeps
/// the write order the proven path uses; see docs/BROWSER-CONFINEMENT.md.
#[cfg(any(target_os = "linux", test))]
const OUTER_ARGS: [&str; 3] = ["--user", "--map-root-user", "--"];

/// The inner script the user-namespace `unshare` execs first. The id map is
/// already written — `--map-root-user` did it before the exec — so the
/// `idmap` stage *verifies* the mapped state in the order the writes happen
/// (uid root, setgroups denied, gid root): a util-linux variant that maps
/// less, or a map that vanished, names `idmap` before the second `unshare`
/// runs. `$1` is the jail script; the remaining parameters pass through to
/// it (`{0}` is the flag set the caller substituted).
#[cfg(any(target_os = "linux", test))]
fn inner_script(inner_flags: &str) -> String {
    format!(
        "echo '{MARKER}idmap' >&2;\
         [ \"$(id -u)\" = 0 ] || {{ echo 'idmap: inner uid is not 0' >&2; exit 81; }};\
         [ \"$(cat /proc/self/setgroups)\" = deny ] || {{ echo 'idmap: setgroups is not deny' >&2; exit 82; }};\
         [ \"$(id -g)\" = 0 ] || {{ echo 'idmap: inner gid is not 0' >&2; exit 83; }};\
         echo '{MARKER}unshare(net/mount/ipc/pid)' >&2;\
         real=$1; shift;\
         exec unshare {inner_flags} -- sh -c \"$real\" sh \"$@\""
    )
}

/// The jail script the second `unshare` execs: resource limits, the tmpfs
/// and bind mounts, `pivot_root`, then the payload. `$1` is the bundle, `$2`
/// the program, the rest its args. The jail root gets its own tmpfs mount:
/// `pivot_root` refuses (`EINVAL`) a new root that is not a mount point, so
/// a directory on the outer `/tmp` tmpfs could never pivot. The `-x` probe
/// before `exec` keeps a program that does not resolve inside the jail at
/// the `exec` stage (exit 88) instead of letting the shell's 127 blur into
/// a payload crash.
#[cfg(any(target_os = "linux", test))]
fn jail_script(limits: &str, host_mounts: bool) -> String {
    let exec = format!(
        "echo '{MARKER}exec' >&2;\
         prog=\"$2\";shift 2;\
         [ -x \"$prog\" ] || {{ echo \"jail: $prog is not executable\" >&2; exit 88; }};\
         exec \"$prog\" \"$@\""
    );
    if host_mounts {
        format!("{limits}cd \"$1\";{exec}")
    } else {
        format!(
            "{limits}\
             echo '{MARKER}mount' >&2;\
             mount -t tmpfs -o nosuid,nodev tmpfs /tmp || exit 84;\
             NR=/tmp/jail;mkdir -p \"$NR\" || exit 84;\
             mount -t tmpfs -o nosuid,nodev tmpfs \"$NR\" || exit 84;\
             mkdir -p \"$NR/bundle\" \"$NR/tmp\" \"$NR/old\" \"$NR/dev\" || exit 84;\
             for d in usr lib lib64 bin sbin; do \
               if [ -d \"/$d\" ]; then \
                 mkdir -p \"$NR/$d\" || exit 84;\
                 mount --bind -o ro \"/$d\" \"$NR/$d\" || exit 84;\
               fi;\
             done;\
             for f in null zero urandom; do \
               if [ -e \"/dev/$f\" ]; then \
                 : > \"$NR/dev/$f\" || exit 84;\
                 mount --bind -o ro \"/dev/$f\" \"$NR/dev/$f\" || exit 84;\
               fi;\
             done;\
             mount --bind -o ro \"$1\" \"$NR/bundle\" || exit 84;\
             mount -t tmpfs -o nosuid,nodev tmpfs \"$NR/tmp\" || exit 84;\
             echo '{MARKER}pivot_root' >&2;\
             cd \"$NR\" && pivot_root . old || exit 86;\
             umount -l /old || exit 87;\
             cd /;{exec}"
        )
    }
}

/// The limits block at the head of the jail script — every limit is
/// fail-closed (`exit 85`): a bound the shell cannot set is a wall it must
/// not silently leave open.
#[cfg(any(target_os = "linux", test))]
fn limits_script(job: &BrowserJob) -> String {
    format!(
        "echo '{MARKER}limits' >&2;\
         ulimit -t {cpu} || exit 85;{as_}ulimit -u {procs} || exit 85;\
         ulimit -f {files} || exit 85;",
        cpu = job.cpu_secs,
        as_ = if job.unbounded_mem() {
            String::new()
        } else {
            format!("ulimit -v {} || exit 85;", job.mem_bytes / 1024)
        },
        procs = if job.unbounded_procs() {
            "unlimited".to_owned()
        } else {
            job.max_procs.to_string()
        },
        files = (job.max_file_bytes / 512).max(1),
    )
}

/// Maps a dead job's stderr and exit code onto a refusal: the last
/// `JAIL-STAGE` marker names the step the kernel refused and the non-marker
/// lines are its error. No marker at all means the first `unshare` died
/// before the inner shell could speak — stage `unshare(user)` by
/// construction, or `idmap` when its error names one of the map files
/// (the outer `unshare` performs those writes itself). After `exec`, the
/// jail shell speaks only when the exec
/// itself was refused — the `-x` probe exits 88, the kernel's own refusal
/// surfaces as the shell's 127; both name `exec`. Any other death past that
/// marker is the *payload's* — `Crashed`, not a jail refusal. A hostile
/// payload could forge a marker line or exit 88 itself; the worst that buys
/// it is a mislabelled crash, never a weaker wall.
#[cfg(any(target_os = "linux", test))]
fn jail_failure(stderr: &[u8], code: Option<i32>) -> BrowserRefusal {
    let text = String::from_utf8_lossy(stderr);
    let mut stage = "unshare(user)";
    let mut detail = String::new();
    for line in text.lines() {
        match line.strip_prefix(MARKER) {
            Some(name) if STAGES.contains(&name.trim()) => stage = name.trim(),
            Some(_) => {}
            None => {
                detail.push_str(line);
                detail.push('\n');
            }
        }
    }
    // The outer `unshare` does the id-map writes itself (`--map-root-user`):
    // when it dies there, no inner marker exists yet — the default
    // `unshare(user)` would mislabel an idmap refusal. The proc path in its
    // error names the step precisely.
    if stage == "unshare(user)" && MAP_FILES.iter().any(|file| detail.contains(file)) {
        stage = "idmap";
    }
    if stage == "exec" && !matches!(code, Some(88 | 127)) {
        return BrowserRefusal::Crashed;
    }
    let detail = detail.trim();
    let detail = if detail.len() > DETAIL_CAP {
        &detail[detail.floor_char_boundary(detail.len() - DETAIL_CAP)..]
    } else {
        detail
    };
    BrowserRefusal::JailStart {
        stage: stage.to_owned(),
        detail: detail.to_owned(),
    }
}

/// The confined spawn, in two stages so a refusal names its step: an outer
/// `unshare --user --map-root-user` gives a user namespace with the caller's
/// euid/egid mapped to root (unshare itself performs the `uid_map`, setgroups
/// deny, `gid_map` writes — in that order — before the exec), then a second
/// `unshare` adds net (no route, no DNS), mount, ipc and pid —
/// `--fork --kill-child` keeps the tree inside. The jail script then
/// mounts a private tmpfs, binds the bundle and system trees read-only,
/// `pivot_root`s so no host path exists, applies `ulimit` bounds and execs
/// the payload. Parameters reach the shells only as positional arguments,
/// never inside their strings.
#[cfg(target_os = "linux")]
fn confined_command(job: &BrowserJob) -> tokio::process::Command {
    use std::process::Stdio;
    use tokio::process::Command;

    let mut inner_flags = String::new();
    if !job.allow_net() {
        inner_flags.push_str("--net ");
    }
    if !job.host_mounts() {
        inner_flags.push_str("--mount ");
    }
    inner_flags.push_str("--ipc --pid --fork --kill-child");
    let limits = limits_script(job);
    let mut cmd = Command::new("unshare");
    cmd.args(OUTER_ARGS)
        .arg("sh")
        .arg("-c")
        .arg(inner_script(&inner_flags))
        .arg("sh")
        .arg(jail_script(&limits, job.host_mounts()))
        .arg(&job.bundle_dir)
        .arg(&job.program)
        .args(&job.args)
        // Its own process group: the deadline kill reaches the whole tree.
        .process_group(0)
        .env_clear()
        // The inner shells resolve unshare/mount/pivot_root by name — with
        // no PATH at all the shell's compiled-in default may lack the sbin
        // dirs pivot_root lives in. A fixed PATH is deterministic.
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", "/tmp")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// Runs `job` confined and returns its stdout (Linux).
///
/// # Errors
/// Every way out is a `BrowserRefusal` — a confined job never answers from a
/// crash, a timeout, or a bound it exceeded, and a jail that cannot start
/// names the stage that failed.
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
    let stderr = child.stderr.take().ok_or(BrowserRefusal::Spawn)?;
    let body = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut out_read = stdout.take(out_cap);
        let mut err_read = stderr.take(STDERR_CAP);
        let (read_out, read_err) = tokio::join!(
            out_read.read_to_end(&mut out),
            err_read.read_to_end(&mut err),
        );
        if let (Ok(_), Ok(_)) = (read_out, read_err) {
            let over = !job.unbounded_output() && out.len() as u64 > job.max_output_bytes;
            let status = child.wait().await;
            let code = status
                .as_ref()
                .ok()
                .and_then(std::process::ExitStatus::code);
            match status {
                Ok(s) if s.success() && !over => Ok(out),
                _ => {
                    kill_tree(&mut child).await;
                    if over {
                        Err(BrowserRefusal::TooLarge)
                    } else {
                        Err(jail_failure(&err, code))
                    }
                }
            }
        } else {
            kill_tree(&mut child).await;
            Err(BrowserRefusal::Crashed)
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

/// Probes whether this host can build the jail at all: `/bin/true` through
/// the full confinement — the exact check a deploy gate fails closed on, so
/// a check worker never runs an unconfined browser. The refusal names the
/// stage the kernel refused (a missing `AppArmor` `userns` grant shows up as
/// `unshare(user)`).
///
/// # Errors
/// The same [`BrowserRefusal`] set [`run`] yields.
#[cfg(target_os = "linux")]
pub async fn selfcheck() -> Result<(), BrowserRefusal> {
    let job = BrowserJob {
        program: "/bin/true".to_owned(),
        bundle_dir: PathBuf::from("/"),
        ..BrowserJob::default()
    };
    run(&job).await.map(|_| ())
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

/// The non-Linux probe: no jail exists here, so the refusal is the same
/// `Unsupported` every confined run gives.
///
/// # Errors
/// Always `BrowserRefusal::Unsupported`.
#[cfg(not(target_os = "linux"))]
#[expect(
    clippy::unused_async,
    reason = "the signature must match the Linux confinement"
)]
pub async fn selfcheck() -> Result<(), BrowserRefusal> {
    Err(BrowserRefusal::Unsupported)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test code: a failed unwrap is a failed test"
)]
mod tests {
    use super::*;

    /// Simulates a stage's own failure text at an exit code and requires the
    /// refusal to name that stage — the marker protocol's whole job.
    fn refusal(stderr: &str, code: Option<i32>) -> BrowserRefusal {
        jail_failure(stderr.as_bytes(), code)
    }

    #[test]
    fn a_dead_unshare_names_the_user_namespace_stage() {
        // Ubuntu 24.04's AppArmor restriction lands here: `unshare(2)` itself
        // is refused, the inner shell never runs, no marker is ever seen.
        let err = refusal(
            "unshare: unshare failed: Operation not permitted\n",
            Some(1),
        );
        let BrowserRefusal::JailStart { stage, detail } = err else {
            panic!("expected JailStart, got {err}");
        };
        assert_eq!(stage, "unshare(user)");
        assert!(detail.contains("Operation not permitted"), "{detail}");
    }

    #[test]
    fn a_refused_map_check_names_the_idmap_stage() {
        // The inner shell verifies the map `--map-root-user` wrote and fails
        // closed under its own marker.
        let err = refusal(
            "JAIL-STAGE idmap\n\
             idmap: setgroups is not deny\n",
            Some(82),
        );
        let BrowserRefusal::JailStart { stage, detail } = err else {
            panic!("expected JailStart, got {err}");
        };
        assert_eq!(stage, "idmap");
        assert!(detail.contains("setgroups is not deny"), "{detail}");
    }

    #[test]
    fn a_map_refusal_inside_unshare_names_the_idmap_stage() {
        // `--map-root-user` writes the maps inside the outer `unshare`
        // itself: a refusal there kills it before the inner shell can emit
        // a marker. The proc path in its error still names the step.
        for message in [
            "unshare: write failed /proc/self/uid_map: Permission denied\n",
            "unshare: cannot open /proc/self/setgroups: Permission denied\n",
            "unshare: write failed /proc/self/gid_map: Permission denied\n",
        ] {
            let err = refusal(message, Some(1));
            let BrowserRefusal::JailStart { stage, detail } = err else {
                panic!("expected JailStart, got {err}");
            };
            assert_eq!(stage, "idmap", "{message}");
            assert!(detail.contains("/proc/self/"), "{detail}");
        }
    }

    #[test]
    fn the_inner_shell_verifies_never_writes_the_map() {
        // On Ubuntu's hardened kernels `proc_setgroups_open` refuses a task
        // whose uid is not yet mapped: a self-write of `setgroups` is what
        // CI run 44899063312 died on. `--map-root-user` is the map writer —
        // the inner shell may only read the resulting state.
        let script = inner_script("--ipc --pid");
        assert!(
            !script.contains("> /proc/self/"),
            "the inner shell must never write its own map files: {script}"
        );
        // The checks run in the order `--map-root-user` performed them:
        // uid to root, setgroups denied, gid to root.
        let positions = ["JAIL-STAGE idmap", "id -u", "setgroups", "id -g"].map(|needle| {
            script
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} missing from {script}"))
        });
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "verify uid, setgroups, gid — in write order: {positions:?} in {script}"
        );
    }

    #[test]
    fn the_outer_unshare_maps_the_caller_to_root() {
        // The map writer of record: `unshare` performs uid_map, setgroups
        // deny, gid_map in the order kernels since Ubuntu's hardening
        // require. Neither alternative can replace it — a child self-write
        // opens `setgroups` with an unmapped uid (`EACCES`), and an
        // unprivileged parent's write lacks `CAP_SYS_ADMIN` over the new
        // namespace (only `newuidmap`-style setuid helpers have it).
        assert_eq!(
            OUTER_ARGS.as_slice(),
            ["--user", "--map-root-user", "--"].as_slice()
        );
    }

    #[test]
    fn a_refused_inner_unshare_names_its_stage() {
        let err = refusal(
            "JAIL-STAGE idmap\n\
             JAIL-STAGE unshare(net/mount/ipc/pid)\n\
             unshare: unshare failed: Operation not permitted\n",
            Some(1),
        );
        let BrowserRefusal::JailStart { stage, .. } = err else {
            panic!("expected JailStart, got {err}");
        };
        assert_eq!(stage, "unshare(net/mount/ipc/pid)");
    }

    #[test]
    fn a_refused_mount_names_the_mount_stage() {
        let err = refusal(
            "JAIL-STAGE idmap\n\
             JAIL-STAGE unshare(net/mount/ipc/pid)\n\
             JAIL-STAGE limits\n\
             JAIL-STAGE mount\n\
             mount: /tmp: permission denied\n",
            Some(84),
        );
        let BrowserRefusal::JailStart { stage, detail } = err else {
            panic!("expected JailStart, got {err}");
        };
        assert_eq!(stage, "mount");
        assert!(detail.contains("permission denied"), "{detail}");
    }

    #[test]
    fn a_refused_pivot_names_the_pivot_stage() {
        let err = refusal(
            "JAIL-STAGE mount\n\
             JAIL-STAGE pivot_root\n\
             pivot_root: failed to change root from . to old: Operation not permitted\n",
            Some(86),
        );
        let BrowserRefusal::JailStart { stage, detail } = err else {
            panic!("expected JailStart, got {err}");
        };
        assert_eq!(stage, "pivot_root");
        assert!(detail.contains("pivot_root"), "{detail}");
    }

    #[test]
    fn a_refused_exec_names_the_exec_stage() {
        // The `-x` probe's exit 88: the program never ran.
        let err = refusal(
            "JAIL-STAGE pivot_root\n\
             JAIL-STAGE exec\n\
             jail: /bundle/probe is not executable\n",
            Some(88),
        );
        let BrowserRefusal::JailStart { stage, detail } = err else {
            panic!("expected JailStart, got {err}");
        };
        assert_eq!(stage, "exec");
        assert!(detail.contains("not executable"), "{detail}");
        // A kernel-side exec refusal (e.g. a missing interpreter) exits 127
        // from the shell — the jail, not the payload, failed.
        let err = refusal(
            "JAIL-STAGE exec\n\
             sh: exec: /bundle/probe: not found\n",
            Some(127),
        );
        let BrowserRefusal::JailStart { stage, .. } = err else {
            panic!("expected JailStart, got {err}");
        };
        assert_eq!(stage, "exec");
    }

    #[test]
    fn a_death_after_exec_is_the_payloads_not_the_jails() {
        for code in [Some(1), Some(42), None] {
            let err = refusal(
                "JAIL-STAGE mount\n\
                 JAIL-STAGE pivot_root\n\
                 JAIL-STAGE exec\n\
                 Segmentation fault\n",
                code,
            );
            assert!(
                matches!(err, BrowserRefusal::Crashed),
                "a marker past exec with a payload's death ({code:?}) is the payload's, not the jail's: {err}"
            );
        }
    }

    #[test]
    fn forged_markers_can_only_mislabel_never_open_a_wall() {
        // The payload's stderr shares the channel: it could print a marker of
        // its own. A mislabelled refusal is cosmetic — the job still dies,
        // the wall still held.
        let err = refusal("JAIL-STAGE mount\nJAIL-STAGE bogus\n", Some(1));
        let BrowserRefusal::JailStart { stage, .. } = err else {
            panic!("expected JailStart, got {err}");
        };
        assert_eq!(stage, "mount", "unknown marker names are ignored");
    }

    #[test]
    fn detail_is_clipped_at_the_bound() {
        let noise = "x".repeat(DETAIL_CAP * 3);
        let stderr = format!("JAIL-STAGE mount\n{noise}");
        let BrowserRefusal::JailStart { detail, .. } = refusal(&stderr, Some(1)) else {
            panic!("expected JailStart");
        };
        assert!(
            detail.len() <= DETAIL_CAP,
            "{} > {DETAIL_CAP}",
            detail.len()
        );
    }

    #[test]
    fn every_marker_in_the_scripts_names_a_known_stage() {
        // The scripts and the stage table are the protocol: a marker neither
        // side knows would mislabel the refusal.
        let limits = limits_script(&BrowserJob {
            program: String::new(),
            ..BrowserJob::default()
        });
        for script in [
            inner_script("--ipc --pid"),
            jail_script(&limits, false),
            jail_script(&limits, true),
        ] {
            for line in script.split(';').chain(script.split('\n')) {
                if let Some(rest) = line.split('\'').nth(1).filter(|_| line.contains(MARKER)) {
                    let name = rest.trim_start_matches(MARKER);
                    assert!(STAGES.contains(&name), "unknown stage {name} in {script}");
                }
            }
        }
    }
}
