#!/bin/sh
# Install Ralphy for this user on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/paulocorcino/ralphy/main/assets/install/install.sh | sh
#
# It downloads the newest release for this computer, checks its SHA-256,
# runs `ralphy install`, and ends with `ralphy doctor`. Nothing needs root.
#
# Settings, all optional:
#   RALPHY_VERSION=v0.1.0-rc.40   install this release instead of the newest
#   RALPHY_DAEMON=1               also start the daemon at sign-in
#   RALPHY_ALLOW_ROOT=1           install as root with no question
#   RALPHY_INSTALL_DIR=<folder>   put ralphy in this folder (`ralphy install --dir`)
#   RALPHY_RELEASES_URL           where the release list is read (for tests)
#   RALPHY_DOWNLOAD_URL           where the archives are read (for tests)

set -eu

REPO="paulocorcino/ralphy"
RELEASES_URL="${RALPHY_RELEASES_URL:-https://api.github.com/repos/$REPO/releases}"
DOWNLOAD_URL="${RALPHY_DOWNLOAD_URL:-https://github.com/$REPO/releases/download}"
# HTTPS only, except when a test points the script at a local fixture.
PROTO="=https"
if [ -n "${RALPHY_RELEASES_URL:-}${RALPHY_DOWNLOAD_URL:-}" ]; then
    PROTO="=https,http"
fi

say() {
    printf '%s\n' "$*"
}

fail() {
    printf 'ralphy install: %s\n' "$*" >&2
    exit 1
}

# Write the body of $1 to $2, with curl or wget.
fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --proto "$PROTO" --proto-redir "$PROTO" -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        if [ "$PROTO" = "=https" ]; then
            wget -q --https-only -O "$2" "$1"
        else
            wget -q -O "$2" "$1"
        fi
    else
        fail "this needs curl or wget to download Ralphy"
    fi
}

# Root is allowed, never wanted: everything Ralphy writes belongs to one user.
check_root() {
    [ "$(id -u)" = 0 ] || return 0
    say "warning: this runs as root. Ralphy does not need root: it installs for one user,"
    say "and everything it writes would belong to root."
    [ "${RALPHY_ALLOW_ROOT:-}" = 1 ] && return 0
    if ! (: </dev/tty) 2>/dev/null; then
        fail "there is no terminal to ask in. Run it as a normal user, or set RALPHY_ALLOW_ROOT=1"
    fi
    printf 'Continue anyway? [y/N] ' >/dev/tty
    read -r answer </dev/tty || answer=""
    case "$answer" in
        y | Y | yes) ;;
        *) fail "stopped. Run it again as a normal user" ;;
    esac
}

detect_target() {
    case "$(uname -s)" in
        Linux) os=linux ;;
        Darwin) os=macos ;;
        *) fail "there is no Ralphy build for $(uname -s). On Windows, use install.ps1" ;;
    esac
    case "$(uname -m)" in
        x86_64 | amd64) arch=x64 ;;
        aarch64 | arm64) arch=arm64 ;;
        *) arch="$(uname -m)" ;;
    esac
    case "$os-$arch" in
        linux-x64 | linux-arm64 | macos-x64 | macos-arm64) printf '%s' "$os-$arch" ;;
        *) fail "there is no Ralphy build for $os-$arch: build it with cargo (see docs/BUILDING.md)" ;;
    esac
}

# The newest release on the rc channel, which admits every release. The list
# comes newest first, and a draft is not visible without a token. A limit: the
# list is in the order the releases were made, not in version order as
# `ralphy update` reads it; the two differ only for a release made out of turn.
pick_tag() {
    if [ -n "${RALPHY_VERSION:-}" ]; then
        printf '%s' "$RALPHY_VERSION"
        return
    fi
    fetch "$RELEASES_URL?per_page=10" "$tmp/releases.json" ||
        fail "could not read the release list from $RELEASES_URL"
    tag="$(grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' "$tmp/releases.json" |
        head -n 1 | sed 's/.*"\([^"]*\)"$/\1/')"
    [ -n "$tag" ] || fail "the release list from $RELEASES_URL names no release"
    printf '%s' "$tag"
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

# Where `ralphy install` puts the binary. The binary answers, so the default
# folder is decided in one place.
installed_binary() {
    if [ -n "${RALPHY_INSTALL_DIR:-}" ]; then
        "$1" install --print-path --dir "$RALPHY_INSTALL_DIR"
    else
        "$1" install --print-path
    fi
}

# Lingering keeps the daemon running with no one signed in. A desktop session
# does not need it.
enable_linger() {
    [ "$(uname -s)" = Linux ] || return 0
    [ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ] || return 0
    if loginctl enable-linger 2>/dev/null; then
        say "Lingering is on: the daemon runs with no one signed in."
    else
        say "Could not turn on lingering. Run: sudo loginctl enable-linger $(id -un)"
    fi
}

main() {
    check_root
    target="$(detect_target)"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    trap 'rm -rf "$tmp"; exit 1' INT TERM
    tag="$(pick_tag)"
    name="ralphy-$tag-$target"
    archive="$name.tar.gz"

    say "Downloading Ralphy $tag for $target..."
    fetch "$DOWNLOAD_URL/$tag/$archive" "$tmp/$archive" ||
        fail "could not download $archive"
    fetch "$DOWNLOAD_URL/$tag/$archive.sha256" "$tmp/$archive.sha256" ||
        fail "could not download $archive.sha256"
    want="$(cut -d ' ' -f 1 <"$tmp/$archive.sha256")"
    got="$(sha256_of "$tmp/$archive")"
    [ "$want" = "$got" ] || fail "the SHA-256 of $archive is wrong, so nothing was installed"

    tar -xzf "$tmp/$archive" -C "$tmp"
    unpacked="$tmp/$name/ralphy"
    ralphy="$(installed_binary "$unpacked")" ||
        fail "cannot run programs from $tmp (is it mounted noexec?): set TMPDIR to another folder"
    # A copy, not a link: the download folder is removed when this script ends.
    # `--force` lets a second run repair an install.
    "$unpacked" install --copy --force --dir "$(dirname "$ralphy")"

    if [ "${RALPHY_DAEMON:-}" = 1 ]; then
        "$ralphy" daemon install
        enable_linger
    else
        say "To start the daemon at sign-in, run: ralphy daemon install"
    fi

    say ""
    # `doctor` exits 1 when a tool is missing; that never stops the install.
    # Any other code means the installed binary did not run.
    status=0
    "$ralphy" doctor || status=$?
    [ "$status" -le 1 ] || fail "could not run $ralphy (exit $status)"
}

main "$@"
