"""Browser acceptance: the secondary pane — the slot (ADR-0037 §3c).

One Playwright pass over a REAL daemon proving the canvas shows two panes at
once: a second file pinned beside the active one, or the active file mirrored
into a second Monaco editor over the SAME model.

Scenario 1  two code tabs open single, as before: one visible `.viewer`
Scenario 2  right-click B → Open to the side: two visible panes, `#viewers.split`,
            A in column 1, B in column 3, B's tab carries the pin
Scenario 3  clicking the pinned tab keeps both panes and focuses the right one
Scenario 4  Mirror on A: two editors, the model count unchanged; typing on the
            right changes the left's value and dirties Save; Ctrl+S in the
            mirror writes the file; the right scrolls independently of the left
Scenario 5  Close mirror: one editor fewer, the model count unchanged, no page error
Scenario 6  closing the pinned tab: single pane, the stored split is null
Scenario 7  pin + ratio survive a reload (per-client view, wb.view.v1)
Scenario 8  an 800px viewport paints single and greys the menu item; back at
            1400 the split returns
Scenario 9  dragging the divider moves `--wb-split` and stores the ratio
Scenario 10 detaching the pinned tab clears the slot; the popup has no divider
            — and no page error anywhere in the run

Boots a Localhost daemon on 7412 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/wb-split-panes-2026-09-22.png.
Run: python crates/ralphy-daemon/tests/wb_split_panes.py   (exit 0 = all pass)
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

PORT = 7412
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
VIEW_KEY = "wb.view.v1"

# Long enough to scroll: the mirror scenario jumps one side to the end and
# expects the other side to keep painting line 1.
A_JS = "".join(f"export function fn{i}() {{ return {i}; }}\n" for i in range(200))
B_JS = "export const b = 'the second file';\n"

VISIBLE = ".viewer:not([style*='display: none'])"

# Pinned exactly (see the exit gate): a scenario that silently stops running
# must fail the run, not shrink it.
EXPECTED_CHECKS = 37

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
    empty = tempfile.mkdtemp(prefix="wbsplit_empty_")
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
    d = tempfile.mkdtemp(prefix="wbsplit_repo_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "a.js").write_text(A_JS, encoding="utf-8")
    (p / "b.js").write_text(B_JS, encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbsplit@example.com"],
        ["git", "config", "user.name", "wbsplit"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def open_file(page, slug, path):
    page.evaluate(
        "([project, path]) => " f"{SH}.openTab({{ project, path, title: path, ftype: 'code' }})",
        [slug, path],
    )
    page.wait_for_function(
        "(p) => !!(window.monaco && window.monaco.editor.getModels().find(m => m.uri.path.endsWith(p)))"
        f" && !!document.querySelector(`.code-viewer[data-tab-id$='${{p}}'] .monaco-editor`)",
        arg=path,
        timeout=30000,
    )


def tab_id(slug, path):
    return f"file:{slug}:{path}"


def visible_panes(page):
    return page.evaluate(f"() => Array.from(document.querySelectorAll(\"{VISIBLE}\")).map(e => e.dataset.tabId || e.dataset.mirrorOf + '#mirror')")


def is_split(page):
    return page.evaluate("() => document.getElementById('viewers').classList.contains('split')")


def stored_split(page):
    return page.evaluate(f"() => JSON.parse(localStorage.getItem({VIEW_KEY!r}) || '{{}}').split ?? null")


def tab_menu(page, slug, path):
    """Right-click a tab and return the menu's rows as [label, disabled]."""
    page.click(f".tab[title='{path}']", button="right")
    page.wait_for_selector("#ctxmenu .ctx-item", timeout=5000)
    return page.evaluate(
        "() => Array.from(document.querySelectorAll('#ctxmenu .ctx-item'))"
        ".map(b => [b.textContent.trim(), b.disabled])"
    )


def click_menu(page, label):
    page.click(f"#ctxmenu .ctx-item:has-text('{label}')")


def editor_count(page):
    return page.evaluate("() => document.querySelectorAll('#viewers .monaco-editor[role=code]').length")


def model_count(page):
    return page.evaluate("() => monaco.editor.getModels().length")


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbsplit_reg_")
    repo_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, repo_dir)
    a_id, b_id = tab_id(slug, "a.js"), tab_id(slug, "b.js")

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            errors = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)

            # --- scenario 1: two tabs, one pane ---------------------------------
            open_file(page, slug, "a.js")
            open_file(page, slug, "b.js")
            page.evaluate(f"() => {SH}.activate('{a_id}')")
            page.wait_for_timeout(200)
            check("two code tabs open single: one visible pane", visible_panes(page) == [a_id], f"got={visible_panes(page)}")
            check("…and the canvas is not split", not is_split(page))

            # --- scenario 2: Open to the side ------------------------------------
            rows = tab_menu(page, slug, "b.js")
            check(
                "a code tab's menu offers the slot: Open to the side, Mirror editor, Close",
                [r[0] for r in rows] == ["Open to the side", "Mirror editor", "Close"],
                f"got={rows}",
            )
            click_menu(page, "Open to the side")
            page.wait_for_function("() => document.getElementById('viewers').classList.contains('split')", timeout=5000)
            panes = visible_panes(page)
            check("Open to the side shows two panes: the active and the pinned", sorted(panes) == sorted([a_id, b_id]), f"got={panes}")
            cols = page.evaluate(
                f"() => [document.querySelector(\"[data-tab-id='{a_id}']\").style.gridColumn,"
                f" document.querySelector(\"[data-tab-id='{b_id}']\").style.gridColumn]"
            )
            check("A sits in column 1, B in column 3", cols == ["1", "3"], f"got={cols}")
            check("B's tab carries the pin", page.evaluate("() => document.querySelector('.tab[title=\"b.js\"]').classList.contains('pinned')"))
            check("the divider is on screen", page.is_visible("#viewers .viewers-divider"))
            # Both editors laid out with a real width: neither is clipped.
            widths = page.evaluate(
                f"() => Array.from(document.querySelectorAll(\"{VISIBLE}\")).map(e => e.getBoundingClientRect().width)"
            )
            check("both panes have real width (>= 280px each)", all(w >= 280 for w in widths), f"got={widths}")
            page.screenshot(path=os.path.join(SHOT_DIR, "wb-split-panes-2026-09-22.png"))

            # --- scenario 3: click the pinned tab --------------------------------
            page.click(".tab[title='b.js']")
            page.wait_for_timeout(300)
            check("activating the pinned tab keeps both panes", sorted(visible_panes(page)) == sorted([a_id, b_id]), f"got={visible_panes(page)}")
            check("…and B stays pinned", page.evaluate(f"() => {SH}.slot?.kind === 'pin' && {SH}.slot.id === '{b_id}'"))
            focused_in = page.evaluate(
                "() => { const el = document.activeElement; const pane = el && el.closest('.viewer'); return pane ? pane.dataset.tabId : null; }"
            )
            check("…with the focus inside the right pane", focused_in == b_id, f"got={focused_in}")
            check("…and the store names B as the pin", (stored_split(page) or {}).get("path") == "b.js", f"got={stored_split(page)}")

            # --- scenario 4: mirror -----------------------------------------------
            page.evaluate(f"() => {SH}.clearSlot()")
            page.evaluate(f"() => {SH}.activate('{a_id}')")
            page.wait_for_timeout(200)
            models_before = model_count(page)
            editors_before = editor_count(page)
            page.click(f"[data-tab-id='{a_id}'] [data-act='mirror']")
            page.wait_for_selector("#viewers .mirror-viewer .monaco-editor", timeout=10000)
            page.wait_for_timeout(300)
            check("Mirror adds one editor", editor_count(page) == editors_before + 1, f"before={editors_before} after={editor_count(page)}")
            check("…and no model", model_count(page) == models_before, f"before={models_before} after={model_count(page)}")
            check("the mirror pane names the same path", page.evaluate("() => document.querySelector('.mirror-viewer .viewer-file').textContent") == "a.js")
            # Type on the RIGHT: the left's value follows, and Save dirties.
            page.click(".mirror-viewer .view-lines")
            page.keyboard.press("Control+Home")
            page.keyboard.type("// mirrored edit\n")
            page.wait_for_timeout(200)
            left_value = page.evaluate(
                f"() => monaco.editor.getModels().find(m => m.uri.path.endsWith('a.js')).getValue()"
            )
            check("typing in the mirror changes the one shared model", left_value.startswith("// mirrored edit"), f"head={left_value[:30]!r}")
            check("…and dirties the pane's Save", page.evaluate(f"() => document.querySelector(\"[data-tab-id='{a_id}'] .vbtn.save\").classList.contains('dirty')"))
            # Ctrl+S in the mirror saves the pane it mirrors — the binding is
            # the editor's own (`addAction`), so two editors on screen do not
            # fight over it — and the bytes reach the real file.
            page.keyboard.press("Control+S")
            deadline = time.time() + 10
            saved = ""
            while time.time() < deadline:
                saved = Path(repo_dir, "a.js").read_text(encoding="utf-8")
                if saved.startswith("// mirrored edit"):
                    break
                time.sleep(0.2)
            check("Ctrl+S in the mirror writes the shared text to disk", saved.startswith("// mirrored edit"), f"head={saved[:20]!r}")
            # Jump the RIGHT's cursor to the end: it scrolls to reveal it, the
            # left stays put — two cursors, two viewports, one text.
            page.keyboard.press("Control+End")
            page.wait_for_timeout(400)
            # Monaco scrolls by re-rendering, not by `scrollTop`: the first
            # line number each pane paints is the observable.
            first_lines = page.evaluate(
                f"() => [document.querySelector(\"[data-tab-id='{a_id}']\"), document.querySelector('.mirror-viewer')]"
                ".map(p => Math.min(...Array.from(p.querySelectorAll('.line-numbers')).map(e => parseInt(e.textContent, 10)).filter(Number.isFinite)))"
            )
            check("the mirror scrolls; the pane it mirrors does not", first_lines[1] > 1 and first_lines[0] == 1, f"first lines left={first_lines[0]} right={first_lines[1]}")

            # --- scenario 5: close the mirror ------------------------------------
            page.click(".mirror-viewer [data-act='mirror']")
            page.wait_for_function("() => !document.querySelector('#viewers .mirror-viewer')", timeout=5000)
            page.wait_for_timeout(200)
            check("Close mirror takes the editor away", editor_count(page) == editors_before, f"got={editor_count(page)}")
            check("…and keeps the model", model_count(page) == models_before, f"got={model_count(page)}")
            check("…with no page error", errors == [], f"errors={errors}")
            check("the toolbar button reads Mirror again", page.evaluate(f"() => document.querySelector(\"[data-tab-id='{a_id}'] [data-act='mirror']\").title") == "Mirror")

            # --- scenario 6: close the pinned tab ----------------------------------
            page.evaluate(f"() => {SH}.pinTab('{b_id}')")
            page.wait_for_function("() => document.getElementById('viewers').classList.contains('split')", timeout=5000)
            page.evaluate(f"() => {SH}.closeTab('{b_id}')")
            page.wait_for_function("() => !document.getElementById('viewers').classList.contains('split')", timeout=5000)
            check("closing the pinned tab returns to one pane", visible_panes(page) == [a_id], f"got={visible_panes(page)}")
            check("…and the store says so, explicitly", stored_split(page) is None, f"got={stored_split(page)}")

            # --- scenario 7: a reload restores the pin and the ratio ---------------
            open_file(page, slug, "b.js")
            page.evaluate(f"() => {SH}.activate('{a_id}')")
            page.evaluate(f"() => {SH}.pinTab('{b_id}')")
            page.wait_for_function("() => document.getElementById('viewers').classList.contains('split')", timeout=5000)
            page.evaluate(
                "() => document.dispatchEvent(new CustomEvent('workbench:split-ratio', { detail: { ratio: 0.35 } }))"
            )
            page.wait_for_timeout(200)
            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function("() => document.getElementById('viewers').classList.contains('split')", timeout=30000)
            page.wait_for_timeout(500)
            # `restoreView` holds the store's write suppressor for 3s after a
            # reload (a refused read must not rewrite the record); the writes
            # scenarios 8-9 assert on come after it.
            page.wait_for_function(f"() => {SH}._restoring === false", timeout=10000)
            check("after a reload the two panes are back", sorted(visible_panes(page)) == sorted([a_id, b_id]), f"got={visible_panes(page)}")
            check("…B is still the pin", page.evaluate("() => document.querySelector('.tab[title=\"b.js\"]').classList.contains('pinned')"))
            ratio_var = page.evaluate("() => document.getElementById('viewers').style.getPropertyValue('--wb-split')")
            check("…at the stored ratio", ratio_var == "35.00%", f"got={ratio_var!r}")

            # --- scenario 8: under the width floor -----------------------------------
            page.set_viewport_size({"width": 800, "height": 900})
            page.wait_for_function("() => !document.getElementById('viewers').classList.contains('split')", timeout=5000)
            check("an 800px viewport paints single", visible_panes(page) == [a_id], f"got={visible_panes(page)}")
            rows = tab_menu(page, slug, "a.js")
            check("…and greys the slot items", [r for r in rows if r[0] == "Open to the side"][0][1] is True, f"got={rows}")
            page.keyboard.press("Escape")
            page.click("body", position={"x": 5, "y": 5})
            page.set_viewport_size({"width": 1400, "height": 900})
            page.wait_for_function("() => document.getElementById('viewers').classList.contains('split')", timeout=5000)
            check("back at 1400px the split returns", sorted(visible_panes(page)) == sorted([a_id, b_id]), f"got={visible_panes(page)}")

            # --- scenario 9: drag the divider -------------------------------------------
            box = page.evaluate("() => { const r = document.querySelector('#viewers .viewers-divider').getBoundingClientRect(); return [r.x + r.width/2, r.y + 40]; }")
            vbox = page.evaluate("() => { const r = document.getElementById('viewers').getBoundingClientRect(); return [r.x, r.width]; }")
            target_x = vbox[0] + vbox[1] * 0.6
            page.mouse.move(box[0], box[1])
            page.mouse.down()
            page.mouse.move(target_x, box[1], steps=8)
            page.mouse.up()
            page.wait_for_timeout(300)
            ratio_var = page.evaluate("() => document.getElementById('viewers').style.getPropertyValue('--wb-split')")
            stored_ratio = (stored_split(page) or {}).get("ratio")
            check("dragging the divider moves the split", ratio_var.startswith("60."), f"got={ratio_var!r}")
            check("…and stores the ratio", stored_ratio is not None and abs(stored_ratio - 0.6) < 0.02, f"got={stored_ratio}")

            # --- scenario 10: detach the pinned tab ---------------------------------------
            with page.expect_popup(timeout=15000) as info:
                page.click(f"[data-tab-id='{b_id}'] [data-act='detach']")
            popup = info.value
            popup.wait_for_load_state()
            popup.wait_for_selector(".viewer", timeout=15000)
            page.wait_for_function("() => !document.getElementById('viewers').classList.contains('split')", timeout=5000)
            check("detaching the pinned tab clears the slot", page.evaluate(f"() => {SH}.slot") is None)
            check("…the popup has no divider and no mirror button", popup.evaluate("() => !document.querySelector('.viewers-divider') && !document.querySelector('[data-act=\"mirror\"]')"))
            popup.close()

            # Asserted LAST, over the whole run: the paths above (reload,
            # detach, the width crossing) are where a throw would hide.
            check("no page error across the run", errors == [], f"errors={errors}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) == EXPECTED_CHECKS
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if len(results) != EXPECTED_CHECKS:
        print(f"EXPECTED {EXPECTED_CHECKS} checks, ran {len(results)}", flush=True)
    if ok:
        print("THE SLOT HOLDS")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
