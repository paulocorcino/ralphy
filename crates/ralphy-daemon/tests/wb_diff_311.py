"""#311 browser acceptance: the diff tab opened from a Changes row.

One Playwright pass over a REAL daemon proving a row click opens a diff tab after
the fixed Consoles tab, the diff renders side by side with unchanged regions
collapsed, a newly added file diffs against absence, closing the tab leaves the
Consoles tab and its live consoles untouched, no commit/discard/stage control
exists on the surface, and a binary row refuses instead of opening.

Scenario 1  a `.chg-row` click opens `diff:<slug>:README.md` at tabs[1], with
            tabs[0].id === "consoles"
Scenario 2  two code panes side by side (equal widths, modified starting where
            original ends) with the `.diff-hidden-lines` collapse widget present
Scenario 3  an untracked file's diff has original === "" and `brand new line`
            on the modified side
Scenario 4  closing the diff tab leaves 1 tab, active "consoles", and the live
            console window count unchanged with its xterm still attached
Scenario 5  `.diff-viewer` exposes exactly one button labelled `Find` and zero
            commit/discard/stage controls
Scenario 6  a binary row flashes `binary` and leaves no diff tab open

Oracles for scenario 2 come from the step-19 live probe: a diff editor has no
`getOption`, so "side by side" is measured GEOMETRICALLY, and the collapse widget
class `.diff-hidden-lines` was read off the live DOM.

Boots a Localhost daemon on 7411 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/311-diff-tab-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_diff_311.py   (exit 0 = all pass)
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

PORT = 7411
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_diff_311.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

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
    """A scratch registry + empty vendor stores: the operator's own daemon dir
    (and its login policy) is never touched, and the usage scan finds nothing."""
    empty = tempfile.mkdtemp(prefix="wb311_empty_")
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
    """A committed repo whose dirt covers every side of the diff: a modified file
    with a 200-line unchanged run, an untracked add, a delete, and a committed
    binary that is then modified."""
    d = tempfile.mkdtemp(prefix="wb311_repo_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    long_body = "\n".join(f"line {i}" for i in range(200)) + "\n"
    (p / "README.md").write_text(long_body, encoding="utf-8")
    (p / "gone.txt").write_text("doomed\n", encoding="utf-8")
    # NUL in the first bytes: the binary tell both readers agree on.
    (p / "logo.png").write_bytes(bytes([0x89, 0x50, 0x00, 0x01, 0x02]))
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb311@example.com"],
        ["git", "config", "user.name", "wb311"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)

    lines = long_body.split("\n")
    lines[100] = "line 100 EDITED BY THE AGENT"
    (p / "README.md").write_text("\n".join(lines), encoding="utf-8")  # modified
    (p / "brand-new.txt").write_text("brand new line\n", encoding="utf-8")  # untracked
    subprocess.run(["git", "rm", "-q", "gone.txt"], cwd=d, check=True, capture_output=True)  # deleted
    (p / "logo.png").write_bytes(bytes([0x89, 0x50, 0x00, 0x09, 0x09]))  # modified binary
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's sidebar.
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True
    )


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


VISIBLE_SECS = (
    "Array.from(document.querySelectorAll('.changes-sec')).filter(e => e.offsetParent !== null)"
)

# Gate every wait on the offsetParent-filtered count, never the raw
# querySelectorAll length: rows exist in the DOM before Alpine's `x-show` flip, so
# a raw count resolves early and the next evaluate reads pre-flip state
# (handoffs.md #307/#309).
VISIBLE_ROWS = (
    "Array.from(document.querySelectorAll('.chg-row')).filter(r => r.offsetParent !== null)"
)


def wait_badge(page, expected, timeout=15000):
    page.wait_for_function(
        f"(want) => {{ const els = {VISIBLE_SECS};"
        " return els.length === 1 && els[0].querySelector('.count').textContent.trim() === want; }",
        arg=expected,
        timeout=timeout,
    )


def open_rows(page, slug, expected_count):
    page.evaluate(f"() => {SH}.toggle('{slug}')")
    wait_badge(page, str(expected_count))
    page.click(".changes-sec:visible")
    page.wait_for_function(
        f"(want) => {VISIBLE_ROWS}.length === want", arg=expected_count, timeout=8000
    )


def click_row(page, path):
    """Click the VISIBLE row for `path`. A bare `page.click` resolves to the first
    DOM match, which may be hidden once more than one project is registered."""
    page.evaluate(
        f"(want) => {{ const r = {VISIBLE_ROWS}"
        ".find(r => r.querySelector('.chg-path').textContent.trim() === want);"
        " if (!r) throw new Error('no visible row for ' + want); r.click(); }",
        arg=path,
    )


def wait_diff_mounted(page, tab_id, timeout=25000):
    page.wait_for_function(
        "(id) => { const el = document.querySelector(`.diff-viewer[data-tab-id=\"${id}\"]`);"
        " return !!el && !!el.querySelector('.monaco-diff-editor .view-lines'); }",
        arg=tab_id,
        timeout=timeout,
    )


def tab_ids(page):
    return page.evaluate(f"() => {SH}.tabs.map(t => t.id)")


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb311_reg_")
    fixture = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows (KNOWLEDGE.md).
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1500, "height": 950})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)

            # 4 changed paths: README.md, brand-new.txt, gone.txt, logo.png.
            open_rows(page, slug, 4)

            # Monaco's model registry BEFORE any diff exists: the dispose oracle
            # below is "back to this number", so it must be read while no diff is open.
            model_baseline = page.evaluate(
                "() => (typeof monaco === 'undefined' ? 0 : monaco.editor.getModels().length)"
            )

            # --- scenario 1: a row click opens a diff tab after Consoles -------
            diff_id = f"diff:{slug}:README.md"
            click_row(page, "README.md")
            wait_diff_mounted(page, diff_id)
            ids = tab_ids(page)
            check(
                "the diff tab opens at tabs[1], after the fixed Consoles tab",
                ids == ["consoles", diff_id],
                f"got={ids}",
            )

            # --- scenario 2: side by side, unchanged regions collapsed ---------
            # The collapse ruler renders after Monaco's diff COMPUTATION settles,
            # which is later than the first `.view-lines` paint — waiting on the
            # paint alone reads zero widgets on a diff that does collapse.
            page.wait_for_function(
                "(id) => { const el = document.querySelector(`.diff-viewer[data-tab-id=\"${id}\"]`);"
                " return !!el && el.querySelectorAll('.diff-hidden-lines').length > 0; }",
                arg=diff_id,
                timeout=20000,
            )
            geom = page.evaluate(
                """(id) => {
                  const root = document.querySelector(`.diff-viewer[data-tab-id="${id}"]`);
                  const de = root.querySelector('.monaco-diff-editor');
                  const box = (sel) => { const e = de.querySelector(sel); if (!e) return null;
                    const r = e.getBoundingClientRect(); return { left: Math.round(r.left), width: Math.round(r.width) }; };
                  return {
                    panes: de.querySelectorAll('.view-lines').length,
                    original: box('.editor.original'),
                    modified: box('.editor.modified'),
                    paneClasses: [
                      de.querySelectorAll('.original-in-monaco-diff-editor').length,
                      de.querySelectorAll('.modified-in-monaco-diff-editor').length,
                    ],
                    collapsed: de.querySelectorAll('.diff-hidden-lines').length,
                    diffEditors: (typeof monaco !== 'undefined' && monaco.editor.getDiffEditors)
                      ? monaco.editor.getDiffEditors().length : -1,
                    bodyH: Math.round(root.querySelector('.viewer-body').getBoundingClientRect().height),
                    deH: Math.round(de.getBoundingClientRect().height),
                  };
                }""",
                diff_id,
            )
            check(
                "the diff renders exactly two code panes (an original and a modified)",
                geom["panes"] == 2 and geom["paneClasses"] == [1, 1],
                f"got={geom['panes']} classes={geom['paneClasses']}",
            )
            o, m = geom["original"], geom["modified"]
            check(
                "the two panes sit side by side: equal widths, modified starting where original ends",
                bool(o and m)
                and abs(o["width"] - m["width"]) <= 2
                and abs(m["left"] - (o["left"] + o["width"])) <= 2
                and o["width"] > 100,
                f"original={o} modified={m}",
            )
            check(
                "unchanged regions are collapsed (the .diff-hidden-lines ruler is present)",
                geom["collapsed"] > 0,
                f"widgets={geom['collapsed']}",
            )
            check(
                "the diff pane fills its tab body with no clipping",
                geom["bodyH"] > 200 and abs(geom["bodyH"] - geom["deH"]) <= 2,
                f"bodyH={geom['bodyH']} deH={geom['deH']}",
            )

            page.screenshot(path=os.path.join(SHOT_DIR, "311-diff-tab-2026-07-25.png"))

            # Monaco's own `useInlineViewWhenSpaceIsLimited` default swaps to the
            # INLINE view under 900px, which `renderSideBySide: true` does not
            # defeat — pinning side-by-side at one wide viewport would miss it.
            page.set_viewport_size({"width": 700, "height": 900})
            page.wait_for_timeout(600)
            narrow = page.evaluate(
                """(id) => {
                  const de = document.querySelector(`.diff-viewer[data-tab-id="${id}"] .monaco-diff-editor`);
                  const box = (sel) => { const e = de.querySelector(sel); if (!e) return null;
                    const r = e.getBoundingClientRect(); return { left: Math.round(r.left), width: Math.round(r.width) }; };
                  return { panes: de.querySelectorAll('.view-lines').length,
                           original: box('.editor.original'), modified: box('.editor.modified') };
                }""",
                diff_id,
            )
            check(
                "the diff stays SIDE BY SIDE at a 700px-wide viewport (no inline swap)",
                narrow["panes"] == 2
                and bool(narrow["original"] and narrow["modified"])
                and narrow["modified"]["left"] > narrow["original"]["left"],
                f"got={narrow}",
            )
            page.set_viewport_size({"width": 1500, "height": 950})
            page.wait_for_timeout(400)

            # --- scenario 5: no commit / discard / staging control -------------
            controls = page.evaluate(
                """(id) => {
                  const root = document.querySelector(`.diff-viewer[data-tab-id="${id}"]`);
                  const re = /commit|discard|stage|revert/i;
                  // Text AND the attributes an icon-only control would hide behind:
                  // a glyph button carries no textContent at all.
                  const hay = (e) => [
                    e.children.length ? "" : e.textContent || "",
                    e.getAttribute('title') || "",
                    e.getAttribute('aria-label') || "",
                    e.getAttribute('data-act') || "",
                  ].join(" ");
                  const scan = (sel) => Array.from(document.querySelectorAll(sel))
                    .flatMap(r => Array.from(r.querySelectorAll('*')))
                    .filter(e => re.test(hay(e))).length;
                  return {
                    vbtns: Array.from(root.querySelectorAll('.vbtn')).map(b => b.textContent.trim()),
                    acts: Array.from(root.querySelectorAll('[data-act]')).map(b => b.getAttribute('data-act')),
                    mutators: scan('.diff-viewer') + scan('.changes-list') + scan('.changes-sec') + scan('.tabbar'),
                    // Monaco's own margin revert arrow writes to the modified side.
                    revertGlyphs: root.querySelectorAll('.diff-review-insert, .codicon-diff-revert, .revertButton').length,
                  };
                }""",
                diff_id,
            )
            check(
                "the diff toolbar exposes exactly one button, labelled Find",
                controls["vbtns"] == ["Find"],
                f"got={controls['vbtns']}",
            )
            check(
                "the only actionable control is the find action",
                controls["acts"] == ["find"],
                f"got={controls['acts']}",
            )
            check(
                "no commit / discard / stage / revert control exists on this surface",
                controls["mutators"] == 0 and controls["revertGlyphs"] == 0,
                f"matches={controls['mutators']} revertGlyphs={controls['revertGlyphs']}",
            )

            # --- scenario 3: a newly added file diffs against absence ----------
            added_id = f"diff:{slug}:brand-new.txt"
            click_row(page, "brand-new.txt")
            wait_diff_mounted(page, added_id)
            sides = page.evaluate(
                """() => { const eds = monaco.editor.getDiffEditors();
                  const ed = eds[eds.length - 1]; const m = ed.getModel();
                  return { original: m.original.getValue(), modified: m.modified.getValue() }; }"""
            )
            check(
                "an added file's original side is empty",
                sides["original"] == "",
                f"got={sides['original']!r}",
            )
            check(
                "an added file's modified side carries its bytes",
                "brand new line" in sides["modified"],
                f"got={sides['modified']!r}",
            )

            # --- scenario 3b: a deleted row exercises the REAL blob.read reply --
            # The added-file case short-circuits `blob.read` entirely (headAbsent),
            # so without this the verb's `present` relay is never proved end to end.
            deleted_id = f"diff:{slug}:gone.txt"
            click_row(page, "gone.txt")
            wait_diff_mounted(page, deleted_id)
            del_sides = page.evaluate(
                """(id) => { const eds = monaco.editor.getDiffEditors();
                  const ed = eds[eds.length - 1]; const m = ed.getModel();
                  return { original: m.original.getValue(), modified: m.modified.getValue() }; }""",
                deleted_id,
            )
            check(
                "a deleted row reads its HEAD side over the wire (blob.read present)",
                "doomed" in del_sides["original"],
                f"got={del_sides['original']!r}",
            )
            check(
                "a deleted row's working side is empty",
                del_sides["modified"] == "",
                f"got={del_sides['modified']!r}",
            )

            # --- scenario 6: a binary row refuses, opening no diff tab ---------
            before = tab_ids(page)
            click_row(page, "logo.png")
            page.wait_for_function(
                f"() => ({SH}.runsActionMsg || '').includes('binary')", timeout=15000
            )
            # The tab is pushed optimistically, then closed on the refusal — wait
            # for the close rather than reading a mid-flight tab list.
            page.wait_for_function(
                f"(want) => {SH}.tabs.map(t => t.id).join('|') === want",
                arg="|".join(before),
                timeout=8000,
            )
            check(
                "a binary row flashes `binary` and leaves no diff tab open",
                tab_ids(page) == before,
                f"before={before} after={tab_ids(page)}",
            )

            # --- scenario 4: closing a diff tab never disturbs the consoles ----
            page.evaluate(f"() => {SH}.newPlainConsole()")
            consoles_before = page.wait_for_function(
                "() => { const n = document.querySelectorAll('#workspace .session-window').length;"
                " return n > 0 ? n : false; }",
                timeout=20000,
            ).json_value()
            xterm_before = page.evaluate(
                "() => document.querySelectorAll('#workspace .session-window .xterm').length"
            )
            # Close every diff tab; the Consoles tab must be all that is left.
            page.evaluate(
                f"() => {SH}.tabs.filter(t => t.kind === 'diff').map(t => t.id)"
                f".forEach(id => {SH}.closeTab(id))"
            )
            page.wait_for_function(f"() => {SH}.tabs.length === 1", timeout=8000)
            after = page.evaluate(
                f"""() => ({{
                  ids: {SH}.tabs.map(t => t.id),
                  active: {SH}.active,
                  consoles: document.querySelectorAll('#workspace .session-window').length,
                  xterms: document.querySelectorAll('#workspace .session-window .xterm').length,
                  panes: document.querySelectorAll('.diff-viewer').length,
                }})"""
            )
            check(
                "closing the diff tabs leaves only the Consoles tab, active",
                after["ids"] == ["consoles"] and after["active"] == "consoles",
                f"got={after}",
            )
            check(
                "the live console window count and its xterm are unchanged",
                after["consoles"] == consoles_before and after["xterms"] == xterm_before,
                f"before={consoles_before}/{xterm_before} after={after['consoles']}/{after['xterms']}",
            )
            check(
                "the diff panes are torn out of the DOM on close",
                after["panes"] == 0,
                f"got={after['panes']}",
            )

            # BOTH models disposed: counted, not inferred. A reopen does NOT prove
            # it — `WBMonaco.createDiff` puts the viewer's per-open `uid` in each
            # model URI, so a reopened tab can never collide with a leaked model.
            # The only honest oracle is Monaco's own model registry returning to
            # the baseline taken before any diff was opened.
            leaked = page.evaluate(
                "(base) => ({ models: monaco.editor.getModels().length,"
                " baseline: base, diffEditors: monaco.editor.getDiffEditors().length })",
                model_baseline,
            )
            check(
                "closing the diff tabs disposes BOTH models of each (no leak)",
                leaked["models"] == model_baseline and leaked["diffEditors"] == 0,
                f"baseline={model_baseline} after={leaked['models']} diffEditors={leaked['diffEditors']}",
            )

            # And the path still reopens afterwards.
            click_row(page, "README.md")
            wait_diff_mounted(page, diff_id)
            check(
                "the same path reopens after being closed",
                tab_ids(page) == ["consoles", diff_id],
                f"got={tab_ids(page)}",
            )

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 20
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("DIFF TAB LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
