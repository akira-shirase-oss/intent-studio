#!/usr/bin/env bash
# Proves scripts/check-architecture.sh can fail: a clean fixture tree passes, and each planted
# violation (one per rule, plus a renamed dependency and source-level I/O) fails with the rule's
# name in the message. A check that cannot fail proves nothing.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
check="$here/check-architecture.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

clean_tree() { # dir
    mkdir -p "$1/crates/intent-domain/src" "$1/crates/intent-server" "$1/crates/intent-notify" \
        "$1/crates/intent-review" "$1/apps/workspace/src"
    printf '[package]\nname = "intent-domain"\n\n[dependencies]\nserde.workspace = true\nsha2 = "0.11"\n\n[dev-dependencies]\ntokio = "1"\n' \
        > "$1/crates/intent-domain/Cargo.toml"
    printf 'pub fn pure() -> u8 { 1 }\n' > "$1/crates/intent-domain/src/lib.rs"
    printf '[package]\nname = "intent-server"\n\n[dependencies]\naxum.workspace = true\nintent-notify.workspace = true\n\n[dev-dependencies]\nreqwest.workspace = true\n' \
        > "$1/crates/intent-server/Cargo.toml"
    printf '[package]\nname = "intent-notify"\n\n[dependencies]\nreqwest.workspace = true\n' \
        > "$1/crates/intent-notify/Cargo.toml"
    printf '[package]\nname = "intent-review"\n\n[dependencies]\nintent-domain.workspace = true\n' \
        > "$1/crates/intent-review/Cargo.toml"
    printf '{"name":"w","dependencies":{"svelte":"5"},"devDependencies":{"vite":"8"}}\n' \
        > "$1/apps/workspace/package.json"
    printf "import { x } from 'svelte';\n" > "$1/apps/workspace/src/app.ts"
}

# Inserts a dependency line right after a manifest's [dependencies] header.
add_dep() { # manifest, line
    awk -v line="$2" '{ print } /^\[dependencies\]$/ { print line }' "$1" > "$1.new" && mv "$1.new" "$1"
}

passed=0
expect_fail() { # name, rule, mutation (run inside the tree)
    local dir="$work/$1"
    clean_tree "$dir"
    (cd "$dir" && eval "$3")
    local out
    if out="$("$check" "$dir" 2>&1)"; then
        echo "FAIL: '$1' should have been refused by the architecture check" >&2
        exit 1
    fi
    if ! grep -q "$2" <<<"$out"; then
        echo "FAIL: '$1' failed, but not with rule $2:" >&2
        echo "$out" >&2
        exit 1
    fi
    passed=$((passed + 1))
}

clean_tree "$work/clean"
"$check" "$work/clean" >/dev/null || { echo "FAIL: the clean fixture tree was refused" >&2; exit 1; }

expect_fail domain-tokio A1 "add_dep crates/intent-domain/Cargo.toml 'tokio = \"1\"'"
expect_fail domain-renamed A1 "printf '\n[dependencies.db]\npackage = \"sqlx\"\nversion = \"0.9\"\n' >> crates/intent-domain/Cargo.toml"
expect_fail domain-target-dep A1 "printf '\n[target.\"cfg(unix)\".dependencies]\nhyper = \"1\"\n' >> crates/intent-domain/Cargo.toml"
expect_fail domain-fs A1 "printf 'pub fn read() { let _ = std::fs::read(\"x\"); }\n' >> crates/intent-domain/src/lib.rs"
expect_fail domain-grouped-fs A1 "printf 'use std::{fs, io};\\npub fn f() {}\\n' >> crates/intent-domain/src/lib.rs"
expect_fail domain-grouped-multiline A1 "printf 'use std::{\\n    io,\\n    process::Command,\\n};\\n' >> crates/intent-domain/src/lib.rs"
expect_fail domain-grouped-net A1 "printf 'use std::{net::{IpAddr, TcpStream}};\\n' >> crates/intent-domain/src/lib.rs"
expect_fail domain-clock A1 "printf 'pub fn now() { let _ = std::time::SystemTime::now(); }\n' >> crates/intent-domain/src/lib.rs"
expect_fail domain-calendar-clock A1 "printf 'pub fn today() -> time::Date { time::OffsetDateTime::now_utc().date() }\n' >> crates/intent-domain/src/lib.rs"
expect_fail server-provider A2 "add_dep crates/intent-server/Cargo.toml 'async-openai = \"0.30\"'"
expect_fail server-http-client A3 "add_dep crates/intent-server/Cargo.toml 'reqwest.workspace = true'"
expect_fail server-raw-hyper-client A3 "mkdir -p crates/intent-server/src && printf 'use hyper::client::conn::http1;\n' > crates/intent-server/src/fetch.rs"
expect_fail review-publish A4 "printf 'intent-publish.workspace = true\n' >> crates/intent-review/Cargo.toml"
expect_fail app-db-dependency A5 "printf '{\"name\":\"w\",\"dependencies\":{\"pg\":\"8\"}}\n' > apps/workspace/package.json"
expect_fail server-decoder A6 "add_dep crates/intent-server/Cargo.toml 'image.workspace = true'"
expect_fail server-links-worker A6 "add_dep crates/intent-server/Cargo.toml 'intent-worker.workspace = true'"
expect_fail store-xml-parser A6 "mkdir -p crates/intent-store && printf '[package]\nname = \"intent-store\"\n\n[dependencies]\nroxmltree = \"0.21\"\n' > crates/intent-store/Cargo.toml"
expect_fail app-db-import A5 "printf \"import postgres from 'postgres';\n\" >> apps/workspace/src/app.ts"

echo "architecture check self-test ok: the clean tree passes and $passed planted violations fail"
