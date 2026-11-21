#!/usr/bin/env bash
# The I0 drills against the real intent-server binary (release build, as deployed) and the local
# stack: intake behaviour, the studio API (I1), connection bounds, notify-down, kill -9 and db-down, each drill with
# the log-capture check.
# db-down stops the stack's Postgres, so it lives in a test binary of its own: Cargo runs test
# binaries one at a time (in name order, not the order given here), so nothing else is running
# while Postgres is down.
#
# A red suite must never hide the others (B2): every suite runs, each exit code is
# recorded, and the script fails once at the end naming everything that went red.
set -euo pipefail
cd "$(dirname "$0")/.."

scripts/stack.sh up
# The asset worker is a binary of its own crate; the asset drill runs the real one.
cargo build --release --locked -p intent-worker
failed=0
run() {
    local name="$1"
    shift
    echo "== $name =="
    if "$@"; then
        echo "== $name: ok =="
    else
        echo "== $name: FAILED =="
        failed=1
    fi
}

# The database layer's ignored tests run on the live stack — N15's class lives there
# and nothing else exercises it.
run intent-db cargo test --release --locked -p intent-db --no-fail-fast -- --ignored
run drills cargo test --release --locked -p intent-server \
    --no-fail-fast --test harness --test intake --test operator --test studio --test assets --test connections --test outbox_fencing --test notifier_pool --test readiness --test drill_notify_down --test drill_kill --test drill_db_down \
    -- --ignored
# The flood drill advances the server clock through POST /__test/clock — a route that
# exists only under the mutation-seams feature (K: no env var may move the shipped
# clock). It runs here against a feature build; the default build above must refuse
# to carry it (the drill itself asserts 404 without the env var).
run drills-clock cargo test --release --locked -p intent-server --features mutation-seams \
    --no-fail-fast --test operator \
    -- --ignored a_flood_of_bad_session_cookies_is_limited_and_leaves_intake_alone
# The attacks run against the default build — what ships — first: every guard must
# refuse when INTENT_MUTATION is not even parsed (the variable is feature-gated).
run attacks cargo test --release --locked -p intent-server \
    --no-fail-fast --test attacks \
    -- --ignored
# The mutation proofs are their own target under the mutation-seams feature: they need
# the seams compiled in (they set INTENT_MUTATION per boot). Without the feature the
# proofs go red — which is the point: a shipped server cannot be tricked into
# carrying them.
run mutation cargo test --release --locked -p intent-server --features mutation-seams \
    --no-fail-fast --test mutation \
    -- --ignored

if [ "$failed" -ne 0 ]; then
    echo "drills.sh: one or more suites FAILED (each is marked above)" >&2
    exit 1
fi
echo "drills.sh: all suites passed"
