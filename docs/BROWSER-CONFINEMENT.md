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
  the mount+user namespaces a jail root is built on a tmpfs: the bundle and the
  system trees the browser links against (`/usr`, `/lib*`, `/bin`, `/sbin`)
  bound read-only, then `pivot_root` and a lazy unmount of the old root, so no
  other host path exists at all. A mount namespace alone is *not* enough — it
  still exposes the whole host tree; the pivot is what removes it.
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

The choice: **`unshare(1)` + `pre_exec` rlimits + process-group kill**. The
driver wraps the browser in `unshare --user --map-current-user --net --mount
--ipc --pid --fork --kill-child`; inside, a shell binds the bundle ro, mounts
tmpfs `/tmp`, drops into it, and `exec`s the browser. No unsafe code in the
spawn path beyond `setrlimit`/`setpgid` in `pre_exec`, which the asset driver
already uses. A seccomp allowlist is rejected honestly: Chromium needs hundreds
of syscalls; the namespaces carry the security, the rlimits carry the bounds.

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
