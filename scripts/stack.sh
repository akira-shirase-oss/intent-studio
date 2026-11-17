#!/usr/bin/env bash
# The local stack (deploy/compose.yaml, compose project `intent-studio`, Postgres on
# 127.0.0.1:59432).
#   up      start it and wait until Postgres is healthy
#   down    stop it, keeping the data
#   reset   stop it and delete the data
#   status  show the services
set -euo pipefail
cd "$(dirname "$0")/.."
compose=(docker compose -p intent-studio -f deploy/compose.yaml)
case "${1:-}" in
    up) "${compose[@]}" up -d --wait ;;
    down) "${compose[@]}" stop ;;
    reset) "${compose[@]}" down -v ;;
    status) "${compose[@]}" ps ;;
    *) echo "usage: scripts/stack.sh up|down|reset|status" >&2; exit 2 ;;
esac
