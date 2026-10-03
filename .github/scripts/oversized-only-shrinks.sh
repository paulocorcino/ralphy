#!/usr/bin/env bash
# Fails when a number in the OVERSIZED table grows, or an entry appears,
# between the base and HEAD. A human allows it with the `oversized-ok` label.
# Usage: oversized-only-shrinks.sh <base-rev>; reads $LABELS (comma-separated).
set -euo pipefail
export LC_ALL=C

base="$1"
table=crates/xtask/tests/ratchets.rs

if printf '%s' "${LABELS:-}" | tr ',' '\n' | grep -qx 'oversized-ok'; then
  echo "labelled oversized-ok: a human took this call"
  exit 0
fi

# rustfmt writes one `("path", 123),` entry per line. Only OVERSIZED has
# two-element entries; the spawn table's entries have three.
entries() {
  sed -nE 's/^[[:space:]]*\("([^"]+)", ([0-9]+)\),[[:space:]]*$/\1 \2/p' | sort
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
if git cat-file -e "$base:$table" 2>/dev/null; then
  git show "$base:$table" | entries > "$tmp/base"
else
  : > "$tmp/base"
fi
entries < "$table" > "$tmp/head"

grown="$(join -a 2 -e 0 -o '0,1.2,2.2' "$tmp/base" "$tmp/head" \
  | awk '$3 > $2 { print "  " $1 ": " $2 " -> " $3 }')"

if [ -n "$grown" ]; then
  echo "::error::an OVERSIZED number grew; the file was not split."
  echo ""
  printf '%s\n' "$grown"
  echo ""
  echo "Split the file instead: the ratchet's failure message says how."
  echo "If it has no clean seam, the // reason next to the entry names the"
  echo "responsibilities checked, and a human applies the oversized-ok label."
  exit 1
fi
echo "no OVERSIZED number grew"
