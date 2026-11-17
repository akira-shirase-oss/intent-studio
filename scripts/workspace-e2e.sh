#!/usr/bin/env bash
# The workspace app in Chromium (desktop and a 360 px phone) against the real server: builds the
# app and the release binary, gives them a fresh database on the local stack, stores three leads
# through the public intake endpoint, and runs apps/workspace/tests with Playwright.
set -euo pipefail
cd "$(dirname "$0")/.."

port=59402
db=intent_e2e
url="postgres://intent:intent-dev@127.0.0.1:59432/$db"
password="e2e-operator-password-$(date +%s)"

scripts/stack.sh up
npm run build --workspace @intent-studio/workspace
cargo build --release --locked -p intent-server
docker compose -p intent-studio -f deploy/compose.yaml exec -T postgres \
    psql -q -U intent -d intent -c "DROP DATABASE IF EXISTS $db WITH (FORCE)" -c "CREATE DATABASE $db"
INTENT_DATABASE_URL="$url" target/release/intent-server migrate
INTENT_DATABASE_URL="$url" target/release/intent-server seed --sites 2
hash="$(printf '%s\n' "$password" | target/release/intent-server hash-password)"

log="$(mktemp)"
INTENT_DATABASE_URL="$url" INTENT_BIND="127.0.0.1:$port" INTENT_NOTIFY_PROVIDER=recorder INTENT_PROFILE=development \
    INTENT_OPERATOR_PASSWORD_HASH="$hash" INTENT_COOKIE_SECURE=false \
    INTENT_WORKSPACE_DIR=apps/workspace/build INTENT_NOTIFY_POLL_MS=50 \
    target/release/intent-server serve >"$log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; rm -f "$log"' EXIT
for _ in $(seq 1 100); do
    curl -sf "http://127.0.0.1:$port/readyz" >/dev/null && break
    sleep 0.1
done

site=00000000-0000-4000-9000-000000000001
for who in oldest middle newest; do
    name="E2E $(tr '[:lower:]' '[:upper:]' <<<"${who:0:1}")${who:1}"
    curl -sf -X POST "http://127.0.0.1:$port/v1/sites/$site/leads" -H 'content-type: application/json' \
        -d "{\"submission_id\":\"e2e-lead-$who\",\"name\":\"$name\",\"email\":\"e2e.$who@example.test\",\"service\":\"boiler service\",\"area\":\"N1\",\"message\":\"A long message to see how the inbox wraps text on a phone, $who.\",\"consent\":true,\"consent_notice\":\"privacy-v1\"}" >/dev/null
    sleep 0.05
done
sleep 0.5

(cd apps/workspace && WORKSPACE_BASE_URL="http://127.0.0.1:$port" WORKSPACE_PASSWORD="$password" npx playwright test --output test-results)
if grep -qE 'example\.test|E2E (Oldest|Middle|Newest)|\$argon2|'"$password" "$log"; then
    echo "workspace-e2e: lead content or a secret reached the server log" >&2
    exit 1
fi
echo "workspace-e2e ok"
