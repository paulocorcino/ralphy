"""Browser acceptance: an HTML file opens as a rendered page, in a frame with
no rights (ADR-0072, amendment of 2026-11-15).

One Playwright pass per engine (Chromium, Firefox, WebKit) over a REAL
daemon. A fixture repo holds `docs/report.html` (a style sheet, a link to a
fragment, an image file next to it, a `data:` image) and `docs/hostile.html`
(a script, an `onerror`, a `meta refresh`, a form, a nested frame, and
images, a style sheet and links that name the daemon's own origin).

Scenario 1  `report.html` is drawn in a frame with an empty `sandbox`, with
            its style
Scenario 2  the image file next to it and the `data:` image are drawn
Scenario 3  a click on the fragment link scrolls the frame, and the frame
            keeps the document
Scenario 4  the pane shows no heading index and no Find button
Scenario 5  Edit shows the file text in the editor; Preview draws it again
Scenario 6  `hostile.html` runs no script, its links have no target, and
            none of its requests reaches the daemon; a request of the page
            itself does (the control)
Scenario 7  the detached file window draws the frame
Scenario 8  an image from the web is not asked for while remote images are
            off, and is drawn after the option is turned on and the page is
            loaded again

Requires `playwright` (`pip install playwright`, then `playwright install`).

Boots a Localhost daemon over a SCRATCH `RALPHY_DAEMON_DIR`. The daemon is
stopped by its own subprocess handle, NEVER by name.

Writes .ralphy/screenshots/html-file-*.png.
Run: python tests/browser/files/wb_html_file.py   (exit 0 = all pass)
"""

import base64
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

PORT = 7583
BASE = f"http://127.0.0.1:{PORT}/"

# tests/browser/files/wb_html_file.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
ENGINES = ("chromium", "firefox", "webkit")
CANARY = "canary-html-file"
PIC_WIDTH = 3
WEB_WIDTH = 7
WEB_IMAGE = "https://images.example/web.png"


def png(width, height):
    """A valid RGB PNG of one color: the daemon checks the leading bytes."""

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    rows = (b"\x00" + b"\x20\x60\xc0" * width) * height
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


PIC = png(PIC_WIDTH, 2)
INLINE = "data:image/png;base64," + base64.b64encode(png(5, 1)).decode()

REPORT = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Report</title>
<style>
  body {{ margin: 0; font: 15px sans-serif; }}
  h1 {{ color: rgb(1, 2, 3); }}
  .tall {{ height: 3000px; }}
</style></head><body>
<h1>The report</h1>
<p><a id="jump" href="#end">Go to the end</a></p>
<img id="pic" src="pic.png" alt="a file next to the report">
<img id="inline" src="{INLINE}" alt="an inline image">
<img id="web" src="{WEB_IMAGE}" alt="an image from the web">
<div class="tall"></div>
<h2 id="end">The end</h2>
</body></html>
"""

HOSTILE = f"""<!doctype html>
<html><head>
<meta http-equiv="refresh" content="1;url=/{CANARY}-refresh">
<link rel="stylesheet" href="/{CANARY}-css">
<style>body {{ background: url(/{CANARY}-bg); }}</style>
</head><body>
<p id="marker">clean</p>
<script>document.getElementById('marker').textContent = 'SCRIPT RAN';
fetch('/{CANARY}-script');</script>
<img src="/{CANARY}-img">
<img src="x" onerror="document.getElementById('marker').textContent = 'ONERROR RAN'">
<p><a id="self" href="/{CANARY}-nav">self</a>
<a id="top" href="/{CANARY}-top" target="_top">top</a>
<a id="blank" href="/{CANARY}-blank" target="_blank">blank</a></p>
<form action="/{CANARY}-form"><button id="send">send</button></form>
<iframe src="/{CANARY}-frame"></iframe>
</body></html>
"""

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


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
    empty = tempfile.mkdtemp(prefix="wbhtml_empty_")
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
    d = tempfile.mkdtemp(prefix="wbhtml_repo_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    docs = p / "docs"
    docs.mkdir()
    (docs / "report.html").write_text(REPORT, encoding="utf-8", newline="\n")
    (docs / "hostile.html").write_text(HOSTILE, encoding="utf-8", newline="\n")
    (docs / "pic.png").write_bytes(PIC)
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbhtml@example.com"],
        ["git", "config", "user.name", "wbhtml"],
        ["git", "config", "core.autocrlf", "false"],
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


def open_path(page, slug, path):
    """Open a repo file the way a link does: the shell classifies its name."""
    page.evaluate(
        "([s, p]) => Alpine.store('tabs').openLink({ project: s, path: p, checkout: null })",
        [slug, path],
    )


def pane(path):
    """The selector of the pane that shows `path` (its tab id ends with it)."""
    return f'.viewer[data-tab-id$=":{path}"]'


def frame_of(page, sel):
    """The frame of a pane, once its document is in it."""
    page.wait_for_function(
        "(s) => (document.querySelector(s + ' iframe.html-frame')?.srcdoc ?? '') !== ''", arg=sel, timeout=30000
    )
    frame = page.query_selector(sel + " iframe.html-frame").content_frame()
    frame.wait_for_selector("body", timeout=10000)
    return frame


def images_of(frame):
    frame.wait_for_function("() => [...document.images].every(i => i.complete)", timeout=10000)
    return frame.evaluate(
        "() => Object.fromEntries([...document.images].map(i => [i.id, [i.naturalWidth, i.src.slice(0, 15)]]))"
    )


def enter(page, slug):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.wait_for_function("() => Alpine.store('projects').projects.length === 1", timeout=15000)
    page.evaluate(
        "(s) => { if (Alpine.store('projects').openSlug !== s)"
        " Alpine.$data(document.querySelector('[x-data]')).toggle(s); }",
        arg=slug,
    )


def set_remote_images(page, on):
    return page.evaluate(
        """async (on) => (await fetch('/api/security/remote-images', {
          method: 'POST',
          headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
          body: 'enable=' + on,
        })).status""",
        "true" if on else "false",
    )


def scenario_remote(page, slug, engine, web_asked):
    """Runs last: the report was drawn with the option off."""
    off = len(web_asked)
    status = set_remote_images(page, True)
    try:
        page.reload()
        enter(page, slug)
        open_path(page, slug, "docs/report.html")
        pics = images_of(frame_of(page, pane("docs/report.html")))
        check(
            f"{engine}: an image from the web is asked for and drawn only with remote images on",
            off == 0 and status == 200 and len(web_asked) >= 1 and pics.get("web") == [WEB_WIDTH, "https://images."],
            f"asked while off={off} status={status} asked after={len(web_asked)} web={pics.get('web')}",
        )
    finally:
        set_remote_images(page, False)


def scenario_report(page, slug, engine):
    open_path(page, slug, "docs/report.html")
    sel = pane("docs/report.html")
    frame = frame_of(page, sel)
    sandbox = page.evaluate("(s) => document.querySelector(s + ' iframe.html-frame').getAttribute('sandbox')", sel)
    title = frame.locator("h1").text_content()
    color = frame.locator("h1").evaluate("e => getComputedStyle(e).color")
    check(
        f"{engine}: the file is drawn in a frame with an empty sandbox, with its style",
        sandbox == "" and title == "The report" and color == "rgb(1, 2, 3)",
        f"sandbox={sandbox!r} h1={title!r} color={color}",
    )

    pics = images_of(frame)
    check(
        f"{engine}: the image file and the inline image are drawn, the web image is not",
        pics == {
            "pic": [PIC_WIDTH, "data:image/png;"],
            "inline": [5, "data:image/png;"],
            "web": [0, "https://images."],
        },
        f"got={pics}",
    )
    page.screenshot(path=os.path.join(SHOT_DIR, f"html-file-{engine}.png"))

    before = frame.evaluate("scrollY")
    frame.locator("#jump").click()
    page.wait_for_timeout(700)
    frame = frame_of(page, sel)
    after = frame.evaluate("scrollY")
    still = frame.locator("h1").count()
    check(
        f"{engine}: a fragment link scrolls the frame, and the frame keeps the document",
        before == 0 and after > 1000 and still == 1 and frame.url.startswith("about:srcdoc"),
        f"scrollY {before} -> {after} h1={still} url={frame.url}",
    )

    hidden = page.evaluate(
        """(s) => {
          const p = document.querySelector(s);
          const shown = (q) => getComputedStyle(p.querySelector(q)).display;
          return [shown('nav.md-outline'), shown('.md-toc-btn'), shown('[data-act="find"]'),
                  shown('[data-act="toggle"]') !== 'none'];
        }""",
        sel,
    )
    check(
        f"{engine}: the pane shows no heading index and no Find button",
        hidden == ["none", "none", "none", True],
        f"got={hidden}",
    )

    page.locator(f'{sel} [data-act="toggle"]').click()
    page.wait_for_function(
        "(src) => !!window.monaco && monaco.editor.getModels().some(m => m.getValue() === src)",
        arg=REPORT,
        timeout=30000,
    )
    editor = page.evaluate("(s) => getComputedStyle(document.querySelector(s + ' .md-editor')).display", sel)
    page.locator(f'{sel} [data-act="toggle"]').click()
    again = frame_of(page, sel).locator("h1").text_content()
    check(
        f"{engine}: Edit shows the file text and Preview draws it again",
        editor == "block" and again == "The report",
        f"editor={editor} h1={again!r}",
    )


def scenario_hostile(page, slug, engine, reached):
    reached.clear()
    open_path(page, slug, "docs/hostile.html")
    sel = pane("docs/hostile.html")
    frame = frame_of(page, sel)
    # Longer than the `meta refresh` of the file.
    page.wait_for_timeout(2500)
    frame = frame_of(page, sel)
    marker = frame.locator("#marker").text_content()
    targets = frame.evaluate("() => [...document.querySelectorAll('a')].map(a => a.getAttribute('href'))")
    for target in ("#self", "#top", "#blank", "#send"):
        try:
            frame.locator(target).click(timeout=3000)
        except Exception as exc:
            print(f"   click {target}: {str(exc).splitlines()[0][:80]}", flush=True)
        page.wait_for_timeout(500)
    # The control: a request of the page itself is seen by the same listener.
    page.evaluate(f"() => fetch('/{CANARY}-control').catch(() => null)")
    page.wait_for_timeout(700)
    names = sorted(url.split(CANARY + "-")[1] for url in reached)
    check(
        f"{engine}: the hostile file runs no script and its links have no target",
        marker == "clean" and targets == [None, None, None],
        f"marker={marker!r} hrefs={targets}",
    )
    check(
        f"{engine}: no request of the hostile file reaches the daemon (the control does)",
        names == ["control"] and page.url == BASE,
        f"reached={names} page={page.url}",
    )


def scenario_detached(page, ctx, slug, engine):
    sel = pane("docs/report.html")
    open_path(page, slug, "docs/report.html")
    frame_of(page, sel)
    with ctx.expect_page() as popup:
        page.locator(f'{sel} [data-act="detach"]').click()
    detached = popup.value
    detached.on("pageerror", lambda e: print("detached pageerror:", e, flush=True))
    try:
        title = frame_of(detached, ".viewer").locator("h1").text_content()
    except Exception as exc:
        title = f"<{str(exc).splitlines()[0][:80]}>"
    check(f"{engine}: the detached window draws the frame", title == "The report", f"h1={title!r}")
    detached.close()


def run_engine(pw, engine, slug):
    args = ["--disable-webgl", "--disable-gpu"] if engine == "chromium" else []
    browser = getattr(pw, engine).launch(headless=True, args=args)
    ctx = browser.new_context(viewport={"width": 1500, "height": 950})
    reached = []
    ctx.on("response", lambda r: reached.append(r.url) if CANARY in r.url else None)
    web_asked = []

    def web_image(route):
        web_asked.append(route.request.url)
        route.fulfill(status=200, content_type="image/png", body=png(WEB_WIDTH, 1))

    ctx.route("https://images.example/**", web_image)
    page = ctx.new_page()
    page.on("pageerror", lambda e: print("pageerror:", e, flush=True))
    try:
        page.goto(BASE)
        enter(page, slug)
        scenario_report(page, slug, engine)
        scenario_hostile(page, slug, engine, reached)
        scenario_detached(page, ctx, slug, engine)
        scenario_remote(page, slug, engine, web_asked)
    except Exception as exc:
        check(f"{engine}: the pass ran to the end", False, str(exc).splitlines()[0][:200])
    finally:
        browser.close()


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbhtml_reg_")
    fixture = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)
        with sync_playwright() as pw:
            for engine in ENGINES:
                run_engine(pw, engine, slug)
    finally:
        stop(proc)

    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
