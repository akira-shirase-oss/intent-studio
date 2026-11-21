#!/usr/bin/env bash
# The production path, tested (run by check.sh): the release package, installed into a scratch
# prefix, started from deploy/intent-studio.env.example in the production profile — the same
# commands docs/OPERATOR_GUIDE.md gives the operator.
#
#   1. scripts/package.sh, unpacked into a scratch prefix
#   2. the env example with its placeholders filled; `check-config` passes it and names a
#      missing variable when one is removed
#   3. `migrate`, then `serve` in the production profile behind no proxy: /readyz, the
#      workspace under its CSP, operator sign-in with a Secure cookie, a lead stored and
#      notified through the webhook, an asset uploaded through the real worker
#   4. backup (pg_dump, then the store) and restore into a fresh database and store, and a
#      second server on the restore serves the same lead and the same asset bytes
#   5. SIGTERM: a graceful exit within the unit's TimeoutStopSec
#   6. the systemd unit through `systemd-analyze verify` (where systemd exists: CI) and the
#      Caddyfile through `caddy validate` (in a pinned container, where Docker is available)
set -euo pipefail
cd "$(dirname "$0")/.."

port=59404
restore_port=59405
fixture_port=59482
db=intent_deploy
restore_db=intent_deploy_restore
caddy_image="caddy:2.10.2-alpine@sha256:4c6e91c6ed0e2fa03efd5b44747b625fec79bc9cd06ac5235a779726618e530d"
password="deploy-test-password-$(date +%s)"

step() { printf '\n-- deploy: %s\n' "$*"; }
fail() {
    echo "test-deploy: $*" >&2
    for log in "$scratch"/*.log; do
        [ -f "$log" ] && { echo "--- $(basename "$log") (last 15 lines)" >&2; tail -15 "$log" >&2; }
    done
    exit 1
}
psql() { docker compose -p intent-studio -f deploy/compose.yaml exec -T postgres psql -qAt -U intent "$@"; }

scratch="$(mktemp -d)"
pids=()
cleanup() {
    for pid in ${pids[@]+"${pids[@]}"}; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
    psql -d intent -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" -c "DROP DATABASE IF EXISTS $restore_db WITH (FORCE)" >/dev/null 2>&1 || true
    rm -rf "$scratch"
}
trap cleanup EXIT

step "package and install into a scratch prefix"
scripts/stack.sh up >/dev/null
package="$(scripts/package.sh "$scratch/dist")"
tar -C "$scratch" -xzf "$package"
prefix="$scratch/intent-studio"
for file in bin/intent-server bin/intent-worker workspace/200.html deploy/intent-studio.service \
    deploy/intent-studio.env.example deploy/Caddyfile OPERATOR_GUIDE.md; do
    [ -e "$prefix/$file" ] || fail "the package has no $file"
done
cargo build --release --locked -p intent-provider-fixture >&2
fixture_bin="$(cargo metadata --format-version 1 --no-deps --locked | jq -r .target_directory)/release/intent-provider-fixture"

step "configuration from the env example"
psql -d intent -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" -c "CREATE DATABASE $db"
psql -d intent -c "DROP DATABASE IF EXISTS $restore_db WITH (FORCE)" -c "CREATE DATABASE $restore_db"
hash="$(printf '%s\n' "$password" | "$prefix/bin/intent-server" hash-password)"
env_file="$scratch/intent-studio.env"
# Fill the placeholders and point the paths at the scratch prefix, as the operator does.
sed -e "s|postgres://intent:__DB_PASSWORD__@127.0.0.1:5432/intent|postgres://intent:intent-dev@127.0.0.1:59432/$db|" \
    -e "s|__OPERATOR_PASSWORD_HASH__|$hash|" \
    -e "s|^INTENT_BIND=.*|INTENT_BIND=127.0.0.1:$port|" \
    -e "s|http://127.0.0.1:59480/v1/notify|http://127.0.0.1:$fixture_port/v1/notify|" \
    -e "s|/opt/intent-studio|$prefix|g" \
    -e "s|/var/lib/intent-studio/store|$scratch/store|" \
    -e "s|/var/lib/intent-studio/state|$scratch/state|" \
    deploy/intent-studio.env.example >"$env_file"
grep -q '__' <(grep -v '^#' "$env_file") && fail "a placeholder is left in the env file"
run() { (set -a; . "$env_file"; set +a; exec "$prefix/bin/intent-server" "$@"); }
run check-config || fail "check-config refused the filled env example"
if (set -a; . "$env_file"; set +a; unset INTENT_DATABASE_URL; "$prefix/bin/intent-server" check-config) 2>"$scratch/missing.txt"; then
    fail "check-config passed without INTENT_DATABASE_URL"
fi
grep -q 'INTENT_DATABASE_URL: required' "$scratch/missing.txt" || fail "check-config did not name the missing variable: $(cat "$scratch/missing.txt")"

step "migrate and serve in the production profile"
run migrate
# A demo site only so a lead has somewhere to go: real sites arrive with promotion (I3).
run seed --sites 1
"$fixture_bin" --bind "127.0.0.1:$fixture_port" --mode ok >"$scratch/fixture.log" 2>&1 &
pids+=($!)
# The webhook must be up before the first lead: a refused first attempt waits out the
# production retry schedule (a minute), longer than this test waits.
for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:$fixture_port/control/stats" >/dev/null && break; sleep 0.1; done
curl -sf "http://127.0.0.1:$fixture_port/control/stats" >/dev/null || fail "the webhook fixture did not start"
# Started directly (not through `run`): `$!` must be the server itself, so the cleanup and the
# SIGTERM below reach it.
(set -a; . "$env_file"; set +a; exec "$prefix/bin/intent-server" serve) >"$scratch/server.log" 2>&1 &
server=$!
pids+=("$server")
for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:$port/readyz" >/dev/null && break; sleep 0.1; done
curl -sf "http://127.0.0.1:$port/readyz" | grep -q '"ready":true' || fail "not ready: $(tail -5 "$scratch/server.log")"
# The upgrade path runs check-config while the old server still holds the store lock.
run check-config >/dev/null || fail "check-config refused the running server's store"
headers="$(curl -s -D - -o /dev/null "http://127.0.0.1:$port/workspace/")"
grep -qi "^HTTP/1.1 200" <<<"$headers" || fail "the workspace is not served"
grep -qi "content-security-policy: frame-ancestors 'none'" <<<"$headers" || fail "the workspace has no CSP"
cookie="$(curl -s -D - -o /dev/null -H 'content-type: application/json' \
    -d "{\"password\":\"$password\"}" "http://127.0.0.1:$port/v1/operator/session" | tr -d '\r' \
    | sed -n 's/^[Ss]et-[Cc]ookie: \([^;]*\);.*/\1/p')"
[[ "$cookie" == __Host-intent_operator=* ]] || fail "sign-in did not set the __Host- cookie: $cookie"
site=00000000-0000-4000-9000-000000000000
lead="$(curl -sf -H 'content-type: application/json' "http://127.0.0.1:$port/v1/sites/$site/leads" \
    -d '{"submission_id":"deploy-smoke-001","name":"Deploy Smoke","email":"deploy.smoke@example.test","consent":true,"consent_notice":"privacy-v1"}')"
lead_id="$(jq -r .lead_id <<<"$lead")"
[ -n "$lead_id" ] && [ "$lead_id" != null ] || fail "the lead was not stored: $lead"
for _ in $(seq 1 150); do
    [ "$(curl -sf "http://127.0.0.1:$fixture_port/control/stats" | jq -r .unique_accepted)" = 1 ] && break
    sleep 0.1
done
[ "$(curl -sf "http://127.0.0.1:$fixture_port/control/stats" | jq -r .unique_accepted)" = 1 ] || fail "the webhook was not notified"
tenant=00000000-0000-4000-8000-000000000001
asset="$(curl -s -H "cookie: $cookie" -H 'x-intent-upload: 1' \
    -F 'licence=Owned by the client' -F 'source=Deploy test' \
    -F 'file=@fixtures/assets/clean-logo.svg;type=image/svg+xml' \
    "http://127.0.0.1:$port/v1/operator/tenants/$tenant/assets")"
asset_id="$(jq -r .id <<<"$asset")"
[ -n "$asset_id" ] && [ "$asset_id" != null ] || fail "the asset upload failed: $asset"
curl -sf -H "cookie: $cookie" "http://127.0.0.1:$port/v1/operator/tenants/$tenant/assets/$asset_id/content" >"$scratch/asset-before.svg"

step "back up (database, then store) and restore"
docker compose -p intent-studio -f deploy/compose.yaml exec -T postgres \
    pg_dump --format=custom -U intent "$db" >"$scratch/intent.dump"
tar -C "$scratch" -czf "$scratch/store.tgz" store
docker compose -p intent-studio -f deploy/compose.yaml exec -T postgres \
    pg_restore --no-owner --role=intent -U intent -d "$restore_db" <"$scratch/intent.dump"
mkdir -p "$scratch/restored"
tar -C "$scratch/restored" -xzf "$scratch/store.tgz"
restored_env="$scratch/restored.env"
sed -e "s|59432/$db|59432/$restore_db|" -e "s|^INTENT_BIND=.*|INTENT_BIND=127.0.0.1:$restore_port|" \
    -e "s|^INTENT_STORE_DIR=.*|INTENT_STORE_DIR=$scratch/restored/store|" "$env_file" >"$restored_env"
(set -a; . "$restored_env"; set +a; exec "$prefix/bin/intent-server" serve) >"$scratch/restored.log" 2>&1 &
pids+=($!)
for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:$restore_port/readyz" >/dev/null && break; sleep 0.1; done
restored_cookie="$(curl -s -D - -o /dev/null -H 'content-type: application/json' \
    -d "{\"password\":\"$password\"}" "http://127.0.0.1:$restore_port/v1/operator/session" | tr -d '\r' \
    | sed -n 's/^[Ss]et-[Cc]ookie: \([^;]*\);.*/\1/p')"
curl -sf -H "cookie: $restored_cookie" "http://127.0.0.1:$restore_port/v1/operator/leads" \
    | jq -e --arg id "$lead_id" '[.leads[].id] == [$id]' >/dev/null || fail "the restored inbox does not hold the lead"
curl -sf -H "cookie: $restored_cookie" \
    "http://127.0.0.1:$restore_port/v1/operator/tenants/$tenant/assets/$asset_id/content" >"$scratch/asset-after.svg"
cmp -s "$scratch/asset-before.svg" "$scratch/asset-after.svg" || fail "the restored asset bytes differ"

step "graceful stop"
kill -TERM "$server"
stopped=no
for _ in $(seq 1 250); do
    kill -0 "$server" 2>/dev/null || { stopped=yes; break; }
    sleep 0.1
done
[ "$stopped" = yes ] || fail "the server did not stop within 25 s of SIGTERM"
wait "$server" && status=0 || status=$?
[ "$status" = 0 ] || fail "the server exited $status after SIGTERM"
grep -q 'shutdown complete' "$scratch/server.log" || fail "no 'shutdown complete' in the log"
if grep -qE 'example\.test|Deploy Smoke|\$argon2|intent-dev|'"$password" "$scratch/server.log" "$scratch/restored.log"; then
    fail "lead content or a secret reached a server log"
fi

step "the systemd unit and the Caddyfile"
grep -q 'Strict-Transport-Security' deploy/Caddyfile || fail "the Caddyfile sets no HSTS header"
# D-068: hyper's header-read bound also bounds the idle wait on a kept-alive connection and
# closes it without a response when it fires. An upstream pool that holds a connection past
# the bound reuses a dead one — a 502 for a request the server never read. The transport's
# keepalive must stay below INTENT_HEADER_READ_TIMEOUT_MS (10 s default; the env example
# leaves it unset).
keepalive="$(sed -nE 's/^[[:space:]]*keepalive[[:space:]]+([0-9]+)s[[:space:]]*$/\1/p' deploy/Caddyfile)"
[[ $keepalive =~ ^[0-9]+$ ]] \
    || fail "the Caddyfile pins no upstream keepalive below the server's head-read bound (D-068)"
bound_ms="$(sed -nE 's/^INTENT_HEADER_READ_TIMEOUT_MS=([0-9]+)$/\1/p' deploy/intent-studio.env.example)"
bound_ms="${bound_ms:-10000}"
[[ $bound_ms =~ ^[0-9]+$ ]] || fail "INTENT_HEADER_READ_TIMEOUT_MS is not a number of milliseconds"
[ "$keepalive" -lt "$((bound_ms / 1000))" ] \
    || fail "upstream keepalive ${keepalive}s is not below the ${bound_ms} ms head-read bound"
if command -v systemd-analyze >/dev/null 2>&1; then
    unit="$scratch/intent-studio.service"
    sed -e "s|^ExecStart=.*|ExecStart=$prefix/bin/intent-server serve|" \
        -e "s|^EnvironmentFile=.*|EnvironmentFile=$env_file|" \
        -e "s|^User=.*|User=$(id -un)|" -e "s|^Group=.*|Group=$(id -gn)|" \
        deploy/intent-studio.service >"$unit"
    systemd-analyze verify "$unit" || fail "systemd-analyze rejected the unit"
else
    echo "systemd-analyze not available here: the unit is verified in CI"
fi
if docker info >/dev/null 2>&1; then
    docker run --rm -v "$PWD/deploy/Caddyfile:/etc/caddy/Caddyfile:ro" "$caddy_image" \
        caddy validate --adapter caddyfile --config /etc/caddy/Caddyfile >/dev/null 2>"$scratch/caddy.txt" \
        || fail "caddy rejected the Caddyfile: $(cat "$scratch/caddy.txt")"
else
    echo "Docker not available here: the Caddyfile is validated in CI"
fi

echo "test-deploy ok"
