#!/usr/bin/env bash
# Builds the release package the operator guide installs from:
#
#   <out>/intent-studio-<commit>.tar.gz
#     intent-studio/bin/intent-server, bin/intent-worker   (release builds)
#     intent-studio/workspace/                             (the built workspace app)
#     intent-studio/deploy/                                (systemd unit, env example, Caddyfile,
#                                                           the browser-jail AppArmor profile)
#     intent-studio/OPERATOR_GUIDE.md
#
# Usage: scripts/package.sh [out directory, default dist]. Prints the package's path.
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-dist}"
mkdir -p "$out"
cargo build --release --locked -p intent-server -p intent-worker >&2
[ -d node_modules ] || npm ci >&2
npm run build --workspace @intent-studio/workspace >&2
# The binaries just built, wherever CARGO_TARGET_DIR or a cargo config puts them (rule S1).
bin="$(cargo metadata --format-version 1 --no-deps --locked | jq -r .target_directory)/release"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
root="$stage/intent-studio"
mkdir -p "$root/bin" "$root/workspace" "$root/deploy"
cp "$bin/intent-server" "$bin/intent-worker" "$root/bin/"
# The binary the package ships carries no mutation seam — checked on the packaged copy,
# not the build tree's, so a bad copy cannot slip past the check (B1).
scripts/check-seams.sh "$root/bin/intent-server" >&2
cp -R apps/workspace/build/. "$root/workspace/"
cp deploy/intent-studio.service deploy/intent-studio.env.example deploy/Caddyfile "$root/deploy/"
mkdir -p "$root/deploy/apparmor.d"
cp deploy/apparmor.d/intent-jail "$root/deploy/apparmor.d/"
cp docs/OPERATOR_GUIDE.md "$root/"
package="$out/intent-studio-$(git rev-parse --short HEAD).tar.gz"
tar -C "$stage" -czf "$package" intent-studio
echo "$package"
