# Browser confinement (I2)

The six D-022 browser checks must render a *hostile* bundle — a generated site or
a hostile reference page is exactly the input that may exploit the browser. The
existing `intent-worker` profile cannot carry this: its seccomp allowlist denies
the sockets and exec a browser needs, and its `FSIZE=0` denies screenshot writes.
The browser therefore runs under a **separate profile**, in `intent-render`, and
the asset worker's profile is unchanged.

## What must hold

- **No network at all** — not "no external hosts": a fresh network namespace has
  no route, no DNS, no socket to anything but (up-configured) loopback.
- **No filesystem beyond a read-only bundle dir and a private tmpfs** — inside
  the mount+user namespaces a jail root is built on a tmpfs (mounted at the
  jail root itself: `pivot_root` requires a mount point): the bundle, the
  system trees the browser links against (`/usr`, `/lib*`, `/bin`, `/sbin`)
  and three device nodes (`/dev/null`, `/dev/zero`, `/dev/urandom`) bound
  read-only, then `pivot_root` and a lazy unmount of the old root, so no other
  host path exists at all. A mount namespace alone is *not* enough — it still
  exposes the whole host tree; the pivot is what removes it.
- **No access to the database, Valkey, or the server's secrets** — follows from
  no-network + `env_clear` (no `DATABASE_URL`, no credentials, no proxy vars).
- **Bounded CPU, memory, wall time, and output** — `RLIMIT_CPU`/`RLIMIT_AS`/
  `RLIMIT_NPROC`/`RLIMIT_FSIZE` on the outer wrapper (inherited by the whole
  tree), a driver deadline, and the same `take(limit)` output bound the asset
  driver already uses.
- **No persistence between jobs** — fresh namespaces per job, private tmpfs, and
  a per-job profile dir inside it; nothing survives the process tree's death.
- **The whole process tree dies on timeout** — the wrapper is its own process
  group; the driver kills the group (`killpg`), not just the leader.

## The options on the target Linux host

| | unshare namespaces (chosen) | bubblewrap | per-job container |
|---|---|---|---|
| Dependency | `unshare(1)` from util-linux — present on every systemd host | bwrap binary, often setuid | a container runtime + image |
| Fit | parent applies namespaces *around* the exec'd browser — matches the existing driver shape (spawn, env_clear, rlimits, deadline) | same idea, extra dep | heavyweight: daemon socket is itself a privileged surface |
| Seccomp | the job gets a **broad** filter (browser syscalls are too many to enumerate); the namespaces + rlimits are the wall | same | same |

The choice: **`unshare(1)` in two stages + shell `ulimit`s + process-group
kill**. The driver wraps the browser in `unshare --user --map-root-user`;
`unshare` itself performs the id-map writes — `uid_map` ("0 euid 1"), then
`setgroups` deny, then `gid_map` — in the order kernels since Ubuntu's
hardening require, then execs an inner shell that *verifies* the mapped
state (the `idmap` stage: inner uid 0, `setgroups` denied, inner gid 0)
before running a second `unshare --net --mount --ipc --pid --fork
--kill-child`. Inside that, a jail shell applies the `ulimit` bounds,
mounts a private tmpfs, binds the bundle and system trees ro, `pivot_root`s,
and `exec`s the browser. No unsafe code in the spawn path. A seccomp
allowlist is rejected honestly: Chromium needs hundreds of syscalls; the
namespaces carry the security, the rlimits carry the bounds.

## The id map: written by `unshare`, verified by the shell

CI run 44899063312 proved a naive self-write cannot do this step: the inner
shell wrote `deny` to `/proc/self/setgroups` first and the kernel refused it
(`EACCES` — `proc_setgroups_open` rejects a task whose uid is not yet
mapped). Three writes are involved and their order is a kernel rule, not a
convention:

1. `uid_map` — the namespace's own task may self-write the single extent
   `"0 <euid> 1"` unprivileged (`new_idmap_permitted`'s self-map exception).
   Once written the task's uid *is* 0 inside the namespace.
2. `setgroups` deny — required before `gid_map` (since Linux 3.19, an
   unprivileged `gid_map` write is refused while `USERNS_SETGROUPS_ALLOWED`
   is set). Opening the file also demands `CAP_SETGID` over the namespace —
   which the task only satisfies once its uid is mapped, i.e. after step 1.
3. `gid_map` — the same self-map exception, now permitted because
   `setgroups` is already `deny`.

`unshare --user --map-root-user` performs exactly this sequence in C before
the `exec`, which is why it was chosen over the two alternatives:

- **Child self-writes, any order** — what CI 44899063312 ran: `setgroups`
  first dies at `open` on hardened kernels; and even the corrected order
  only re-implements by shell what `unshare` already performs in C — the
  same writes `unshare -Ur` does successfully on these kernels.
- **Parent writes `/proc/<child>/{setgroups,uid_map,gid_map}` over a sync
  pipe** — impossible unprivileged: `map_write` additionally requires the
  *file opener's* credentials to hold `CAP_SYS_ADMIN` over the new
  namespace, and `proc_setgroups_open` requires `CAP_SETGID` over it or its
  parent. A regular user's parent process has neither; only setuid helpers
  (`newuidmap`/`newgidmap`) carry that privilege, which is exactly why they
  exist. A synchronization pipe cannot grant a capability.

If the map writes inside `unshare` fail, its own error names the file
(`/proc/self/uid_map`, `setgroups`, `gid_map`), and the classifier reports
the stage as `idmap`, not `unshare(user)`. The inner shell then re-checks
the state — `[ "$(id -u)" = 0 ]`, `[ "$(cat /proc/self/setgroups)" = deny
]`, `[ "$(id -g)" = 0 ]`, in write order — so a util-linux variant that
maps less than `--map-root-user` promises still names `idmap` and never
reaches the second `unshare`.

## A failed jail start names its stage

A `Crashed` refusal with no cause is what CI run 44893760525 produced —
loud, but useless. The inner shells emit a `JAIL-STAGE <name>` line to
stderr immediately before each step the kernel may refuse —
`unshare(user)`, `idmap`, `unshare(net/mount/ipc/pid)`, `limits`, `mount`,
`pivot_root`, `exec` — and the driver's stderr pipe (bounded at 16 KiB)
keeps the last marker plus the shell's own error lines. A dead job whose
last marker precedes `exec` returns `BrowserRefusal::JailStart` with the
stage and a ≤600-byte detail; a refusal inside `unshare`'s own map writes
(no inner marker yet) still reports `idmap` when its error names a map
file. After `exec`, only the shell's own exec
refusals (exit 88/127) still name the stage — any other death is the
payload's `Crashed`. A hostile payload can forge a marker line and buy a
mislabelled crash; it cannot buy an open wall.

Ubuntu 24.04's `kernel.apparmor_restrict_unprivileged_userns=1` is the
failure this was built for: it lands at `unshare(user)`. CI relaxes the
sysctl on its ephemeral runner only; production carries
`deploy/apparmor.d/intent-jail`, a scoped `userns` grant on the two
binaries, and the boot self-check (a confined `/bin/true` through the same
path) refuses to serve before readiness when the jail cannot start —
fail closed, never an unconfined browser.

## Talking to the browser without a network

Chrome DevTools Protocol over **`--remote-debugging-pipe`**: the parent creates a
socketpair, dup2s its ends onto the child's fds 3 (read) and 4 (write) in
`pre_exec`, and speaks CDP frames over it — no TCP, no /dev/socket, nothing the
confined process could reuse as a network. WebKit/WebKitGTK's inspector protocol
is pipe-capable the same way; the interface takes the channel, not a port.

## macOS dev honesty

Namespaces are Linux-only. On macOS the module compiles to a `Unsupported` path
and every confinement test is `#[cfg(target_os = "linux")]` — **CI on Linux must
prove**: the probe cannot open a TCP connection, cannot read a host path, dies
on a fork bomb, dies on a memory bomb, dies on deadline, and is cut at the
output bound. The browser *checks* stay `Pending` here — and stay honest in
records as `browser backend not installed` until a real renderer exists.

## Attack classes (each into the mutation proof)

| Class | Seam | Guard |
|---|---|---|
| network egress | `browser-allow-net` | `CLONE_NEWNET` |
| host file read | `browser-host-mounts` | ro bundle bind + private mount ns only |
| fork bomb | `browser-unbounded-procs` | `RLIMIT_NPROC` |
| memory bomb | `browser-unbounded-mem` | `RLIMIT_AS` |
| hung page | `browser-no-deadline` | driver deadline + killpg |
| oversized output | `browser-unbounded-output` | `take(limit)` read bound |
