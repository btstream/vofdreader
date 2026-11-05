#!/usr/bin/env bash
# Render every page of every OFD under a directory and report failures.
# Usage: scripts/render-corpus.sh [resources-dir] [output-dir]
set -euo pipefail

root="${1:-resources}"
out="${2:-/tmp/ofd-render-corpus}"
exe="./target/release/ofd-cli"

cargo build --release -p ofd-cli
rm -rf "$out"
mkdir -p "$out"

files=0
pages=0
failures=0
while IFS= read -r -d '' file; do
    files=$((files + 1))
    key="${file#"$root"/}"
    key="${key//\//__}"
    dir="$out/$key"
    mkdir -p "$dir"
    count=$("$exe" pages "$file" | grep -c '^#' || true)
    for ((page = 0; page < count; page++)); do
        pages=$((pages + 1))
        if ! "$exe" render "$file" --page "$page" --png "$dir/p$page.png" --dpi 96 >/dev/null; then
            echo "FAIL $file page $page" >&2
            failures=$((failures + 1))
        fi
    done
done < <(find "$root" -type f -name '*.ofd' -print0 | sort -z)

echo "rendered $pages pages from $files files into $out"
if ((failures > 0)); then
    echo "$failures page(s) failed" >&2
    exit 1
fi
