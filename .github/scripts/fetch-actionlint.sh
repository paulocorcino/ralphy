#!/usr/bin/env bash
# Downloads the actionlint release binary for Linux x64 into the current
# directory and checks it against ACTIONLINT_SHA256. ACTIONLINT_VERSION and
# ACTIONLINT_SHA256 come from the workflow env of ci.yml.
set -euo pipefail
curl -sSLf -o actionlint.tar.gz "https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/actionlint_${ACTIONLINT_VERSION}_linux_amd64.tar.gz"
echo "${ACTIONLINT_SHA256}  actionlint.tar.gz" | sha256sum --check
tar xzf actionlint.tar.gz actionlint
