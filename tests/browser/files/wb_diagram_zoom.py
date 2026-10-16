"""Browser acceptance: a diagram or a large image in a markdown file can be enlarged.

One Playwright pass over a REAL daemon. A markdown file holds a Mermaid fence, a
large image and a small icon. The check proves that the Enlarge button belongs
to the diagram and the large image only, that it opens a view that fills the
window, that the wheel zooms, that a drag moves the picture, and that Escape
closes the view and puts the focus back on the button. It also opens the same
file in the detached file window.

Scenario 1  the Mermaid holder and the large image each get an Enlarge button;
            the icon does not; the button of the diagram appears once it is drawn
Scenario 2  the button is hidden until the pointer is over the picture
Scenario 3  pressing it opens `.zoom-overlay` over the whole window with the
            picture fitted inside it
Scenario 4  the wheel zooms: the stage grows, and the page behind does not scroll
Scenario 5  a drag moves the zoomed picture
Scenario 6  Escape closes the view and the focus returns to the button
Scenario 7  an image opens at its own size, never above it
Scenario 8  the detached file window opens the same view

Requires `playwright` and `pillow` (`pip install playwright pillow`, then
`playwright install chromium`).

Boots a Localhost daemon over a SCRATCH `RALPHY_DAEMON_DIR`. The daemon is
stopped by its own subprocess handle, NEVER by name.

Writes .ralphy/screenshots/diagram-zoom-*.png.
Run: python tests/browser/files/wb_diagram_zoom.py   (exit 0 = all pass)
"""

import os
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
import zlib
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7580
BASE = f"http://127.0.0.1:{PORT}/"

# tests/browser/files/wb_diagram_zoom.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
FILES = "Alpine.$data(document.querySelector('[x-data=\"wbFiles\"]'))"

BIG_W, BIG_H = 800, 400
DOT_W, DOT_H = 24, 24

DOC = """# Architecture

```mermaid
graph LR
  A[Client] --> B[Daemon]
  B --> C[Agent]
```

![big](big.png)

![dot](dot.png)
"""

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def png_bytes(width, height, rgb=(232, 217, 168)):
    """A real, decodable RGB PNG, hand-encoded so `naturalWidth` is an independent oracle."""

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


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


def empty_env(daemon_dir):
    """A scratch registry and empty vendor stores: the operator's own daemon is never touched."""
    empty = tempfile.mkdtemp(prefix="wbzoom_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wbzoom_repo_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "arch.md").write_text(DOC, encoding="utf-8")
    (p / "big.png").write_bytes(png_bytes(BIG_W, BIG_H))
    (p / "dot.png").write_bytes(png_bytes(DOT_W, DOT_H, (111, 159, 192)))
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbzoom@example.com"],
        ["git", "config", "user.name", "wbzoom"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run([EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8")
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are embedded in the binary, so it is rebuilt after any assets/ui edit.
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def open_from_tree(page, title):
    page.wait_for_function(
        f"(t) => !!{FILES}._tree && !!{FILES}._tree.findFirst(n => n.title === t)", arg=title, timeout=20000
    )
    page.evaluate(f"(t) => {FILES}.openFile({FILES}._tree.findFirst(n => n.title === t))", arg=title)


# The article of the open markdown pane, once the diagram is drawn and the
# images have decoded (a decoded image is what gives its button).
READY = (
    "() => { const a = document.querySelector('.md-body');"
    " return !!a && !!a.querySelector('.mermaid svg')"
    " && [...a.querySelectorAll('img')].every(i => i.complete && i.naturalWidth > 0); }"
)

STAGE = (
    "() => { const s = document.querySelector('.zoom-stage'); if (!s) return null;"
    " const r = s.getBoundingClientRect();"
    " return { x: r.x, y: r.y, w: r.width, h: r.height }; }"
)


def stage_rect(page):
    return page.evaluate(STAGE)


def exercise(page, label):
    """Scenarios 1 to 7 on the page that shows the markdown pane."""
    page.wait_for_function(READY, timeout=30000)
    page.wait_for_function("() => document.querySelectorAll('.md-body .zoom-btn').length === 2", timeout=10000)
    counts = page.evaluate(
        """() => {
          const a = document.querySelector('.md-body');
          const boxes = [...a.querySelectorAll('.zoomable')];
          return {
            buttons: a.querySelectorAll('.zoom-btn').length,
            mermaid: !!a.querySelector('.zoomable-block .mermaid svg'),
            bigBoxed: boxes.some(b => b.querySelector('img') && b.querySelector('img').naturalWidth === %d),
            dotBoxed: boxes.some(b => b.querySelector('img') && b.querySelector('img').naturalWidth === %d),
          };
        }"""
        % (BIG_W, DOT_W)
    )
    check(
        f"[{label}] the diagram and the large image have a button, the icon has none",
        counts["buttons"] == 2 and counts["mermaid"] and counts["bigBoxed"] and not counts["dotBoxed"],
        f"got={counts}",
    )

    diagram_btn = page.locator(".md-body .zoomable-block > .zoom-btn")
    page.mouse.move(2, 2)
    check(f"[{label}] the button is hidden while the pointer is elsewhere", float(diagram_btn.evaluate("e => getComputedStyle(e).opacity")) == 0)
    page.locator(".md-body .zoomable-block").hover()
    check(f"[{label}] the button shows when the pointer is over the diagram", float(diagram_btn.evaluate("e => getComputedStyle(e).opacity")) == 1)

    diagram_btn.click()
    page.wait_for_selector(".zoom-overlay", timeout=5000)
    vw, vh = page.evaluate("() => [innerWidth, innerHeight]")
    overlay = page.evaluate("() => { const r = document.querySelector('.zoom-overlay').getBoundingClientRect(); return [r.width, r.height]; }")
    check(f"[{label}] the view fills the window", overlay == [vw, vh], f"overlay={overlay} window={[vw, vh]}")
    fitted = stage_rect(page)
    inside = fitted["x"] >= -1 and fitted["y"] >= -1 and fitted["x"] + fitted["w"] <= vw + 1 and fitted["y"] + fitted["h"] <= vh + 1
    check(f"[{label}] the diagram is fitted inside the window", inside, f"stage={fitted}")
    page.screenshot(path=os.path.join(SHOT_DIR, f"diagram-zoom-{label}-fit.png"))

    scroll_before = page.evaluate("() => document.querySelector('.md-body').closest('.md-split, .viewer-body, body').scrollTop")
    page.mouse.move(vw / 2, vh / 2)
    for _ in range(4):
        page.mouse.wheel(0, -300)
    page.wait_for_timeout(150)
    zoomed = stage_rect(page)
    check(f"[{label}] the wheel zooms the diagram", zoomed["w"] > fitted["w"] * 1.5, f"{fitted['w']:.0f} -> {zoomed['w']:.0f}")
    scroll_after = page.evaluate("() => document.querySelector('.md-body').closest('.md-split, .viewer-body, body').scrollTop")
    check(f"[{label}] the page behind does not scroll", scroll_before == scroll_after, f"{scroll_before} -> {scroll_after}")
    page.screenshot(path=os.path.join(SHOT_DIR, f"diagram-zoom-{label}-zoomed.png"))

    page.mouse.move(vw / 2, vh / 2)
    page.mouse.down()
    page.mouse.move(vw / 2 + 120, vh / 2 + 10, steps=6)
    page.mouse.up()
    page.wait_for_timeout(100)
    dragged = stage_rect(page)
    check(f"[{label}] a drag moves the zoomed diagram", abs(dragged["x"] - zoomed["x"]) > 40, f"x {zoomed['x']:.0f} -> {dragged['x']:.0f}")
    check(f"[{label}] a drag does not close the view", page.locator(".zoom-overlay").count() == 1)

    page.keyboard.press("Escape")
    page.wait_for_function("() => !document.querySelector('.zoom-overlay')", timeout=3000)
    focused = page.evaluate("() => document.activeElement && document.activeElement.className")
    check(f"[{label}] Escape closes the view and the focus returns to the button", focused == "zoom-btn", f"active={focused!r}")

    # An image opens at its own size, never above it (a raster has no more pixels).
    page.locator(".md-body .zoomable:not(.zoomable-block)").hover()
    page.locator(".md-body .zoomable:not(.zoomable-block) > .zoom-btn").click()
    page.wait_for_selector(".zoom-overlay", timeout=5000)
    image = stage_rect(page)
    check(f"[{label}] a large image opens at its own size", abs(image["w"] - BIG_W) < 2, f"w={image['w']:.0f}")
    page.keyboard.press("Escape")
    page.wait_for_function("() => !document.querySelector('.zoom-overlay')", timeout=3000)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbzoom_reg_")
    fixture = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1500, "height": 950})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function("() => Alpine.store('projects').projects.length === 1", timeout=15000)
            page.evaluate(f"(s) => {{ if (Alpine.store('projects').openSlug !== s) {SH}.toggle(s); }}", arg=slug)
            open_from_tree(page, "arch.md")
            exercise(page, "attached")

            # --- scenario 8: the detached file window ---------------------------
            # The pane's own Detach button opens the real popup, which asks this
            # page for the file; a bare load of the popup page has no opener.
            with ctx.expect_page() as popup:
                page.locator('.viewer:has(.md-body) [data-act="detach"]').first.click()
            detached = popup.value
            detached.on("pageerror", lambda e: print("detached pageerror:", e, flush=True))
            detached.wait_for_function("() => !!document.querySelector('.md-body .mermaid svg')", timeout=30000)
            detached.wait_for_selector(".md-body .zoomable-block > .zoom-btn", timeout=10000)
            detached.locator(".md-body .zoomable-block").hover()
            detached.locator(".md-body .zoomable-block > .zoom-btn").click()
            detached.wait_for_selector(".zoom-overlay", timeout=5000)
            check("[detached] the same button opens the same view", detached.locator(".zoom-overlay .zoom-stage svg").count() == 1)
            detached.screenshot(path=os.path.join(SHOT_DIR, "diagram-zoom-detached.png"))
            detached.keyboard.press("Escape")
            detached.wait_for_function("() => !document.querySelector('.zoom-overlay')", timeout=3000)
            check("[detached] Escape closes it", detached.locator(".zoom-overlay").count() == 0)
            browser.close()
    finally:
        stop(proc)

    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
