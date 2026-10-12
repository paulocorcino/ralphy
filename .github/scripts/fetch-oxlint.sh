#!/usr/bin/env bash
# Downloads the oxlint release binary for Linux x64 into the current
# directory and checks it against OXLINT_SHA256. OXLINT_VERSION and
# OXLINT_SHA256 come from the workflow env of ci.yml.
set -euo pipefail
curl -sSLf -o oxlint.tar.gz "https://github.com/oxc-project/oxc/releases/download/oxlint_v${OXLINT_VERSION}/oxlint-x86_64-unknown-linux-gnu.tar.gz"
echo "${OXLINT_SHA256}  oxlint.tar.gz" | sha256sum --check
tar xzf oxlint.tar.gz
