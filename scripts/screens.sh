#!/usr/bin/env bash
# Writes every workspace screen at 360 and 1440 px, in light and dark, to docs/screens/ (for
# review; committed when the screens change), by running the workspace journeys with SCREENS=1.
set -euo pipefail
cd "$(dirname "$0")/.."
rm -f docs/screens/*.png
SCREENS=1 scripts/workspace-e2e.sh
ls docs/screens
