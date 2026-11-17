#!/usr/bin/env bash
# Lead-intake load runs with the notification provider down (END_GOAL evidence plan, D-025).
#
#   scripts/load.sh realism       50 submissions/s for 60 s       (the guide's realism check)
#   scripts/load.sh burst         250 submissions/s for 60 s      (5x burst)
#   scripts/load.sh notify-down   100,000 submissions at RATE      (failure case 3, smoke size)
#   scripts/load.sh full          1,000,000 submissions at RATE    (the full run; RATE required)
#
# Each run: release binaries, a fresh database `intent_load` on the local stack with SITES demo
# sites, the provider fixture refusing (503) for the whole run, k6 (open model, 5% deliberate
# resubmissions, every acknowledgment logged), then recovery (fixture back to ok), a wait for the
# outbox to drain, and `intent-server reconcile --require-terminal`: every acknowledged lead
# stored exactly once under its lead id, one outbox row per lead, every row delivered or dead
# after 8 attempts. The result — machine, commit, `uptime` before and after, k6 metrics, the
# verdict — is written to perf/results/ whether it passed or not (misses are kept).
#
# Overrides: RATE, TOTAL, P99_MS, SITES, RETRY_BASE_MS (default 1000: the D-030 schedule at 1/60
# scale, so a run sees the whole shape in minutes), DRAIN_TIMEOUT_S, K6.
set -euo pipefail
cd "$(dirname "$0")/.."

scenario="${1:-}"
case "$scenario" in
    realism) : "${RATE:=50}" "${TOTAL:=3000}" "${P99_MS:=100}" ;;
    burst) : "${RATE:=250}" "${TOTAL:=15000}" "${P99_MS:=100}" ;;
    notify-down) : "${RATE:=500}" "${TOTAL:=100000}" "${P99_MS:=100}" ;;
    full) : "${RATE:?set RATE for the full run}" "${TOTAL:=1000000}" "${P99_MS:=100}" ;;
    *) echo "usage: scripts/load.sh realism|burst|notify-down|full" >&2; exit 2 ;;
esac
: "${SITES:=10000}" "${RETRY_BASE_MS:=1000}" "${DRAIN_TIMEOUT_S:=1800}"
k6="${K6:-$(command -v k6 || echo "$HOME/.local/bin/k6")}"

port=59403
fixture_port=59481
db=intent_load
url="postgres://intent:intent-dev@127.0.0.1:59432/$db"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
run="load-${scenario}-${stamp}"
work="perf/out/$run"
mkdir -p "$work" perf/results
psql() { docker compose -p intent-studio -f deploy/compose.yaml exec -T postgres psql -qAt -U intent "$@"; }

scripts/stack.sh up >/dev/null
cargo build --release --locked -p intent-server -p intent-provider-fixture
psql -d intent -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" -c "CREATE DATABASE $db"
INTENT_DATABASE_URL="$url" target/release/intent-server migrate
INTENT_DATABASE_URL="$url" target/release/intent-server seed --sites "$SITES"

target/release/intent-provider-fixture --bind "127.0.0.1:$fixture_port" --mode unavailable \
    >"$work/fixture.log" 2>&1 &
fixture=$!
INTENT_DATABASE_URL="$url" INTENT_BIND="127.0.0.1:$port" INTENT_LOG=warn \
    INTENT_NOTIFY_PROVIDER=webhook INTENT_NOTIFY_WEBHOOK_URL="http://127.0.0.1:$fixture_port/v1/notify" \
    INTENT_NOTIFY_RETRY_BASE_MS="$RETRY_BASE_MS" INTENT_TRUSTED_PROXIES=127.0.0.1 \
    INTENT_DB_MAX_CONNECTIONS=32 \
    target/release/intent-server serve >"$work/server.log" 2>&1 &
server=$!
trap 'kill "$server" "$fixture" 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$port/readyz" >/dev/null && break
    sleep 0.1
done

uptime_before="$(uptime)"
started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
set +e
"$k6" run --quiet --console-output "$work/acks.log" \
    -e BASE_URL="http://127.0.0.1:$port" -e RUN="$run" -e SITES="$SITES" -e RATE="$RATE" \
    -e TOTAL="$TOTAL" -e P99_MS="$P99_MS" -e SUMMARY="$work/k6-summary.json" \
    perf/k6/leads.js
k6_status=$?
set -e
uptime_after="$(uptime)"
metrics_down="$(curl -sf "http://127.0.0.1:$port/metrics" || true)"
fixture_down="$(curl -sf "http://127.0.0.1:$fixture_port/control/stats" || echo '{}')"

# Recovery: the provider comes back; wait for every outbox row to be delivered or dead.
curl -sf -X PUT "http://127.0.0.1:$fixture_port/control/mode" -H 'content-type: application/json' \
    -d '{"mode":"ok"}'
drain_started=$(date +%s)
while :; do
    pending="$(psql -d "$db" -c "SELECT count(*) FROM notification_outbox WHERE state = 'pending'")"
    [ "$pending" = 0 ] && break
    if [ $(( $(date +%s) - drain_started )) -gt "$DRAIN_TIMEOUT_S" ]; then
        echo "outbox did not drain within ${DRAIN_TIMEOUT_S}s ($pending pending)" >&2
        break
    fi
    sleep 5
done
drain_seconds=$(( $(date +%s) - drain_started ))
fixture_after="$(curl -sf "http://127.0.0.1:$fixture_port/control/stats" || echo '{}')"
metrics_after="$(curl -sf "http://127.0.0.1:$port/metrics" || true)"

set +e
INTENT_DATABASE_URL="$url" target/release/intent-server reconcile --prefix "$run" \
    --acks "$work/acks.log" --require-terminal >"$work/verdict.json"
reconcile_status=$?
set -e
leaks="$(grep -cE 'example\.test|Load Lead |\+1 555 01|Quote request ' "$work/server.log" || true)"

commit="$(git rev-parse HEAD)"
# Untracked files count too: a result names exactly what produced it.
[ -z "$(git status --porcelain -- crates perf/k6 scripts deploy Cargo.toml Cargo.lock)" ] \
    || commit="$commit (with uncommitted changes)"
machine="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m), $(sysctl -n hw.ncpu 2>/dev/null || nproc) cores, $(( $(sysctl -n hw.memsize 2>/dev/null || echo 0) / 1073741824 )) GiB, $(sw_vers -productName 2>/dev/null || uname -s) $(sw_vers -productVersion 2>/dev/null || uname -r)"
pass=yes
[ "$k6_status" = 0 ] && [ "$reconcile_status" = 0 ] && [ "$leaks" = 0 ] || pass=no

result="perf/results/$(date -u +%Y-%m-%d)-${scenario}-$(git rev-parse --short HEAD).json"
metric() { grep -E "^intent_$1 " <<<"$2" | awk '{print $2}' | head -1; }
jq -n \
    --arg scenario "$scenario" --arg run "$run" --arg started "$started" --arg commit "$commit" \
    --arg machine "$machine" --arg uptime_before "$uptime_before" --arg uptime_after "$uptime_after" \
    --arg pass "$pass" --argjson k6_status "$k6_status" --argjson reconcile_status "$reconcile_status" \
    --argjson rate "$RATE" --argjson total "$TOTAL" --argjson p99_ms "$P99_MS" --argjson sites "$SITES" \
    --argjson retry_base_ms "$RETRY_BASE_MS" --argjson drain_seconds "$drain_seconds" \
    --argjson leaks "$leaks" \
    --argjson fixture_down "$fixture_down" --argjson fixture_after "$fixture_after" \
    --slurpfile k6 "$work/k6-summary.json" --slurpfile verdict "$work/verdict.json" \
    --arg attempts "$(metric notify_attempts_total "$metrics_after")" \
    --arg dead "$(metric notify_dead_total "$metrics_after")" \
    --arg timeouts "$(metric notify_timeouts_total "$metrics_after")" \
    --arg shed "$(metric intake_shed_total "$metrics_down")" \
    --arg unavailable "$(metric intake_unavailable_total "$metrics_down")" \
    --arg address_keys "$(metric rate_table_address_keys "$metrics_down")" \
    --arg address_evictions "$(metric rate_table_address_evictions "$metrics_down")" \
    '{
      scenario: $scenario, run: $run, started: $started, commit: $commit, machine: $machine,
      uptime_before: $uptime_before, uptime_after: $uptime_after,
      note: "Development observation on a shared laptop, not production capacity. The load generator, the server and Postgres (Docker) share the machine.",
      config: { rate_per_s: $rate, total: $total, p99_threshold_ms: $p99_ms, sites: $sites,
                provider: "fixture refusing with 503 for the whole run", resubmissions: "every 20th (5%)",
                retry_base_ms: $retry_base_ms, db_connections: 32 },
      pass: ($pass == "yes"), k6_exit: $k6_status, reconcile_exit: $reconcile_status,
      k6: {
        iterations: $k6[0].metrics.iterations.values.count,
        achieved_rate_per_s: $k6[0].metrics.iterations.values.rate,
        dropped_iterations: ($k6[0].metrics.dropped_iterations.values.count // 0),
        http_req_duration_ms: $k6[0].metrics.http_req_duration.values,
        acknowledged_rate: $k6[0].metrics.lead_acknowledged.values.rate,
        refused: ($k6[0].metrics.lead_refused.values.count // 0),
        unanswered: ($k6[0].metrics.lead_unanswered.values.count // 0),
        replays: ($k6[0].metrics.lead_replays.values.count // 0),
        thresholds: ($k6[0].metrics | to_entries | map(select(.value.thresholds != null) | {key, value: .value.thresholds}) | from_entries)
      },
      reconcile: $verdict[0],
      provider_fixture: { at_end_of_run: $fixture_down, after_recovery: $fixture_after },
      notifier: { attempts: $attempts, dead: $dead, timeouts: $timeouts, drain_seconds_after_recovery: $drain_seconds },
      intake: { shed: $shed, unavailable: $unavailable, address_table_keys: $address_keys, address_table_evictions: $address_evictions },
      log_lines_with_lead_content: $leaks
    }' >"$result"

echo "result: $result (pass: $pass)"
jq '{pass, k6: {iterations: .k6.iterations, achieved_rate_per_s: .k6.achieved_rate_per_s, dropped_iterations: .k6.dropped_iterations, p50: .k6.http_req_duration_ms.med, p99: .k6.http_req_duration_ms["p(99)"], acknowledged_rate: .k6.acknowledged_rate}, reconcile: {violations: .reconcile.violations, acknowledged_submissions: .reconcile.acknowledged_submissions, outbox: .reconcile.outbox}, uptime_before}' "$result"
[ "$pass" = yes ]
