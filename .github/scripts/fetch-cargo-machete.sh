#!/usr/bin/env bash
# Downloads the cargo-machete release binary for Linux x64 into the current
# directory and checks it against MACHETE_SHA256. MACHETE_VERSION and
# MACHETE_SHA256 come from the workflow env of ci.yml. The binary lands in the
# current directory, so that `cargo machete` finds it when the directory is on
# PATH.
set -euo pipefail
dir="cargo-machete-v${MACHETE_VERSION}-x86_64-unknown-linux-musl"
curl -sSLf -o cargo-machete.tar.gz "https://github.com/bnjbvr/cargo-machete/releases/download/v${MACHETE_VERSION}/${dir}.tar.gz"
echo "${MACHETE_SHA256}  cargo-machete.tar.gz" | sha256sum --check
tar xzf cargo-machete.tar.gz --strip-components=1 "${dir}/cargo-machete"
