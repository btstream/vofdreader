#!/usr/bin/env bash
# Fail if any referenced embedded glyph is replaced. Does not modify samples.
# Usage: bash scripts/check-fonts.sh [resources-dir]
set -euo pipefail
root="${1:-resources}"
[[ -d "$root" ]] || { echo "not a directory: $root" >&2; exit 1; }
cargo build --release -p ofd-cli
files=0
failures=0
while IFS= read -r -d '' file; do
    files=$((files + 1))
    echo "$file"
    if ! ./target/release/ofd-cli fonts "$file" --strict; then
        failures=$((failures + 1))
    fi
done < <(find "$root" -type f -name '*.ofd' -print0 | sort -z)
echo "checked $files files; $failures failed"
(( files > 0 && failures == 0 ))
