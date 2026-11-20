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
/// namespaces, xattrs, rlimit/priority changes, fcntl signal ownership, pidfd, io_uring —
/// answers EPERM. x32 needs no separate check: an x32 call carries a number with the x32
/// bit set, and no such number is on this list, so the default refusal takes it.
#[cfg(target_os = "linux")]
fn allowed_syscalls() -> Vec<i64> {
    let allowed: Vec<i64> = [
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
    .into_iter()
    .map(i64::from)
    .collect();
    allowed
}

/// The syscall filter, installed before a byte of input is read. Only the named calls get
/// Allow; everything else — matched or unmatched — answers EPERM rather than a kill, so a
/// legitimate library path that tries one fails visibly instead of crashing mid-job. The
/// filter survives exec (itself refused), so there is no stepping out.
#[cfg(target_os = "linux")]
fn confine_syscalls() -> std::io::Result<()> {
    use seccompiler::{SeccompAction, SeccompFilter, TargetArch};
    let failed = || std::io::Error::other("seccomp confinement failed");
    // An unprivileged process may filter itself only once it can never gain privileges.
    // SAFETY: PR_SET_NO_NEW_PRIVS takes no pointer arguments; the flag survives exec.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let rules: std::collections::BTreeMap<i64, Vec<seccompiler::SeccompRule>> = allowed_syscalls()
        .into_iter()
        .map(|call| (call, Vec::new()))
        .collect();
    let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(|_| failed())?;
    let eperm = u32::try_from(libc::EPERM).map_err(|_| failed())?;
    // Rules hit Allow; any other number — sockets, opens, exec, the x32 bit set — hits Errno.
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Errno(eperm),
        SeccompAction::Allow,
        arch,
    )
    .map_err(|_| failed())?;
    let program = seccompiler::BpfProgram::try_from(filter).map_err(|_| failed())?;
    seccompiler::apply_filter(&program).map_err(|_| failed())
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{allowed_syscalls, confine};

    /// Marker env var: the confined child re-runs exactly this test with it set, and the test
    /// body is then the probe — apply the filter, try the two calls, print the verdicts, exit.
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
                allowed.contains(&i64::from(call)),
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
                !allowed.contains(&i64::from(call)),
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

    /// The probe half of the confinement proof: inside the confined child, a socket and an
    /// open must answer EPERM while the allowed calls keep working. Exits directly so the
    /// harness never needs a refused call.
    #[test]
    fn confined_child_probes() {
        if std::env::var(CHILD_FLAG).is_err() {
            return;
        }
        confine().unwrap_or_else(|e| panic!("confinement refused to install: {e}"));
        let errno_of = |rc: i32| {
            if rc == -1 {
                unsafe { *libc::__errno_location() }
            } else {
                0
            }
        };
        let socket_errno = errno_of(unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) });
        let open_errno = errno_of(unsafe {
            libc::openat(libc::AT_FDCWD, c"/etc/passwd".as_ptr(), libc::O_RDONLY)
        });
        // Allowed calls still work: randomness and a write to the inherited pipe.
        let mut byte = [0u8; 1];
        let random_ok = unsafe { libc::getrandom(byte.as_mut_ptr().cast(), 1, 0) } == 1;
        let line =
            format!("CONFINED socket={socket_errno} open={open_errno} getrandom_ok={random_ok}\n");
        unsafe {
            libc::write(1, line.as_ptr().cast(), line.len());
        }
        let proved = socket_errno == libc::EPERM && open_errno == libc::EPERM && random_ok;
        std::process::exit(if proved { 0 } else { 1 });
    }

    /// The parent half: spawn this same test binary confined and check its report — deleting
    /// `confine()` or `deny_dumpable` must fail this, a list check alone cannot.
    #[test]
    fn the_filter_is_installed_and_refuses_escape() {
        if std::env::var(CHILD_FLAG).is_ok() {
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["confined_child_probes", "--test-threads=1"])
            .env(CHILD_FLAG, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "confined child failed: status {:?}, stdout {stdout}, stderr {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("socket=1 open=1 getrandom_ok=true"),
            "unexpected confined report: {stdout}"
        );
    }
}
