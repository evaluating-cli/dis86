#!/usr/bin/env bash
set -euo pipefail

SST_COMMIT=37c73caf53dcd22d3dd369ff09305d13d117a4fe
RAW_ROOT="https://raw.githubusercontent.com/SingleStepTests/80286/${SST_COMMIT}"
RAW_BASE="${RAW_ROOT}/v1_real_mode"

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
manifest="$repo_root/dis86/data/sst/manifest.txt"

usage() {
  echo "usage: $0 <target-dir> [NAME ...]" >&2
  echo "  no NAME args fetches every entry in $manifest" >&2
  echo "  entries: <NAME> <SHA256-OF-DECOMPRESSED-MOO>  (# comments allowed)" >&2
  exit 2
}

[[ $# -ge 1 ]] || usage
target_dir=$1
shift
mkdir -p "$target_dir"

if [[ ! -r "$manifest" ]]; then
  echo "error: manifest not readable: $manifest" >&2
  exit 1
fi

mapfile -t lines < "$manifest"
fetch_one() {
  local name=$1 want=$2
  local out="$target_dir/$name.MOO"
  if [[ -f "$out" ]]; then
    local got
    got=$(sha256sum "$out" | awk '{print $1}')
    if [[ "$got" == "$want" ]]; then
      echo "skip: $name.MOO already present and hash matches"
      return 0
    fi
    echo "re-fetch: $name.MOO present but hash mismatch" >&2
  fi
  local tmp
  tmp=$(mktemp)
  if ! curl -fsSL "${RAW_BASE}/${name}.MOO.gz" -o "$tmp"; then
    rm -f "$tmp"
    return 1
  fi
  if ! gzip -dc "$tmp" > "$out"; then
    rm -f "$tmp" "$out"
    return 1
  fi
  rm -f "$tmp"
  local got
  got=$(sha256sum "$out" | awk '{print $1}')
  if [[ "$got" != "$want" ]]; then
    echo "error: $name.MOO sha256 mismatch" >&2
    echo "  expected: $want" >&2
    echo "  actual:   $got" >&2
    rm -f "$out"
    return 1
  fi
  echo "fetched: $name.MOO ($(stat -c%s "$out") bytes, sha256 ok)"
}

fetch_auxiliary() {
  # These files live at the pinned repository root, not under v1_real_mode.
  # The commit pin makes their contents immutable and keeps the runner's
  # revocation input tied to exactly the same hardware corpus revision.
  local name tmp out
  for name in revocation_list.txt CHANGELOG.md; do
    out="$target_dir/$name"
    tmp=$(mktemp)
    if ! curl -fsSL "${RAW_ROOT}/${name}" -o "$tmp"; then
      rm -f "$tmp"
      return 1
    fi
    mv "$tmp" "$out"
    echo "fetched: $name ($(stat -c%s "$out") bytes, pinned commit)"
  done
}

if [[ $# -eq 0 ]]; then
  while IFS= read -r line; do
    line=${line%%#*}
    [[ -z "${line//[[:space:]]/}" ]] && continue
    read -r name want _ <<< "$line"
    [[ -z "$name" || -z "$want" ]] && continue
    fetch_one "$name" "$want"
  done < <(printf '%s\n' "${lines[@]}")
else
  declare -A wantmap
  while IFS= read -r line; do
    line=${line%%#*}
    [[ -z "${line//[[:space:]]/}" ]] && continue
    read -r name want _ <<< "$line"
    [[ -z "$name" || -z "$want" ]] && continue
    wantmap[$name]=$want
  done < <(printf '%s\n' "${lines[@]}")
  for name in "$@"; do
    want=${wantmap[$name]:-}
    if [[ -z "$want" ]]; then
      echo "error: no manifest entry for '$name'" >&2
      exit 1
    fi
    fetch_one "$name" "$want"
  done
fi

fetch_auxiliary
