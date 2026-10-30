#!/usr/bin/env bash
# Downloads the typos release binary for Linux x64 into the current
# directory and checks it against TYPOS_SHA256. TYPOS_VERSION and
# TYPOS_SHA256 come from the workflow env of ci.yml.
set -euo pipefail
curl -sSLf -o typos.tar.gz "https://github.com/crate-ci/typos/releases/download/v${TYPOS_VERSION}/typos-v${TYPOS_VERSION}-x86_64-unknown-linux-musl.tar.gz"
echo "${TYPOS_SHA256}  typos.tar.gz" | sha256sum --check
tar xzf typos.tar.gz ./typos
