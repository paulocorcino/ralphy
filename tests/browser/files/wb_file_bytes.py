"""Browser acceptance of the file bytes route (ADR-0091 D5), in three engines.

For each of Chromium, Firefox and WebKit, over a REAL daemon:

  1. an image tab draws its picture from the route: the live `<img>` decodes at
     the fixture's size and its `src` is the route address, not a data: URL;
  2. a page navigation to the route address gets 403, because the browser sends
     `Sec-Fetch-Dest: document` and the route serves only fetch and media.

The daemon runs over a SCRATCH `RALPHY_DAEMON_DIR` on a free port, so the
operator's own daemon is untouched. It is stopped by its own subprocess handle,
never by name.

Requires `playwright` (`pip install playwright`, then
`playwright install chromium firefox webkit`).
Run: python tests/browser/files/wb_file_bytes.py   (exit 0 = all pass)
"""

import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
import zlib
from pathlib import Path

from playwright.sync_api import Error as PlaywrightError, sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
FILES = "Alpine.$data(document.querySelector('[x-data=\"wbFiles\"]'))"
WIDTH, HEIGHT = 64, 48

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def png_bytes(width, height):
    """A decodable RGB PNG, hand-encoded so `naturalWidth` is an independent oracle."""

    def chunk(tag, data):
        body = tag + data
        crc = zlib.crc32(body) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + body + struct.pack(">I", crc)

    raw = b"".join(b"\x00" + bytes((232, 217, 168)) * width for _ in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wbbytes_repo_")
    (Path(d) / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (Path(d) / "logo.png").write_bytes(png_bytes(WIDTH, HEIGHT))
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbbytes@example.com"],
        ["git", "config", "user.name", "wbbytes"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    out = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    return out.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def wait_listening(base, timeout=25):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(base, timeout=1)
            return True
        except Exception:
            time.sleep(0.3)
    return False


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def launch(daemon_dir, port):
    empty = tempfile.mkdtemp(prefix="wbbytes_empty_")
    env = dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(port)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )


def navigation_status(page, url):
    """The status the browser's network layer got for a page navigation to `url`.
    Firefox shows its own error page for a bodyless 403 and `goto` raises
    NS_ERROR_NET_EMPTY_RESPONSE, so the status is read off the response event."""
    seen = []
    page.on("response", lambda r: seen.append(r.status) if r.url == url else None)
    try:
        page.goto(url)
    except PlaywrightError as e:
        print(f"  navigation error: {str(e).splitlines()[0]}", flush=True)
    return seen[-1] if seen else None


def check_engine(engine, base, slug):
    page = engine.launch(headless=True).new_context().new_page()
    page.goto(base)
    page.wait_for_selector("[x-data]", timeout=15000)
    page.wait_for_function("() => Alpine.store('projects').projects.length === 1", timeout=15000)
    page.evaluate(f"(s) => {{ if (Alpine.store('projects').openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(
        f"() => !!{FILES}.tree() && !!{FILES}.tree().findFirst(n => n.title === 'logo.png')", timeout=20000
    )
    page.evaluate(f"() => {FILES}.openFile({FILES}.tree().findFirst(n => n.title === 'logo.png'))")
    tab = f"file:{slug}:logo.png"
    page.wait_for_function(
        "(id) => { const img = document.querySelector("
        "`.image-viewer[data-tab-id=\"${id}\"] img.img-canvas`);"
        " return !!img && img.complete && img.naturalWidth > 0; }",
        arg=tab,
        timeout=20000,
    )
    got = page.evaluate(
        "(id) => { const img = document.querySelector("
        "`.image-viewer[data-tab-id=\"${id}\"] img.img-canvas`);"
        " return { src: img.getAttribute('src'), w: img.naturalWidth, h: img.naturalHeight }; }",
        tab,
    )
    name = engine.name
    check(
        f"{name}: the image tab draws the route's bytes at {WIDTH}x{HEIGHT}",
        (got["w"], got["h"]) == (WIDTH, HEIGHT) and got["src"].startswith("/api/file/bytes?"),
        f"got={got['src'][:40]!r} {got['w']}x{got['h']}",
    )
    url = base.rstrip("/") + got["src"]
    status = navigation_status(page, url)
    check(f"{name}: a page navigation to the route gets 403", status == 403, f"status={status}")
    page.context.browser.close()


def main():
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True) \
        if os.environ.get("RALPHY_TEST_SKIP_BUILD") != "1" else None
    port = free_port()
    base = f"http://127.0.0.1:{port}/"
    daemon_dir = tempfile.mkdtemp(prefix="wbbytes_reg_")
    slug = register_fixture(daemon_dir, make_fixture_repo())
    proc = launch(daemon_dir, port)
    try:
        if not wait_listening(base):
            check(f"daemon listening on {port}", False)
            sys.exit(1)
        with sync_playwright() as p:
            for engine in (p.chromium, p.firefox, p.webkit):
                check_engine(engine, base, slug)
    finally:
        stop(proc)
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if len(results) == 6 and all(results) else 1)


if __name__ == "__main__":
    main()
