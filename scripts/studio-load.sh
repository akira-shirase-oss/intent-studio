#!/usr/bin/env bash
# Studio API load runs (I1, D-063): the operator's reads and the page-tree spec check under k6.
#
#   scripts/studio-load.sh baseline    the first quiet baseline: 200 requests/s for 60 s —
#                                      the run D-063's proposed thresholds are set from
#   scripts/studio-load.sh full        1,000,000 requests at RATE (the full run; RATE required)
#
# Each run: release binary, a fresh database on the local stack, the server in the production
# profile (the deployed shape; the studio API never notifies, so the configured webhook is
# never called), then k6's constant-arrival mix — 50% the fact list, 25% one brief, 25% a
# spec check of a valid page tree (the heaviest route: it loads the brief and every referenced
# fact, validates and lints). Thresholds are passed to perf/k6/studio.js first, so a result
# records what it was judged against; the log is scanned for record content and secrets. The
# result — machine, commit, `uptime` before and after, k6 metrics, the verdict — is written to
# perf/results/ whether it passed or not (misses are kept, as with load.sh).
#
# Overrides: RATE, TOTAL, P99_READ_MS, P99_SPEC_MS, P999_READ_MS, P999_SPEC_MS (0: measured,
# not gated — D-063 judges the tail on CI's dedicated run), K6, KEEP_DB (keep the run's
# database instead of dropping it), LOG_LEVEL.
set -euo pipefail
cd "$(dirname "$0")/.."

scenario="${1:-}"
case "$scenario" in
    # The thresholds of D-063 (the addendum of 2026-11-20: the baseline's p99 × 4).
    baseline) : "${RATE:=200}" "${TOTAL:=12000}" "${P99_READ_MS:=45}" "${P99_SPEC_MS:=45}" ;;
    full)     : "${RATE:?set RATE for the full run}" "${TOTAL:=1000000}" "${P99_READ_MS:=45}" "${P99_SPEC_MS:=45}" ;;
    *) echo "usage: scripts/studio-load.sh baseline|full" >&2; exit 2 ;;
esac
: "${LOG_LEVEL:=debug,hyper=info,h2=info,sqlx=warn,reqwest=info}"
k6="${K6:-$(command -v k6 || echo "$HOME/.local/bin/k6")}"

port=59406
fixture_port=59483   # configured but never called: the studio API notifies nothing
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
# A database of its own per run, so nothing from an earlier run can join it by name (D-037).
db="intent_studio_$(tr '[:upper:]' '[:lower:]' <<<"$stamp")"
url="postgres://intent:intent-dev@127.0.0.1:59432/$db"
run="studio-${scenario}-${stamp}"
work="perf/out/$run"
mkdir -p "$work" perf/results
psql() { docker compose -p intent-studio -f deploy/compose.yaml exec -T postgres psql -qAt -U intent "$@"; }
password="studio-load-$(date +%s)"

scripts/stack.sh up >/dev/null
cargo build --release --locked -p intent-server
# The binary just built, wherever CARGO_TARGET_DIR or a cargo config puts it (never a stale
# ./target: scripts/test-scripts.sh, rule S1).
bin="$(cargo metadata --format-version 1 --no-deps --locked | jq -r .target_directory)/release"
psql -d intent -c "CREATE DATABASE $db"
INTENT_DATABASE_URL="$url" "$bin"/intent-server migrate
hash="$(printf '%s\n' "$password" | "$bin"/intent-server hash-password)"

# Every level is logged, so the leak scan below covers what a debug line would print too
# (LOG_LEVEL overrides; the level used is recorded in the result).
INTENT_DATABASE_URL="$url" INTENT_BIND="127.0.0.1:$port" INTENT_LOG="$LOG_LEVEL" \
    INTENT_NOTIFY_PROVIDER=webhook INTENT_NOTIFY_WEBHOOK_URL="http://127.0.0.1:$fixture_port/v1/notify" \
    INTENT_OPERATOR_PASSWORD_HASH="$hash" INTENT_TRUSTED_PROXIES=127.0.0.1 \
    "$bin"/intent-server serve >"$work/server.log" 2>&1 &
server=$!
cleanup() {
    kill "$server" 2>/dev/null || true
    wait "$server" 2>/dev/null || true
    [ -n "${KEEP_DB:-}" ] || psql -d intent -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" || true
}
trap cleanup EXIT
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$port/readyz" >/dev/null && break
    sleep 0.1
done

uptime_before="$(uptime)"
started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
set +e
"$k6" run --quiet \
    -e BASE_URL="http://127.0.0.1:$port" -e PASSWORD="$password" -e RATE="$RATE" \
    -e TOTAL="$TOTAL" -e P99_READ_MS="$P99_READ_MS" -e P99_SPEC_MS="$P99_SPEC_MS" \
    -e SUMMARY="$work/k6-summary.json" \
    perf/k6/studio.js
k6_status=$?
set -e
uptime_after="$(uptime)"

# Content the scenario writes ("Load Heating", "Boiler service", load.example) and secret
# shapes (the operator password and its argon2 hash, the database URL, the session cookie), at
# every level the server logged: no route may record record content or a credential.
leaks="$(grep -cE 'Load Heating|Boiler service|load\.example|postgres://|intent-dev|\$argon2|[Bb]earer |intent_operator|'"$password" "$work/server.log" || true)"
log_lines="$(wc -l <"$work/server.log" | tr -d ' ')"

commit="$(git rev-parse HEAD)"
# Untracked files count too: a result names exactly what produced it.
[ -z "$(git status --porcelain -- crates apps packages perf/k6 scripts Cargo.toml Cargo.lock package.json package-lock.json)" ] \
    || commit="$commit (with uncommitted changes)"
machine="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m), $(sysctl -n hw.ncpu 2>/dev/null || nproc) cores, $(( $(sysctl -n hw.memsize 2>/dev/null || echo 0) / 1073741824 )) GiB, $(sw_vers -productName 2>/dev/null || uname -s) $(sw_vers -productVersion 2>/dev/null || uname -r)"
pass=yes
[ "$k6_status" = 0 ] && [ "$leaks" = 0 ] || pass=no

result="perf/results/$(date -u +%Y-%m-%d)-studio-${scenario}-$(git rev-parse --short HEAD).json"
jq -n \
    --arg scenario "studio-$scenario" --arg run "$run" --arg started "$started" --arg commit "$commit" \
    --arg machine "$machine" --arg uptime_before "$uptime_before" --arg uptime_after "$uptime_after" \
    --arg pass "$pass" --argjson k6_status "$k6_status" \
    --argjson rate "$RATE" --argjson total "$TOTAL" \
    --argjson p99_read_ms "$P99_READ_MS" --argjson p99_spec_ms "$P99_SPEC_MS" \
    --argjson leaks "$leaks" --arg database "$db" \
    --arg log_level "$LOG_LEVEL" --argjson log_lines "$log_lines" \
    --slurpfile k6 "$work/k6-summary.json" \
    '{
      scenario: $scenario, run: $run, started: $started, commit: $commit, machine: $machine,
      uptime_before: $uptime_before, uptime_after: $uptime_after,
      note: "Development observation on a shared laptop, not production capacity. The load generator, the server and Postgres (Docker) share the machine.",
      config: { rate_per_s: $rate, total: $total,
                p99_read_threshold_ms: $p99_read_ms, p99_spec_threshold_ms: $p99_spec_ms, budget: "D-063 thresholds (baseline p99 x4)",
                mix: "50% GET facts, 25% GET brief, 25% POST spec-check", profile: "production",
                provider: "webhook configured, never called (the studio API notifies nothing)" },
      pass: ($pass == "yes"), k6_exit: $k6_status,
      k6: {
        iterations: $k6[0].metrics.iterations.values.count,
        achieved_rate_per_s: $k6[0].metrics.iterations.values.rate,
        dropped_iterations: ($k6[0].metrics.dropped_iterations.values.count // 0),
        read_ms: $k6[0].metrics["http_req_duration{kind:read}"].values,
        spec_ms: $k6[0].metrics["http_req_duration{kind:spec}"].values,
        all_ms: $k6[0].metrics.http_req_duration.values,
        failed_rate: $k6[0].metrics.http_req_failed.values.rate,
        thresholds: ($k6[0].metrics | to_entries | map(select(.value.thresholds != null) | {key, value: .value.thresholds}) | from_entries)
      },
      log_lines_with_content_or_secrets: $leaks,
      log: { level: $log_level, lines_scanned: $log_lines },
      database: $database
    }' >"$result"

echo "result: $result (pass: $pass)"
jq '{pass, k6: {iterations: .k6.iterations, achieved_rate_per_s: .k6.achieved_rate_per_s, dropped_iterations: .k6.dropped_iterations, read_p99: .k6.read_ms["p(99)"], spec_p99: .k6.spec_ms["p(99)"], failed_rate: .k6.failed_rate}, uptime_before}' "$result"
[ "$pass" = yes ]
