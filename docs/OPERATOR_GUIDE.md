# Operator guide

How to install, run, upgrade, back up and restore Intent Studio on one Linux host, and what to
do when something is wrong. Every command here is exercised by `scripts/test-deploy.sh` on each
`scripts/check.sh` run (configuration check, unit and proxy validation, a production-profile
start with a lead and a graceful stop, and the backup and restore) except where a step says it
is not.

**What this release is for.** I0 and I1: the lead inbox, and recording a client's brief, facts,
assets and references. Generating and publishing sites (I2, I3) are not built yet, and sites
that send leads are created by promotion (I3) — until then the only sites are the demo ones
`intent-server seed` makes, which are for testing. See `README.md` for what is proven.

## What runs where

| Piece | What it is | Where it lives |
|---|---|---|
| `intent-server` | The API, the lead intake, the notifier, the workspace's files | `/opt/intent-studio/bin/` |
| `intent-worker` | Checks every uploaded file, one process per file (D-062) | `/opt/intent-studio/bin/` |
| The workspace | A static app the server serves at `/workspace` | `/opt/intent-studio/workspace/` |
| PostgreSQL 16 or newer | The only authority (tested with 18.6) | a local or managed server |
| The content store | Asset and snapshot bytes by SHA-256 (D-061) | `/var/lib/intent-studio/store` |
| Caddy 2 | HTTPS and the client address | in front, on 443 |
| A notification webhook | Receives one JSON notification per lead | yours, on loopback or a private network |

The server listens on `127.0.0.1:59400` and only Caddy talks to it. A 2 GB host is enough for
the pilot.

## Install

On a build machine with Rust (rustup reads `rust-toolchain.toml`: 1.99.1), Node 22.17 or
newer, and `jq`:

```sh
git clone https://github.com/akira-shirase-oss/intent-studio.git && cd intent-studio
scripts/package.sh            # prints dist/intent-studio-<commit>.tar.gz
```

The package holds `bin/intent-server`, `bin/intent-worker`, `workspace/`, `deploy/` (the unit,
the env example, the Caddyfile) and this guide. Build on the same CPU architecture and libc
family as the host (or build on the host). Copy it to the host, then:

```sh
tar -xzf intent-studio-*.tar.gz && cd intent-studio
sudo useradd --system --home-dir /var/lib/intent-studio --shell /usr/sbin/nologin intent
sudo install -d -m 0755 /opt/intent-studio/bin /opt/intent-studio/workspace
sudo install -m 0755 bin/intent-server bin/intent-worker /opt/intent-studio/bin/
sudo cp -R workspace/. /opt/intent-studio/workspace/
sudo install -d -m 0750 -g intent /etc/intent-studio
sudo install -m 0640 -g intent deploy/intent-studio.env.example /etc/intent-studio/intent-studio.env
sudo install -m 0644 deploy/intent-studio.service /etc/systemd/system/intent-studio.service
sudo systemctl daemon-reload
```

The unit (`deploy/intent-studio.service`) runs as `intent`, makes `/var/lib/intent-studio`
its only writable place, and sandboxes the server and the worker it starts (no new
privileges, read-only system, no home directories, a system-call filter that still allows the
worker to lower its own limits).

**Not yet provided, and owed by D-011/D-062:** the worker's own container (read-only root, no
network). Today the worker runs as a separate process under the unit's sandbox, with an empty
environment and its own CPU, file-size and memory limits; it shares the service's network.

## First run

1. **Database.** As the PostgreSQL superuser:

   ```sql
   CREATE ROLE intent LOGIN PASSWORD 'a long random password';
   CREATE DATABASE intent OWNER intent;
   ```

   Keep PostgreSQL's durability defaults (`fsync` and `synchronous_commit` on): a lead is
   acknowledged only after its commit, so the commit must mean it.

2. **The operator password.** Pick one and hash it (the password is read from standard input,
   never from the command line):

   ```sh
   read -rs password && printf '%s\n' "$password" | /opt/intent-studio/bin/intent-server hash-password; unset password
   ```

3. **The configuration.** Edit `/etc/intent-studio/intent-studio.env`: replace every
   `__PLACEHOLDER__` — the database password in `INTENT_DATABASE_URL`, the hash (in single
   quotes) in `INTENT_OPERATOR_PASSWORD_HASH`, and your webhook in `INTENT_NOTIFY_WEBHOOK_URL`.
   Then check it; every problem is listed at once, and nothing starts:

   ```sh
   sudo -u intent sh -c 'set -a; . /etc/intent-studio/intent-studio.env; exec /opt/intent-studio/bin/intent-server check-config'
   ```

4. **Migrate and start.**

   ```sh
   sudo -u intent sh -c 'set -a; . /etc/intent-studio/intent-studio.env; exec /opt/intent-studio/bin/intent-server migrate'
   sudo systemctl enable --now intent-studio
   journalctl -u intent-studio -n 20          # expect a "listening" line
   curl -s http://127.0.0.1:59400/readyz      # {"ready":true,...}
   ```

5. **HTTPS.** Copy `deploy/Caddyfile` into Caddy's configuration, replace
   `studio.example.com` with your host name, then `caddy validate --config /etc/caddy/Caddyfile`
   and `systemctl reload caddy`.

6. **Sign in.** Open `https://<your host>/workspace/` and sign in with the password from step 2.
   Add the first client on the **Facts** screen (*Add a client*), record its facts and their
   approvals, upload its logo on **Assets**, and write its brief on **Briefs**.

## Upgrade

1. Take a backup (below) and copy it off the host.
2. Build the new release as in *Install* and copy the two binaries and the workspace into place
   (`/opt/intent-studio/bin/`, `/opt/intent-studio/workspace/`).
3. Check the configuration and apply migrations with the new binary (`check-config`, then
   `migrate`, as in *First run*). Migrations only add; none is undone automatically.
4. `sudo systemctl restart intent-studio`. The old process drains first: readiness turns false,
   in-flight requests get up to 10 seconds, the notifier finishes its attempts in flight.
5. Check `journalctl -u intent-studio -n 50` and `/readyz`.

To go back, restore the backup from step 1 and the previous binaries; a newer schema is not
guaranteed to run older binaries.

## Back up and restore

The database and the store are backed up together (D-016, D-061). **Database first, then the
store:** the store only ever grows, and every blob a row names was written before that row
committed, so a store copied after the dump holds every blob the dump refers to.

```sh
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
pg_dump --format=custom --file="intent-$stamp.dump" "postgres://intent:...@127.0.0.1:5432/intent"
sudo tar -C /var/lib/intent-studio -czf "store-$stamp.tgz" store
```

Copy both files off the host. To restore (into an empty database and store):

```sh
sudo systemctl stop intent-studio
dropdb intent && createdb --owner intent intent          # as the PostgreSQL superuser
pg_restore --no-owner --role=intent --dbname="postgres://intent:...@127.0.0.1:5432/intent" "intent-$stamp.dump"
sudo rm -rf /var/lib/intent-studio/store && sudo tar -C /var/lib/intent-studio -xzf "store-$stamp.tgz"
sudo chown -R intent:intent /var/lib/intent-studio/store
sudo systemctl start intent-studio
```

`scripts/test-deploy.sh` runs this dump and restore against a fresh database on each check and
compares what came back.

## Troubleshooting

| What you see | Why | What to do |
|---|---|---|
| The service exits at start with `invalid configuration (N problem(s))` | One or more settings are missing or out of range | Run `check-config` (First run, step 3); fix each line it prints |
| `INTENT_STORE_DIR ...: store I/O failed` at start | The store directory cannot be written | `chown -R intent:intent /var/lib/intent-studio`; check the disk is not full |
| `INTENT_WORKER_BIN: no worker at ...` at start | `intent-worker` is missing | Install it beside `intent-server`, or set `INTENT_WORKER_BIN` |
| `/readyz` answers 503 with `"database":"down"` | PostgreSQL cannot be reached | Leads are refused with `503` and `Retry-After` (nothing is acknowledged that is not stored); bring PostgreSQL back and readiness turns green within a second |
| Sign-in says it is not configured | `INTENT_OPERATOR_PASSWORD_HASH` is not set | Hash a password (First run, step 2) and restart |
| Sign-in says to wait a minute | Five attempts a minute per address, 20 failed ones a minute overall (D-041, D-052) | Wait; an address you signed in from in the last 30 days is not held up by others' attempts |
| A lead shows *Not notified (8 attempts…)* | The webhook was unreachable for about a day | The lead is safe in the inbox; contact the client by hand. Manual resend arrives with I3 |
| An upload says *the file checker could not be started* | The worker binary cannot run | Check its path, that it is executable and built for this machine; `journalctl -u intent-studio` |
| An upload says the file *made the checker fail* | The worker crashed on that file | Only that upload failed; try a re-exported copy of the file. If every upload fails this way, check the unit's `SystemCallFilter` still allows `setrlimit` |
| A reference *points to a private, local or reserved network* | The egress guard refused it (D-012) | Intended: such addresses are never fetched. Save the page and upload what you need |
| The asset store is *not available on this server* | `INTENT_STORE_DIR` is unset | Set it and restart |

**Logs.** `journalctl -u intent-studio` shows JSON lines with request ids, routes, status codes
and refusal codes. By design it never shows lead content, brief or fact text, approval tokens
or secrets (D-029); read the inbox or the record itself in the workspace.

**Metrics.** `curl -s -H "Authorization: Bearer $INTENT_METRICS_TOKEN" http://127.0.0.1:59400/metrics`
(or, with no token set, `curl -s http://127.0.0.1:59400/metrics` from the host itself).

**Development stack.** `deploy/compose.yaml` and `scripts/stack.sh` are for development and
tests only: public credentials, ports 59400–59499 on loopback.
