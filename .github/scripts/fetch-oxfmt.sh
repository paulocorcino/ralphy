#!/usr/bin/env bash
# Downloads the oxfmt release binary for Linux x64 into the current
# directory and checks it against OXFMT_SHA256. OXFMT_VERSION and
# OXFMT_SHA256 come from the workflow env of ci.yml.
set -euo pipefail
curl -sSLf -o oxfmt.tar.gz "https://github.com/oxc-project/oxc/releases/download/oxfmt_v${OXFMT_VERSION}/oxfmt-x86_64-unknown-linux-gnu.tar.gz"
echo "${OXFMT_SHA256}  oxfmt.tar.gz" | sha256sum --check
tar xzf oxfmt.tar.gz
