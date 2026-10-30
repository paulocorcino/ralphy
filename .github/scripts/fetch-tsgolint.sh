#!/usr/bin/env bash
# Downloads the tsgolint binary for Linux x64 (the npm tarball of
# @oxlint-tsgolint/linux-x64) into the current directory and checks it
# against TSGOLINT_SHA256. TSGOLINT_VERSION and TSGOLINT_SHA256 come from
# the workflow env of ci.yml. oxlint finds the binary through
# OXLINT_TSGOLINT_PATH.
set -euo pipefail
curl -sSLf -o tsgolint.tgz "https://registry.npmjs.org/@oxlint-tsgolint/linux-x64/-/linux-x64-${TSGOLINT_VERSION}.tgz"
echo "${TSGOLINT_SHA256}  tsgolint.tgz" | sha256sum --check
tar xzf tsgolint.tgz --strip-components=1 package/tsgolint
