#!/usr/bin/env bash
# Downloads the shellcheck release binary for Linux x64 into the current
# directory and checks it against SHELLCHECK_SHA256. SHELLCHECK_VERSION and
# SHELLCHECK_SHA256 come from the workflow env of ci.yml.
set -euo pipefail
curl -sSLf -o shellcheck.tar.xz "https://github.com/koalaman/shellcheck/releases/download/v${SHELLCHECK_VERSION}/shellcheck-v${SHELLCHECK_VERSION}.linux.x86_64.tar.xz"
echo "${SHELLCHECK_SHA256}  shellcheck.tar.xz" | sha256sum --check
tar xJf shellcheck.tar.xz --strip-components=1 "shellcheck-v${SHELLCHECK_VERSION}/shellcheck"
