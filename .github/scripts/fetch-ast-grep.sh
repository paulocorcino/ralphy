#!/usr/bin/env bash
# Downloads the ast-grep release binary for Linux x64 into the current
# directory and checks it against ASTGREP_SHA256. ASTGREP_VERSION and
# ASTGREP_SHA256 come from the workflow env of ci.yml.
set -euo pipefail
curl -sSLf -o ast-grep.zip "https://github.com/ast-grep/ast-grep/releases/download/${ASTGREP_VERSION}/app-x86_64-unknown-linux-gnu.zip"
echo "${ASTGREP_SHA256}  ast-grep.zip" | sha256sum --check
# The archive holds the binary twice; `sg` is taken, because the rules sit in
# a folder named `ast-grep` at the root.
unzip -o -q ast-grep.zip sg
