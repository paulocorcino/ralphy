"""Browser acceptance: a Mermaid file (`.mmd`) opens as a drawn diagram.

One Playwright pass over a REAL daemon. A fixture repo holds three Mermaid
files under `docs/`: a small flowchart, a file Mermaid rejects, and an
`erDiagram` over Mermaid's default limit of 50,000 characters.

Scenario 1  `flow.mmd` opens as one diagram (one svg, with the label `Start`)
Scenario 2  Edit shows the file text in the editor; Preview draws it again
Scenario 3  the diagram has one Enlarge button, and the pane shows no heading
            index (no outline, no Contents button)
Scenario 4  a change on disk draws the diagram again
Scenario 5  `broken.mmd` says "Mermaid error:" in the pane
Scenario 6  `big.mmd` (more than 50,000 characters) is drawn, all 120 entities
Scenario 7  the detached file window draws the diagram

Requires `playwright` (`pip install playwright`, then
`playwright install chromium`).

Boots a Localhost daemon over a SCRATCH `RALPHY_DAEMON_DIR`. The daemon is
stopped by its own subprocess handle, NEVER by name.

Writes .ralphy/screenshots/rendered-files-*.png.
Run: python tests/browser/files/wb_rendered_files.py   (exit 0 = all pass)
"""

import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7581
BASE = f"http://127.0.0.1:{PORT}/"

# tests/browser/files/wb_rendered_files.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
FILES = "Alpine.$data(document.querySelector('[x-data=\"wbFiles\"]'))"

FLOW = "flowchart TD\n  A[Start] --> B[Finish]\n"
CHANGED = "flowchart TD\n  A[Changed] --> B[Finish]\n"
BROKEN = "flowchart TD\n  A[Start --> (((\n  this is not mermaid ]]]\n"
BIG_ENTITIES = 120
BIG_COLUMNS = 8
MERMAID_DEFAULT_LIMIT = 50000

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def big_diagram():
    """An `erDiagram` with no relations, over Mermaid's default text limit."""
    lines = ["erDiagram"]
    for e in range(BIG_ENTITIES):
        lines.append(f"  table_number_{e:03d} {{")
        for c in range(BIG_COLUMNS):
            lines.append(f'    string column_number_{c:02d}_of_the_table "a comment that makes the file longer"')
        lines.append("  }")
    text = "\n".join(lines) + "\n"
    assert len(text) > MERMAID_DEFAULT_LIMIT, len(text)
    return text


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
    empty = tempfile.mkdtemp(prefix="wbrender_empty_")
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
    d = tempfile.mkdtemp(prefix="wbrender_repo_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    docs = p / "docs"
    docs.mkdir()
    (docs / "flow.mmd").write_text(FLOW, encoding="utf-8", newline="\n")
    (docs / "broken.mmd").write_text(BROKEN, encoding="utf-8", newline="\n")
    (docs / "big.mmd").write_text(big_diagram(), encoding="utf-8", newline="\n")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbrender@example.com"],
        ["git", "config", "user.name", "wbrender"],
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


def open_from_tree(page, folder, title):
    """Open a file the way a double-click does. The folder stays expanded, so
    the daemon's change nudge for it reaches the open tab."""
    page.wait_for_function(
        f"(f) => !!{FILES}._tree && !!{FILES}.rawTree().findFirst(n => n.title === f)", arg=folder, timeout=20000
    )
    page.evaluate(f"async (f) => {{ await {FILES}.rawTree().findFirst(n => n.title === f).setExpanded(true); }}", folder)
    page.wait_for_function(f"(t) => !!{FILES}.rawTree().findFirst(n => n.title === t)", arg=title, timeout=20000)
    page.evaluate(f"(t) => {FILES}.openFile({FILES}.rawTree().findFirst(n => n.title === t))", arg=title)


def pane(path):
    """The selector of the pane that shows `path` (its tab id ends with it)."""
    return f'.viewer[data-tab-id$=":{path}"]'


def svg_count(page, sel):
    return page.evaluate("(s) => document.querySelectorAll(s + ' .md-body svg').length", sel)


def svg_text(page, sel):
    return page.evaluate(
        "(s) => [...document.querySelectorAll(s + ' .md-body svg')].map(e => e.textContent).join(' ')", sel
    )


def wait_svg(page, sel, timeout=30000):
    page.wait_for_function("(s) => !!document.querySelector(s + ' .md-body .mermaid svg')", arg=sel, timeout=timeout)


def scenario_flow(page, fixture):
    sel = pane("docs/flow.mmd")
    wait_svg(page, sel)
    n, text = svg_count(page, sel), svg_text(page, sel)
    check("mmd: the file is drawn as one diagram", n == 1 and "Start" in text, f"svgs={n} text={text[:60]!r}")
    page.screenshot(path=os.path.join(SHOT_DIR, "rendered-files-flow.png"))

    page.wait_for_function("(s) => !!document.querySelector(s + ' .md-body .zoom-btn')", arg=sel, timeout=10000)
    buttons = page.evaluate("(s) => document.querySelectorAll(s + ' .zoom-btn').length", sel)
    check("mmd: the diagram has an Enlarge button", buttons == 1, f"buttons={buttons}")
    hidden = page.evaluate(
        """(s) => {
          const p = document.querySelector(s);
          const shown = (q) => getComputedStyle(p.querySelector(q)).display;
          return { outline: shown('nav.md-outline'), toc: shown('.md-toc-btn') };
        }""",
        sel,
    )
    check(
        "mmd: the pane shows no heading index",
        hidden == {"outline": "none", "toc": "none"},
        f"got={hidden}",
    )

    # Edit shows the source in Monaco; Preview draws it again.
    page.locator(f'{sel} [data-act="toggle"]').click()
    page.wait_for_function(
        "(s) => !!document.querySelector(s + ' .md-editor .monaco-editor')", arg=sel, timeout=30000
    )
    page.wait_for_function(
        "(src) => !!window.monaco && monaco.editor.getModels().some(m => m.getValue() === src)", arg=FLOW, timeout=15000
    )
    editor_shown = page.evaluate("(s) => getComputedStyle(document.querySelector(s + ' .md-editor')).display", sel)
    page.locator(f'{sel} [data-act="toggle"]').click()
    wait_svg(page, sel)
    n = svg_count(page, sel)
    check(
        "mmd: Edit shows the source and Preview draws it again",
        editor_shown == "block" and n == 1,
        f"editor={editor_shown} svgs={n}",
    )

    # A change on disk draws the diagram again.
    Path(fixture, "docs", "flow.mmd").write_text(CHANGED, encoding="utf-8", newline="\n")
    try:
        page.wait_for_function(
            "(s) => [...document.querySelectorAll(s + ' .md-body svg')].some(e => e.textContent.includes('Changed'))",
            arg=sel,
            timeout=10000,
        )
        changed = True
    except Exception:
        changed = False
    badge = page.evaluate("(s) => getComputedStyle(document.querySelector(s + ' .viewer-disk-badge')).display", sel)
    check(
        "mmd: a change on disk draws the diagram again",
        changed and svg_count(page, sel) == 1,
        f"svgs={svg_count(page, sel)} disk-badge={badge}",
    )


def scenario_broken(page, slug):
    open_path(page, slug, "docs/broken.mmd")
    sel = pane("docs/broken.mmd")
    page.wait_for_function("(s) => !!document.querySelector(s + ' .mermaid-error')", arg=sel, timeout=30000)
    text = page.evaluate("(s) => document.querySelector(s + ' .mermaid-error').textContent", sel)
    check("mmd: a broken diagram says so in the pane", text.startswith("Mermaid error:"), f"text={text[:80]!r}")


def scenario_big(page, slug):
    open_path(page, slug, "docs/big.mmd")
    sel = pane("docs/big.mmd")
    wait_svg(page, sel, timeout=60000)
    got = page.evaluate(
        """(s) => {
          const svg = document.querySelector(s + ' .md-body svg');
          const names = new Set([...svg.querySelectorAll('text, tspan')]
            .map(t => t.textContent.trim())
            .filter(t => /^table_number_\\d{3}$/.test(t)));
          return { entities: names.size, tooBig: svg.textContent.includes('Maximum text size') };
        }""",
        sel,
    )
    check(
        "mmd: a diagram over 50,000 characters is drawn",
        got["entities"] == BIG_ENTITIES and not got["tooBig"],
        f"got={got}",
    )


def scenario_detached(page, ctx, slug):
    open_path(page, slug, "docs/flow.mmd")
    sel = pane("docs/flow.mmd")
    wait_svg(page, sel)
    with ctx.expect_page() as popup:
        page.locator(f'{sel} [data-act="detach"]').click()
    detached = popup.value
    detached.on("pageerror", lambda e: print("detached pageerror:", e, flush=True))
    try:
        detached.wait_for_function("() => !!document.querySelector('.md-body .mermaid svg')", timeout=30000)
        n = detached.evaluate("() => document.querySelectorAll('.md-body svg').length")
    except Exception:
        n = 0
    detached.screenshot(path=os.path.join(SHOT_DIR, "rendered-files-detached.png"))
    check("mmd: the detached window draws the diagram", n == 1, f"svgs={n}")


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbrender_reg_")
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
            page.on("pageerror", lambda e: print("pageerror:", e, flush=True))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function("() => Alpine.store('projects').projects.length === 1", timeout=15000)
            page.evaluate(
                "(s) => { if (Alpine.store('projects').openSlug !== s)"
                " Alpine.$data(document.querySelector('[x-data]')).toggle(s); }",
                arg=slug,
            )
            open_from_tree(page, "docs", "flow.mmd")
            scenario_flow(page, fixture)
            scenario_broken(page, slug)
            scenario_big(page, slug)
            page.screenshot(path=os.path.join(SHOT_DIR, "rendered-files-big.png"))
            scenario_detached(page, ctx, slug)
            browser.close()
    finally:
        stop(proc)

    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
