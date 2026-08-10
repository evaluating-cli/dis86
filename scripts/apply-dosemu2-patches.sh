#!/usr/bin/env bash
set -euo pipefail

expected_base=604ce0cdd1a71f657e2a2df623d216d5ab289313
repo=${1:-}

if [[ -z "$repo" ]]; then
  echo "usage: $0 /path/to/dosemu2" >&2
  exit 2
fi

if ! git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "not a git checkout: $repo" >&2
  exit 2
fi

actual=$(git -C "$repo" rev-parse HEAD)
if [[ "$actual" != "$expected_base" ]]; then
  echo "dosemu2 HEAD mismatch" >&2
  echo "  expected: $expected_base" >&2
  echo "  actual:   $actual" >&2
  exit 1
fi

if ! git -C "$repo" diff --quiet || ! git -C "$repo" diff --cached --quiet; then
  echo "dosemu2 checkout must be clean before applying the series" >&2
  exit 1
fi

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
series_dir="$root/patches/dosemu2"

while IFS= read -r patch; do
  [[ -z "$patch" || "$patch" == \#* ]] && continue
  echo "Applying $patch"
  git -C "$repo" am "$series_dir/$patch"
done < "$series_dir/series"
