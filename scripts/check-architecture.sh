#!/usr/bin/env bash
# The architecture rules from docs/ROADMAP.md §I0, enforced on every check.sh run and in CI:
#
#   A1  intent-domain has no I/O: no runtime, database, HTTP or filesystem crate among its
#       dependencies (direct, renamed or transitive) and no I/O call in its source;
#   A2  only intent-agent may depend on a model-provider client;
#   A3  an HTTP client crate may appear only in intent-egress (client-named URLs, D-012),
#       intent-agent (the model port) and intent-notify (the operator-configured notification
#       provider, D-032) — never in the server, the database layer or the domain;
#   A4  intent-review may not depend on intent-publish (no code path from review to promotion);
#   A5  the workspace app (apps/*, packages/*) never imports a database client.
#
# Usage: scripts/check-architecture.sh [repository root]. The optional root lets
# scripts/test-architecture-check.sh prove that every rule can fail.
set -euo pipefail

root="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
cd "$root"
failures=0

fail() {
    echo "architecture: $*" >&2
    failures=$((failures + 1))
}

# Dependency names of one crate's manifest, one per line: keys of [dependencies],
# [build-dependencies] and [target.*.dependencies] (inline and table forms), plus the real name
# behind any `package = "..."` rename. Dev-dependencies are excluded: tests may use anything.
deps_of() {
    local manifest="$1/Cargo.toml"
    [ -f "$manifest" ] || return 0
    awk '
        /^\[/ {
            section = $0
            gsub(/[[:space:]]/, "", section)
            normal = (section ~ /^\[(target\..*\.)?(build-)?dependencies\]$/)
            table = ""
            if (match(section, /^\[(target\..*\.)?(build-)?dependencies\.[^]]+\]$/)) {
                table = section
                sub(/^.*dependencies\./, "", table)
                sub(/\]$/, "", table)
                print table
            }
            next
        }
        normal && /^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*(\.|=)/ {
            name = $0
            sub(/^[[:space:]]*/, "", name)
            sub(/[[:space:]]*(\.|=).*/, "", name)
            print name
        }
        (normal || table != "") && /package[[:space:]]*=[[:space:]]*"/ {
            pkg = $0
            sub(/.*package[[:space:]]*=[[:space:]]*"/, "", pkg)
            sub(/".*/, "", pkg)
            print pkg
        }
    ' "$manifest" | sort -u
}

has_dep() { # crate_dir, extended regex of names
    deps_of "$1" | grep -Eqx "$2"
}

io_crates='tokio|tokio-util|tokio-postgres|async-std|smol|mio|socket2|sqlx|sqlx-.*|postgres|diesel|redis|reqwest|hyper|hyper-util|axum|tower|tower-http|ureq|isahc|surf|attohttpc|curl|tempfile|notify|fs-err|walkdir'
provider_clients='async-openai|openai|openai-api-rs|anthropic|anthropic-sdk|misanthropy|genai|rig-core|ollama-rs|google-generativeai|aws-sdk-bedrockruntime|mistralai-client|cohere-rust|llm'
http_clients='reqwest|ureq|isahc|surf|attohttpc|curl|awc'
db_clients='pg|postgres|@neondatabase/serverless|@vercel/postgres|@prisma/client|prisma|drizzle-orm|kysely|knex|sequelize|typeorm|mysql2|better-sqlite3|sqlite3|@supabase/supabase-js|@libsql/client'

# A1: the domain is pure.
domain=crates/intent-domain
if [ -d "$domain" ]; then
    bad="$(deps_of "$domain" | grep -Ex "$io_crates" || true)"
    [ -z "$bad" ] || fail "A1 intent-domain depends on I/O crate(s): $(echo "$bad" | tr '\n' ' ')"
    [ ! -e "$domain/build.rs" ] || fail "A1 intent-domain must not have a build script"
    calls="$(grep -rnE 'std::fs|std::process|std::env|TcpStream|TcpListener|UdpSocket|thread::sleep|SystemTime::now|Instant::now|File::(open|create)|tokio::' "$domain/src" || true)"
    [ -z "$calls" ] || fail "A1 intent-domain source performs I/O:"$'\n'"$calls"
    # Grouped imports (`use std::{fs, io};`, also across lines) name the module without the
    # `std::fs` spelling: join each file into one line and look inside every `std::{...}` group.
    grouped="$(find "$domain/src" -name '*.rs' -print0 | while IFS= read -r -d '' file; do
        tr '\n' ' ' <"$file" | grep -oE 'std::\{[^;]*\}' \
            | grep -E '(^|[^A-Za-z0-9_])(fs|process|env|thread)([^A-Za-z0-9_]|$)|Tcp|Udp' \
            | sed "s|^|$file: |" || true
    done)"
    [ -z "$grouped" ] || fail "A1 intent-domain imports I/O in a grouped use:"$'\n'"$grouped"
    # Transitive: what Cargo actually resolves (only in a real, locked workspace).
    if [ -f Cargo.lock ] && command -v cargo >/dev/null 2>&1; then
        resolved="$(cargo tree --locked --quiet -p intent-domain -e normal,build --prefix none --format '{p}' 2>/dev/null \
            | awk '{print $1}' | sort -u)" || resolved=""
        [ -n "$resolved" ] || fail "A1 could not resolve intent-domain's dependency tree (cargo tree failed)"
        bad="$(echo "$resolved" | grep -Ex "$io_crates" || true)"
        [ -z "$bad" ] || fail "A1 intent-domain pulls I/O crate(s) in transitively: $(echo "$bad" | tr '\n' ' ')"
    fi
fi

for dir in crates/*/; do
    dir="${dir%/}"
    [ -f "$dir/Cargo.toml" ] || continue
    name="$(basename "$dir")"
    # A2: provider clients only in the agent.
    if [ "$name" != intent-agent ] && has_dep "$dir" "$provider_clients"; then
        fail "A2 $name depends on a model-provider client; only intent-agent may"
    fi
    # A3: HTTP clients only where an outbound call is the crate's job.
    case "$name" in
        intent-egress | intent-agent | intent-notify) ;;
        *) if has_dep "$dir" "$http_clients"; then
            fail "A3 $name depends on an HTTP client; only intent-egress, intent-agent and intent-notify may"
        fi ;;
    esac
done

# A4: review never reaches publish.
if [ -d crates/intent-review ] && has_dep crates/intent-review 'intent-publish'; then
    fail "A4 intent-review depends on intent-publish"
fi

# A5: no database client in the browser workspace.
for pkg in apps/*/package.json packages/*/package.json; do
    [ -f "$pkg" ] || continue
    bad="$(jq -r '((.dependencies // {}) + (.devDependencies // {}) + (.peerDependencies // {})) | keys[]' "$pkg" \
        | grep -Ex "$db_clients" || true)"
    [ -z "$bad" ] || fail "A5 $pkg depends on a database client: $(echo "$bad" | tr '\n' ' ')"
    src="$(dirname "$pkg")/src"
    if [ -d "$src" ]; then
        imports="$(grep -rnE "(from|import\\()[[:space:]]*['\"]($db_clients)(/[^'\"]*)?['\"]" "$src" || true)"
        [ -z "$imports" ] || fail "A5 $(dirname "$pkg") imports a database client:"$'\n'"$imports"
    fi
done

if [ "$failures" -gt 0 ]; then
    echo "architecture check failed: $failures rule violation(s)" >&2
    exit 1
fi
echo "architecture ok: A1 pure domain, A2 provider clients, A3 HTTP clients, A4 review/publish, A5 no database client in the workspace app"
