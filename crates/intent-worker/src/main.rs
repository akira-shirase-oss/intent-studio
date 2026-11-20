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

/// Syscalls a job never needs, denied on Linux (D-011). The list denies rather than
/// allow-lists: a name missed here still runs, but everything that could read a file, open a
/// socket, start another process or leave a namespace is named. The legacy open/stat/fork
/// family exists only on some architectures, so the list is built, not literal.
#[cfg(target_os = "linux")]
fn denied_syscalls() -> Vec<i64> {
    let mut denied = vec![
        // A socket, and everything done on one. The job's only channels are the pipes it was
        // started with, so no address family may be opened — loopback and the database are as
        // unreachable as the internet.
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_bind,
        libc::SYS_connect,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_sendto,
        libc::SYS_sendmsg,
        libc::SYS_sendmmsg,
        libc::SYS_recvfrom,
        libc::SYS_recvmsg,
        libc::SYS_recvmmsg,
        libc::SYS_shutdown,
        libc::SYS_getsockopt,
        libc::SYS_setsockopt,
        libc::SYS_getpeername,
        libc::SYS_getsockname,
        // Opening or making a file by name. Without these, neither the server's
        // /proc/<pid>/environ nor the store nor anything else on the filesystem can be read,
        // written, renamed or removed — the path-based ops below do not need open() to hurt.
        libc::SYS_openat,
        libc::SYS_openat2,
        libc::SYS_open_by_handle_at,
        libc::SYS_name_to_handle_at,
        libc::SYS_newfstatat,
        libc::SYS_statx,
        libc::SYS_faccessat,
        libc::SYS_faccessat2,
        libc::SYS_readlinkat,
        libc::SYS_statfs,
        libc::SYS_unlinkat,
        libc::SYS_mkdirat,
        libc::SYS_renameat,
        libc::SYS_renameat2,
        libc::SYS_linkat,
        libc::SYS_symlinkat,
        libc::SYS_mknodat,
        libc::SYS_fchmodat,
        libc::SYS_fchownat,
        libc::SYS_utimensat,
        libc::SYS_futimesat,
        libc::SYS_setxattr,
        libc::SYS_lsetxattr,
        libc::SYS_getxattr,
        libc::SYS_lgetxattr,
        libc::SYS_listxattr,
        libc::SYS_llistxattr,
        libc::SYS_removexattr,
        libc::SYS_lremovexattr,
        // Watching the filesystem without opening it (existence and timing by side channel).
        libc::SYS_inotify_init1,
        libc::SYS_inotify_add_watch,
        libc::SYS_inotify_rm_watch,
        libc::SYS_fanotify_init,
        libc::SYS_fanotify_mark,
        // Running another program (the filter would follow it, but there is nothing to run).
        libc::SYS_execve,
        libc::SYS_execveat,
        // New processes and namespaces: a forked child would outlive the driver's kill of the
        // job's own process.
        libc::SYS_clone,
        libc::SYS_clone3,
        libc::SYS_unshare,
        libc::SYS_setns,
        // Signalling other processes (the job's parent is the server).
        libc::SYS_kill,
        libc::SYS_tkill,
        libc::SYS_tgkill,
        libc::SYS_pidfd_send_signal,
        libc::SYS_rt_sigqueueinfo,
        libc::SYS_rt_tgsigqueueinfo,
        // Mounts and roots.
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_fsopen,
        libc::SYS_fsmount,
        libc::SYS_fsconfig,
        libc::SYS_move_mount,
        libc::SYS_open_tree,
        // Reading other processes and kernel objects.
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_kcmp,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_userfaultfd,
        libc::SYS_memfd_create,
        libc::SYS_add_key,
        libc::SYS_keyctl,
        libc::SYS_request_key,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        // io_uring runs work on kernel threads: a documented shape for walking past a syscall
        // filter on old kernels.
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
    ];
    // The pre-openat file syscalls and the old fork pair exist on x86_64 but not on the
    // generic-ABI architectures (aarch64, riscv64); deny them where the kernel knows them.
    #[cfg(target_arch = "x86_64")]
    denied.extend([
        libc::SYS_open,
        libc::SYS_creat,
        libc::SYS_stat,
        libc::SYS_lstat,
        libc::SYS_access,
        libc::SYS_readlink,
        libc::SYS_truncate,
        libc::SYS_unlink,
        libc::SYS_rmdir,
        libc::SYS_mkdir,
        libc::SYS_rename,
        libc::SYS_link,
        libc::SYS_symlink,
        libc::SYS_mknod,
        libc::SYS_chmod,
        libc::SYS_chown,
        libc::SYS_lchown,
        libc::SYS_utime,
        libc::SYS_utimes,
        libc::SYS_fork,
        libc::SYS_vfork,
        libc::SYS_inotify_init,
    ]);
    // libc's SYS_* constants are c_long; the filter's keys are i64.
    denied.into_iter().map(i64::from).collect()
}

/// The syscall filter, installed before a byte of input is read. EPERM answers a denied call
/// rather than a kill, so a legitimate library path that tries one fails visibly instead of
/// crashing mid-job; the filter survives exec (itself denied), so there is no stepping out.
#[cfg(target_os = "linux")]
fn confine_syscalls() -> std::io::Result<()> {
    use seccompiler::{SeccompAction, SeccompFilter, TargetArch};
    let failed = || std::io::Error::other("seccomp confinement failed");
    // An unprivileged process may filter itself only once it can never gain privileges.
    // SAFETY: PR_SET_NO_NEW_PRIVS takes no pointer arguments; the flag survives exec.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let rules: std::collections::BTreeMap<i64, Vec<seccompiler::SeccompRule>> = denied_syscalls()
        .into_iter()
        .map(|call| (call, Vec::new()))
        .collect();
    let arch = TargetArch::try_from(std::env::consts::ARCH).map_err(|_| failed())?;
    let eperm = u32::try_from(libc::EPERM).map_err(|_| failed())?;
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(eperm),
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
    use super::denied_syscalls;

    #[test]
    fn the_filter_denies_every_escape_a_job_has() {
        let denied = denied_syscalls();
        // The review's named surfaces, each present exactly once: the network, the filesystem
        // by name, other programs, other processes, the kernel's own doors and io_uring.
        for call in [
            libc::SYS_socket,
            libc::SYS_connect,
            libc::SYS_accept4,
            libc::SYS_openat,
            libc::SYS_openat2,
            libc::SYS_newfstatat,
            libc::SYS_unlinkat,
            libc::SYS_renameat,
            libc::SYS_statfs,
            libc::SYS_inotify_add_watch,
            libc::SYS_execve,
            libc::SYS_clone,
            libc::SYS_clone3,
            libc::SYS_unshare,
            libc::SYS_setns,
            libc::SYS_kill,
            libc::SYS_mount,
            libc::SYS_chroot,
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_perf_event_open,
            libc::SYS_memfd_create,
            libc::SYS_io_uring_setup,
        ] {
            assert_eq!(
                denied.iter().filter(|&&d| d == call).count(),
                1,
                "syscall {call} is missing or duplicated"
            );
        }
        assert_eq!(
            denied.len(),
            denied
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            "duplicate syscall numbers in the deny list"
        );
    }
}
