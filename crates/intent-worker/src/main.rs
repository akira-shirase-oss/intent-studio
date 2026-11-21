//! `intent-worker <svg|png|jpeg|font>`: one job, then exit (D-011, D-062).
//!
//! Started by `intent-render`'s driver with an empty environment. Before reading a byte it
//! limits itself: CPU time, no file writes, no core dumps, few descriptors and, on Linux, its
//! address space plus a seccomp filter that denies it sockets, file opens, exec and new
//! processes — a job reads stdin and writes stdout and needs nothing else. A limit that cannot
//! be set stops the job (it never runs unconfined). It writes one framed answer and exits 0; a
//! crash is the driver's to report.

use std::{
    io::{Read as _, Write as _},
    process::ExitCode,
};

use intent_render::{AssetKind, Header, MAX_INPUT_BYTES, WorkerRefusal};
use rlimit::Resource;

/// CPU seconds a job may use (the driver's wall-time deadline is the other bound).
const CPU_SECONDS: u64 = 10;
/// Address space on Linux (1 GiB), where the kernel enforces it.
#[cfg(target_os = "linux")]
const ADDRESS_SPACE_BYTES: u64 = 1024 * 1024 * 1024;

/// The syscalls a job needs, allowed on Linux (D-011). An allow-list rather than a deny
/// list: every number not named here — sockets, file opens by name, exec, new processes,
/// namespaces, xattrs, `rlimit`/priority changes, `fcntl` signal ownership, `pidfd`,
/// `io_uring` — kills the job. `x32` needs no separate check: an `x32` call carries a
/// number with the `x32` bit set, and no such number is on this list, so the default
/// kill takes it.
/// `libc::SYS_*` constants are `c_long`, which is `i64` only on 64-bit targets; this list
/// is built for them and a 32-bit target gets no filter at all (see below).
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
fn allowed_syscalls() -> Vec<i64> {
    vec![
        // The job's whole I/O: read the job on stdin, write the framed answer on stdout.
        libc::SYS_read,
        libc::SYS_readv,
        libc::SYS_write,
        libc::SYS_writev,
        libc::SYS_close,
        // Heap and stack management, by the allocator and the runtime only.
        libc::SYS_brk,
        libc::SYS_mmap,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        // Parking and waking between threads the runtime started before confinement.
        libc::SYS_futex,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        // Randomness, time, a sleep, a yield, and the two ways out.
        libc::SYS_getrandom,
        libc::SYS_clock_gettime,
        libc::SYS_nanosleep,
        libc::SYS_clock_nanosleep,
        libc::SYS_sched_yield,
        libc::SYS_getpid,
        libc::SYS_restart_syscall,
        libc::SYS_exit,
        libc::SYS_exit_group,
    ]
}

/// The syscall filter, installed before a byte of input is read. Only the named calls get
/// Allow; everything else — sockets, opens, exec, the x32 bit set — kills the job, so a
/// probe surfaces as a crash the driver reports, never as a quiet errno the job could try
/// to work around. The filter survives exec (itself refused), so there is no stepping out.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
fn confine_syscalls() -> std::io::Result<()> {
    use seccompiler::{SeccompAction, SeccompFilter, TargetArch};
    let failed = || std::io::Error::other("seccomp confinement failed");
    // `apply_filter` sets PR_SET_NO_NEW_PRIVS before loading the program — an
    // unprivileged process may filter itself only once it can never gain privileges.
    let mut rules: std::collections::BTreeMap<i64, Vec<seccompiler::SeccompRule>> =
        allowed_syscalls()
            .into_iter()
            .map(|call| (call, Vec::new()))
            .collect();
    // mmap is allowed only without PROT_EXEC: the job decodes, it never runs generated
    // code. seccompiler arg 2 of mmap is the `prot` flags word.
    let no_exec = seccompiler::SeccompCondition::new(
        2,
        seccompiler::SeccompCmpArgLen::Dword,
        seccompiler::SeccompCmpOp::MaskedEq(u64::try_from(libc::PROT_EXEC).unwrap_or(u64::MAX)),
        0,
    )
    .map_err(|_| failed())?;
    rules.insert(
        libc::SYS_mmap,
        vec![seccompiler::SeccompRule::new(vec![no_exec]).map_err(|_| failed())?],
    );
    let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(|_| failed())?;
    // Rules hit Allow; any other number — sockets, opens, exec, the x32 bit set — kills.
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::KillProcess,
        SeccompAction::Allow,
        arch,
    )
    .map_err(|_| failed())?;
    let program = seccompiler::BpfProgram::try_from(filter).map_err(|_| failed())?;
    seccompiler::apply_filter(&program).map_err(|_| failed())
}

/// A 32-bit Linux cannot express this filter (syscall numbers there are not `i64`), so
/// rather than run unconfined the job refuses to start.
#[cfg(all(target_os = "linux", not(target_pointer_width = "64")))]
fn confine_syscalls() -> std::io::Result<()> {
    Err(std::io::Error::other(
        "seccomp confinement is only built for 64-bit kernels",
    ))
}

fn confine() -> std::io::Result<()> {
    rlimit::setrlimit(Resource::CPU, CPU_SECONDS, CPU_SECONDS)?;
    // No file may be written, of any size.
    rlimit::setrlimit(Resource::FSIZE, 0, 0)?;
    rlimit::setrlimit(Resource::CORE, 0, 0)?;
    rlimit::setrlimit(Resource::NOFILE, 16, 16)?;
    #[cfg(target_os = "linux")]
    {
        rlimit::setrlimit(Resource::AS, ADDRESS_SPACE_BYTES, ADDRESS_SPACE_BYTES)?;
        confine_syscalls()?;
    }
    Ok(())
}

fn answer(result: Result<intent_render::Processed, WorkerRefusal>) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    match result {
        Ok(processed) => {
            let header = Header {
                ok: true,
                media_type: Some(processed.media_type),
                width: processed.width,
                height: processed.height,
                removed_parts: Some(processed.removed_parts),
                dropped_parts: Some(processed.dropped_parts),
                length: Some(processed.output.len()),
                refusal: None,
            };
            serde_json::to_writer(&mut stdout, &header)?;
            stdout.write_all(b"\n")?;
            stdout.write_all(&processed.output)?;
        }
        Err(refusal) => {
            let header = Header {
                ok: false,
                media_type: None,
                width: None,
                height: None,
                removed_parts: None,
                dropped_parts: None,
                length: None,
                refusal: Some(refusal),
            };
            serde_json::to_writer(&mut stdout, &header)?;
            stdout.write_all(b"\n")?;
        }
    }
    stdout.flush()
}

fn main() -> ExitCode {
    if confine().is_err() {
        return ExitCode::from(3);
    }
    let Some(kind) = std::env::args()
        .nth(1)
        .as_deref()
        .and_then(AssetKind::parse)
    else {
        return ExitCode::from(2);
    };
    let mut input = Vec::new();
    let limit = (MAX_INPUT_BYTES + 1) as u64;
    if std::io::stdin()
        .lock()
        .take(limit)
        .read_to_end(&mut input)
        .is_err()
    {
        return ExitCode::from(4);
    }
    let result = if input.len() > MAX_INPUT_BYTES {
        Err(WorkerRefusal::TooLarge)
    } else {
        intent_worker::process(kind, &input)
    };
    match answer(result) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(5),
    }
}

#[cfg(all(test, target_os = "linux", target_pointer_width = "64"))]
mod tests {
    use super::{allowed_syscalls, confine};
    use std::io::Write as _;
    use std::os::unix::process::ExitStatusExt as _;

    /// Marker env var: a child re-runs `confined_child_probes` with it set to a probe name
    /// — the test body applies the filter, performs that one call, then reports.
    const CHILD_FLAG: &str = "INTENT_WORKER_CONFINED_CHILD";

    #[test]
    fn the_allowed_list_is_exactly_what_a_job_needs() {
        let allowed = allowed_syscalls();
        for call in [
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_close,
            libc::SYS_brk,
            libc::SYS_mmap,
            libc::SYS_munmap,
            libc::SYS_mremap,
            libc::SYS_madvise,
            libc::SYS_futex,
            libc::SYS_rt_sigaction,
            libc::SYS_rt_sigprocmask,
            libc::SYS_rt_sigreturn,
            libc::SYS_sigaltstack,
            libc::SYS_getrandom,
            libc::SYS_clock_gettime,
            libc::SYS_exit_group,
        ] {
            assert!(
                allowed.contains(&call),
                "syscall {call} a job needs is not allowed"
            );
        }
        // The review's named escapes must not be on the list.
        for call in [
            libc::SYS_socket,
            libc::SYS_openat,
            libc::SYS_prlimit64,
            libc::SYS_setpriority,
            libc::SYS_fchmodat2,
            libc::SYS_pidfd_getfd,
            libc::SYS_io_uring_setup,
            libc::SYS_clone3,
        ] {
            assert!(
                !allowed.contains(&call),
                "escape syscall {call} must not be allowed"
            );
        }
        assert_eq!(
            allowed.len(),
            allowed
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            "duplicate syscall numbers in the allow list"
        );
    }

    /// The probe half of the confinement proof: a confined child performs exactly one call.
    /// A refused call kills it inside the attempt (the filter's action is `KillProcess`); the
    /// allowed probe reports its calls still work.
    #[test]
    fn confined_child_probes() {
        let Ok(probe) = std::env::var(CHILD_FLAG) else {
            return;
        };
        confine().unwrap_or_else(|e| panic!("confinement refused to install: {e}"));
        match probe.as_str() {
            "socket" => {
                // `bind` opens a socket before anything else; if the call returns at
                // all the filter did not kill it — report that as a clean exit.
                let _ = std::net::TcpListener::bind("127.0.0.1:0");
                std::process::exit(0);
            }
            "openat" => {
                let _ = std::fs::File::open("/etc/passwd");
                std::process::exit(0);
            }
            "allowed" => {
                // The calls a job keeps: randomness and a write to the inherited pipe.
                let _random = std::hash::RandomState::new();
                std::io::stdout()
                    .write_all(b"CONFINED\n")
                    .unwrap_or_else(|e| panic!("pipe write failed: {e}"));
                std::process::exit(0);
            }
            other => panic!("unknown probe {other}"),
        }
    }

    /// The parent half: a refused call must kill the child, the allowed set must let it
    /// finish — deleting `confine()` fails this, a list check alone cannot.
    #[test]
    fn the_filter_kills_escape_and_allows_the_job() {
        if std::env::var(CHILD_FLAG).is_ok() {
            return;
        }
        for probe in ["socket", "openat"] {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["confined_child_probes", "--test-threads=1"])
                .env(CHILD_FLAG, probe)
                .status()
                .unwrap();
            // Seccomp's kill action answers a denied syscall with SIGSYS — no other
            // signal is the filter's signature (F6): a SIGSEGV crash or an external
            // SIGKILL must not satisfy this proof.
            assert_eq!(
                status.signal(),
                Some(libc::SIGSYS),
                "{probe} probe was not killed by the seccomp filter: {status:?}"
            );
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["confined_child_probes", "--test-threads=1"])
            .env(CHILD_FLAG, "allowed")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "allowed probe failed: {:?} {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        // The child is this test binary re-run under a filter, so libtest's own lines
        // share its stdout — under --test-threads=1 the banner `test ...confined...`
        // is on the SAME line the child's write lands on, so line equality never sees
        // a bare CONFINED. The marker with its own newline inside the stream is the
        // honest proof: the child wrote it, wherever libtest put its text.
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("CONFINED\n"),
            "the confined child never printed CONFINED: {stdout}"
        );
    }
}
