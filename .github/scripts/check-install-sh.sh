#!/usr/bin/env bash
# Runs assets/install/install.sh against a fake release packed from
# target/debug/ralphy and served on loopback (ADR-0056 §12). Run by release.yml
# on Linux and macOS. The workflow sets RALPHY_RELEASES_URL,
# RALPHY_DOWNLOAD_URL (port 8765) and RALPHY_INSTALL_DIR. The server runs in
# the same step as the script: a background process does not reliably outlive
# its step. Each refusal is checked by its message, so an unrelated failure
# does not pass for it.
set -eux
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=linux-x64 ;;
  Darwin-arm64) target=macos-arm64 ;;
  Darwin-x86_64) target=macos-x64 ;;
  *) echo "no release target for $(uname -s)-$(uname -m)"; exit 1 ;;
esac
python3 .github/scripts/install-fixture.py target/debug/ralphy "$target" "$RUNNER_TEMP/release"
python3 -m http.server 8765 --bind 127.0.0.1 --directory "$RUNNER_TEMP/release" >/dev/null 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null' EXIT
for _ in $(seq 1 50); do
  curl -fsS -o /dev/null "$RALPHY_RELEASES_URL" && break
  sleep 0.2
done
sh assets/install/install.sh
"$RALPHY_INSTALL_DIR/ralphy" --version
sh assets/install/install.sh   # a second run repairs the install
# The default folder, decided by the binary: a scratch home has no
# ~/.cargo/bin, so it is ~/.local/bin. Each ( ) group sets its own HOME
# on purpose, so the next one starts from the real one.
# shellcheck disable=SC2030,SC2031
( export HOME="$RUNNER_TEMP/home"; mkdir -p "$HOME"; unset RALPHY_INSTALL_DIR
  sh assets/install/install.sh
  "$HOME/.local/bin/ralphy" --version )
if [ "$(uname -s)" = Darwin ]; then
  # The daemon at sign-in: a launchd agent in a scratch home.
  # shellcheck disable=SC2030,SC2031
  ( export HOME="$RUNNER_TEMP/daemon-home"; mkdir -p "$HOME"
    RALPHY_DAEMON=1 sh assets/install/install.sh
    test -f "$HOME/Library/LaunchAgents/dev.ralphy.daemon.plist"
    "$RALPHY_INSTALL_DIR/ralphy" daemon uninstall )
fi
if [ "$(uname -s)" = Linux ]; then
  # As root with no terminal, it stops unless root is allowed.
  if sudo -E setsid -w sh assets/install/install.sh </dev/null 2>"$RUNNER_TEMP/root.err"; then
    echo "install.sh ran as root with no terminal and no RALPHY_ALLOW_ROOT"
    exit 1
  fi
  grep -q "no terminal to ask in" "$RUNNER_TEMP/root.err"
  sudo -E env RALPHY_ALLOW_ROOT=1 RALPHY_INSTALL_DIR="$RUNNER_TEMP/rootbin" \
    setsid -w sh assets/install/install.sh </dev/null
  "$RUNNER_TEMP/rootbin/ralphy" --version
fi
# A wrong checksum installs nothing.
sum="$RUNNER_TEMP/release/download/v0.0.0-fixture/ralphy-v0.0.0-fixture-$target.tar.gz.sha256"
echo "0000  x" > "$sum"
if RALPHY_INSTALL_DIR="$RUNNER_TEMP/badbin" sh assets/install/install.sh 2>"$RUNNER_TEMP/sum.err"; then
  echo "install.sh accepted a wrong checksum"
  exit 1
fi
grep -q "SHA-256 of" "$RUNNER_TEMP/sum.err"
test ! -e "$RUNNER_TEMP/badbin/ralphy"
