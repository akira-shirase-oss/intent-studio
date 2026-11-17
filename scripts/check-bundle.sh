#!/usr/bin/env bash
# The workspace's bundle budget (D-063), checked on the built app (run after the workspace build
# in check.sh). Sizes are gzip -9 bytes of everything the browser can download from
# build/_app/immutable: every route's code counts, though a route loads only its own chunks.
#
#   all JavaScript  <= 160 KiB      (measured 115 KiB when the budget was set)
#   largest chunk   <=  48 KiB      (measured  32 KiB)
#   all CSS         <=  48 KiB      (measured  32 KiB)
#
# Fonts are excluded: they load only for the weights a page uses, and the UI kit owns them.
set -euo pipefail
cd "$(dirname "$0")/../apps/workspace/build"

budget_js=$((160 * 1024))
budget_chunk=$((48 * 1024))
budget_css=$((48 * 1024))

js=0
largest=0
largest_file=""
while IFS= read -r -d '' file; do
    size=$(gzip -9 -c "$file" | wc -c | tr -d ' ')
    js=$((js + size))
    if [ "$size" -gt "$largest" ]; then largest=$size; largest_file=$file; fi
done < <(find _app/immutable -name '*.js' -print0)
css=0
while IFS= read -r -d '' file; do
    css=$((css + $(gzip -9 -c "$file" | wc -c | tr -d ' ')))
done < <(find _app/immutable -name '*.css' -print0)

kib() { awk -v b="$1" 'BEGIN { printf "%.1f KiB", b / 1024 }'; }
echo "bundle: JavaScript $(kib "$js") (budget $(kib "$budget_js")), largest chunk $(kib "$largest") ($largest_file, budget $(kib "$budget_chunk")), CSS $(kib "$css") (budget $(kib "$budget_css"))"
status=0
[ "$js" -le "$budget_js" ] || { echo "bundle: JavaScript over budget" >&2; status=1; }
[ "$largest" -le "$budget_chunk" ] || { echo "bundle: a chunk is over budget" >&2; status=1; }
[ "$css" -le "$budget_css" ] || { echo "bundle: CSS over budget" >&2; status=1; }
exit "$status"
