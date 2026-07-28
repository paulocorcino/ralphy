"""#317 browser acceptance: Changes is a rail view, not a sidebar accordion.

One Playwright pass over a REAL daemon proving the promotion: the rail switches
the sidebar's view, the view is scoped to the open project, the change count
survives the move as a per-project badge that CANNOT read as a cross-repo
aggregate, the diff tab is unchanged, the accordion is gone, and the view still
holds a toolbar and a message box over a 60-file list at two viewport widths.

Scenario 1  the rail has 5 buttons and no digits; clicking Changes shows
            `.changes-view` and hides `.projects-view`, and Projects reverses it
Scenario 2  the visible `.chg-name` set equals the OPEN project's paths, and
            re-scopes when another project is opened
Scenario 3  the open project's `.chg-badge` is readable with the Changes view
            closed; with no project open the rows read 2 on A and 3 on B while NO
            element on the page reads 5 (the aggregate an implementation that
            summed the map would print), an unopened project shows no badge, and
            a failed `changes.list` moves A's badge to `—`, never `0`
Scenario 4  clicking a `.chg-row` still opens the diff as a canvas tab
Scenario 5  the accordion is gone: no `.changes-sec`, no `li.project
            .changes-list`
Scenario 6  1440x900 over 60 changes: the toolbar and the message box stay
            inside the sidebar, keep a usable height, and the list scrolls
            instead of pushing them out — plus the same at a 160px sidebar,
            since `--side-w` is a fixed track no viewport width reflows
Scenario 7  the same at 390x844 (a phone width)

Boots a Localhost daemon on 7417 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/317-changes-rail-{desktop,phone}-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_changes_317.py   (exit 0 = all pass)
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

PORT = 7417
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_changes_317.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

RAIL_CHANGES = "nav.rail button[title=\"Changes\"]"
RAIL_PROJECTS = "nav.rail button[title=\"Projects\"]"
VIEW = "document.querySelector('.changes-view')"
PROJ_VIEW = "document.querySelector('.projects-view')"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def info(name, detail):
    print(f"[INFO] {name} {detail}", flush=True)


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
    empty = tempfile.mkdtemp(prefix="wb317_empty_")
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


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def seed(d, tag, extra=()):
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text(f"# {tag}\n\nThe #317 rail-view fixture repo.\n", encoding="utf-8")
    for name in extra:
        (p / name).write_text(f"{name}\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb317@example.com")
    git(d, "config", "user.name", "wb317")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return p


def make_two():
    """A = exactly 2 changed PATHS: a staged README.md, and a renamed path that
    is also modified in the worktree — so both groups render AND `.chg-from`
    has a row to draw on."""
    d = tempfile.mkdtemp(prefix="wb317_a_")
    p = seed(d, "alpha", extra=("old.txt",))
    (p / "README.md").write_text("# alpha\n\nstaged edit\n", encoding="utf-8")
    git(d, "add", "README.md")
    git(d, "mv", "old.txt", "new.txt")
    (p / "new.txt").write_text("old.txt\nworktree edit\n", encoding="utf-8")
    return d


def make_three():
    """B = exactly 3 changed paths, all unstaged."""
    d = tempfile.mkdtemp(prefix="wb317_b_")
    p = seed(d, "bravo", extra=("one.txt", "two.txt"))
    for name in ("README.md", "one.txt", "two.txt"):
        (p / name).write_text(f"{name} edited\n", encoding="utf-8")
    return d


def make_clean():
    """C = a registered project that is never opened — the control for "a slug
    nobody read renders no badge at all"."""
    d = tempfile.mkdtemp(prefix="wb317_c_")
    seed(d, "charlie")
    return d


def make_sixty():
    """D = 60 changed paths: the layout budget's load."""
    d = tempfile.mkdtemp(prefix="wb317_d_")
    p = seed(d, "delta")
    for i in range(60):
        (p / f"file-{i:02d}.txt").write_text(f"file {i}\n", encoding="utf-8")
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


def show_projects(page):
    """Return to the Projects view — a no-op click would COLLAPSE the sidebar,
    since the rail button of the view already showing is the collapse gesture."""
    page.evaluate(
        f"() => {{ const v = {PROJ_VIEW};"
        f" if (!v || v.offsetParent === null) document.querySelector('{RAIL_PROJECTS}').click(); }}"
    )
    page.wait_for_function(
        f"() => {{ const v = {PROJ_VIEW}; return !!v && v.offsetParent !== null; }}", timeout=15000
    )


def open_changes(page, slug):
    """Open a project in the Projects view, then reach its change set the way an
    operator does: by clicking the rail's Changes button."""
    show_projects(page)
    # The slug rides as an ARGUMENT, never interpolated into the source: a repo
    # registered from a Windows path carries backslashes a string literal would
    # swallow as escapes, silently opening nothing (#316).
    # `toggle` is a TOGGLE: calling it on the already-open project closes it.
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    page.evaluate(
        f"() => {{ const v = {VIEW};"
        f" if (!v || v.offsetParent === null) document.querySelector('{RAIL_CHANGES}').click(); }}"
    )
    # An Alpine x-show flip is NOT visible to the very next evaluate, so every
    # wait polls on an offsetParent-gated predicate (KNOWLEDGE.md #307/#309).
    page.wait_for_function(
        f"() => {{ const v = {VIEW}; return !!v && v.offsetParent !== null; }}", timeout=15000
    )


def wait_rows(page, n):
    page.wait_for_function(
        f"(n) => {{ const v = {VIEW}; if (!v || v.offsetParent === null) return false;"
        " return Array.from(v.querySelectorAll('.chg-row'))"
        "   .filter(e => e.offsetParent !== null).length === n; }",
        arg=n,
        timeout=20000,
    )


def groups(page):
    """The Changes view as plain data: which paths it lists, how they split
    between the two groups, and what a rename says it came from. Read inside ONE
    evaluate — the sync API cannot round-trip a DOM handle."""
    return page.evaluate(
        f"() => {{ const v = {VIEW}; if (!v) return null;"
        " const vis = (e) => e.offsetParent !== null;"
        " const rows = (sel) => Array.from(v.querySelectorAll(sel)).filter(vis);"
        " const names = rows('.chg-row')"
        "   .map(e => (e.querySelector('.chg-name') || {}).textContent.trim());"
        # A group is found through the HEADLINE above it, never by index: when a
        # clean side renders no headline the surviving group slides into slot 0,
        # so `ul[0]` would silently answer for the other side.
        # The headline's LABEL, not the head's whole text: #318 hung a
        # stage-all/unstage-all button in the same box, so `textContent` now
        # trails the button's glyph. The label lives in the head's own `<span>`;
        # the `|| e` fallback keeps this reading a bare head too.
        " const label = (e) => (e.querySelector('span') || e).textContent.trim();"
        " const group = (name) => {"
        "   const h = Array.from(v.querySelectorAll('.chg-group-head')).filter(vis)"
        "     .find(e => label(e) === name);"
        "   const ul = h && h.nextElementSibling;"
        "   return ul && vis(ul)"
        "     ? Array.from(ul.querySelectorAll('.chg-row')).filter(vis).length : 0; };"
        " return {"
        "   names: names.filter((n, i) => names.indexOf(n) === i),"
        "   staged: group('Staged Changes'),"
        "   unstaged: group('Changes'),"
        "   heads: Array.from(v.querySelectorAll('.chg-group-head')).filter(vis).map(label),"
        "   from: rows('.chg-from').map(e => e.textContent.trim()) }; }"
    )


# One project row's badge, by slug. The row is found through `.project-slug`'s
# `title` (the full slug) because the visible label is the UPPERCASED repo name.
BADGE_EXPR = (
    "(() => { const r = Array.from(document.querySelectorAll('li.project'))"
    "   .find(e => { const n = e.querySelector('.project-slug');"
    "               return n && n.getAttribute('title') === s; });"
    "  if (!r) return null; const b = r.querySelector('.chg-badge');"
    "  return b ? { shown: b.offsetParent !== null, text: b.textContent.trim() }"
    "           : { shown: false, text: null }; })()"
)


# Squeezing the sidebar in a browser test is a `--side-w` override plus a ~400ms
# wait (the `grid-template-columns` transition is 0.2s). 160px, not 320: the
# default is 300px, so any "too narrow" assertion at 320 passes vacuously.
SQUEEZED_SIDE_W = "160px"


def squeeze_read(page):
    """Toolbar/compose geometry at a 160px sidebar, restoring the width after."""
    page.evaluate(
        "(w) => document.documentElement.style.setProperty('--side-w', w)",
        arg=SQUEEZED_SIDE_W,
    )
    page.wait_for_timeout(400)
    box = page.evaluate(
        f"() => {{ const v = {VIEW}; const side = document.querySelector('.side');"
        " const r = (sel) => { const e = v.querySelector(sel);"
        "   if (!e || e.offsetParent === null) return null;"
        "   const b = e.getBoundingClientRect();"
        "   return { left: b.left, right: b.right, width: b.width, height: b.height }; };"
        " const sb = side.getBoundingClientRect();"
        " return { side: { left: sb.left, right: sb.right, width: sb.width },"
        "   toolbar: r('.chg-toolbar'), compose: r('.chg-compose'), msg: r('.chg-msg'),"
        "   sideBleed: side.scrollWidth - side.clientWidth }; }"
    )
    page.evaluate("() => document.documentElement.style.removeProperty('--side-w')")
    page.wait_for_timeout(400)
    return box


def squeezed_ok(page):
    b = squeeze_read(page)
    if not all(b[k] for k in ("toolbar", "compose", "msg")):
        return False
    # The squeeze must be REAL (the whole point of 160 over 320), the strips must
    # stay inside the sidebar's box horizontally, and the message box must keep a
    # usable height rather than collapsing to a hairline.
    return (
        b["side"]["width"] < 200
        and b["toolbar"]["right"] <= b["side"]["right"] + 0.5
        and b["compose"]["right"] <= b["side"]["right"] + 0.5
        and b["compose"]["left"] >= b["side"]["left"] - 0.5
        and b["msg"]["width"] > 0
        and b["msg"]["height"] >= 24
        and b["toolbar"]["height"] >= 24
        and b["sideBleed"] == 0
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb317_reg_")
    dir_a, dir_b, dir_c, dir_d = make_two(), make_three(), make_clean(), make_sixty()
    slug_a = register_fixture(daemon_dir, dir_a)
    slug_b = register_fixture(daemon_dir, dir_b)
    slug_c = register_fixture(daemon_dir, dir_c)
    slug_d = register_fixture(daemon_dir, dir_d)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 4", timeout=15000)

            # --- scenario 1: the rail switches the sidebar's view --------------
            rail = page.evaluate(
                "() => ({ n: document.querySelectorAll('nav.rail button').length,"
                " titles: Array.from(document.querySelectorAll('nav.rail button'))"
                "   .map(b => b.getAttribute('title')),"
                " text: (document.querySelector('nav.rail').textContent || '') })"
            )
            check(
                "the rail carries five buttons, Changes among them",
                rail["n"] == 5 and "Changes" in rail["titles"],
                f"got={rail['titles']}",
            )
            # The negative control for criterion 6: an implementation that hung a
            # roll-up badge on the rail button would print a digit HERE.
            import re as _re

            check(
                "…and no digit anywhere in the rail (no cross-repo aggregate)",
                _re.search(r"[0-9]", rail["text"]) is None,
                f"text={rail['text']!r}",
            )

            open_changes(page, slug_a)
            flip = page.evaluate(
                f"() => ({{ changes: !!{VIEW} && {VIEW}.offsetParent !== null,"
                f" projects: !!{PROJ_VIEW} && {PROJ_VIEW}.offsetParent !== null }})"
            )
            check(
                "clicking Changes shows the Changes view and hides Projects",
                flip["changes"] and not flip["projects"],
                f"got={flip}",
            )
            show_projects(page)
            back = page.evaluate(
                f"() => ({{ changes: !!{VIEW} && {VIEW}.offsetParent !== null,"
                f" projects: !!{PROJ_VIEW} && {PROJ_VIEW}.offsetParent !== null }})"
            )
            check(
                "…and clicking Projects reverses it",
                back["projects"] and not back["changes"],
                f"got={back}",
            )

            # The collapse gesture `showSideView` inherits from the pre-#317
            # `toggleSide`: the rail button of the view ALREADY showing closes the
            # sidebar. `offsetParent` cannot see this — the shell collapses with a
            # 0px grid track and `overflow: hidden` (styles.css `body
            # .side-collapsed`), never `display: none` — so the oracle is the
            # sidebar's measured WIDTH, which is what a dropped `sideOpen = true`
            # would leave at 0.
            def side_state():
                return page.evaluate(
                    "() => ({ collapsed: document.body.classList.contains('side-collapsed'),"
                    " width: document.querySelector('.side').getBoundingClientRect().width })"
                )

            before = side_state()
            page.click(RAIL_PROJECTS)  # Projects is showing → this collapses
            # The collapsed track is 0px but `.side` keeps its 1px right border,
            # so the floor is <= 1, never == 0.
            page.wait_for_function(
                "() => document.querySelector('.side').getBoundingClientRect().width <= 1"
                " && document.body.classList.contains('side-collapsed')",
                timeout=8000,
            )
            collapsed = side_state()
            check(
                "clicking the showing view's rail button collapses the sidebar",
                before["width"] > 100 and collapsed["collapsed"] and collapsed["width"] <= 1,
                f"before={before} after={collapsed}",
            )
            page.click(RAIL_CHANGES)  # the OTHER view → re-opens, showing Changes
            page.wait_for_function(
                "() => document.querySelector('.side').getBoundingClientRect().width > 100"
                " && !document.body.classList.contains('side-collapsed')",
                timeout=8000,
            )
            reopened = page.evaluate(
                f"() => ({{ width: document.querySelector('.side').getBoundingClientRect().width,"
                f" changes: !!{VIEW} && {VIEW}.offsetParent !== null }})"
            )
            check(
                "…and the other view's button re-opens it onto that view",
                reopened["width"] > 100 and reopened["changes"],
                f"got={reopened}",
            )
            show_projects(page)

            # --- scenario 2: the view is scoped to the OPEN project ------------
            open_changes(page, slug_a)
            wait_rows(page, 3)
            a_view = groups(page)
            check(
                "the open project's own paths are what the view lists",
                a_view["names"] == ["README.md", "new.txt"],
                f"got={a_view}",
            )
            check(
                "…split into its staged and unstaged sides",
                a_view["staged"] == 2 and a_view["unstaged"] == 1,
                f"got={a_view}",
            )
            # A renamed-then-modified path is ONE entry that lands in BOTH groups
            # (#315's fold), so its origin is drawn once per group — two rows.
            check(
                "…and a rename still shows where it came from",
                a_view["from"] == ["← old.txt", "← old.txt"],
                f"got={a_view['from']}",
            )

            open_changes(page, slug_b)
            wait_rows(page, 3)
            b_view = groups(page)
            check(
                "opening another project re-scopes the view to ITS paths",
                b_view["names"] == ["README.md", "one.txt", "two.txt"],
                f"got={b_view}",
            )
            # The discriminator: A and B both render 3 rows, so a view that never
            # re-scoped would still count 3 — the SPLIT is what separates them.
            check(
                "…with nothing of the previous project left in it",
                b_view["staged"] == 0 and b_view["unstaged"] == 3,
                f"got={b_view}",
            )

            # --- scenario 3: the count survives as a per-project indicator -----
            # Leg (i) — the property #297 built the section for: with B still the
            # open project and its Changes view CLOSED, B's count is on screen
            # with no click and no navigation.
            show_projects(page)
            page.wait_for_function(
                f"(s) => {{ const b = {BADGE_EXPR}; return !!b && b.shown && b.text === '3'; }}",
                arg=slug_b,
                timeout=15000,
            )
            check(
                "the open project's count is on the Projects row, no navigation needed",
                page.evaluate(f"(s) => {BADGE_EXPR}", arg=slug_b)["shown"],
                "with the Changes view closed",
            )
            # Leg (ii) — `.projects.has-open .project:not(.open)` is display:none
            # (styles.css:258, pre-existing), so the two rows are only comparable
            # with NO project open. Closing it is also what proves the badge
            # survives on a project that is merely "previously opened".
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug_b)
            page.wait_for_function(f"() => !{SH}.openSlug", timeout=15000)
            page.wait_for_function(
                f"(s) => {{ const b = {BADGE_EXPR}; return !!b && b.shown && b.text !== ''; }}",
                arg=slug_a,
                timeout=15000,
            )
            badges = page.evaluate(
                "(slugs) => {"
                " const rows = Array.from(document.querySelectorAll('li.project'));"
                " const find = (s) => rows.find(r => {"
                "   const n = r.querySelector('.project-slug');"
                "   return n && n.getAttribute('title') === s; });"
                " const read = (s) => { const r = find(s); if (!r) return null;"
                "   const b = r.querySelector('.chg-badge');"
                "   return b ? { shown: b.offsetParent !== null, text: b.textContent.trim() }"
                "            : { shown: false, text: null }; };"
                # Any element whose WHOLE text is the aggregate 2+3 — the number
                # an implementation that summed the count map would print.
                " const five = Array.from(document.querySelectorAll('*'))"
                "   .filter(e => (e.textContent || '').trim() === '5')"
                "   .map(e => e.tagName + '.' + e.className);"
                " return { a: read(slugs[0]), b: read(slugs[1]), c: read(slugs[2]), five };"
                "}",
                arg=[slug_a, slug_b, slug_c],
            )
            check(
                "the Projects row keeps the open project's count with no navigation",
                badges["a"] and badges["a"]["shown"] and badges["a"]["text"] == "2",
                f"got={badges['a']}",
            )
            check(
                "…per project, so the second one reads its own count",
                badges["b"] and badges["b"]["shown"] and badges["b"]["text"] == "3",
                f"got={badges['b']}",
            )
            check(
                "…and NO element on the page reads the cross-repo aggregate",
                badges["five"] == [],
                f"got={badges['five']}",
            )
            check(
                "a project nobody opened claims nothing at all",
                badges["c"] and not badges["c"]["shown"],
                f"got={badges['c']}",
            )

            # A failed read must never read like a clean tree.
            page.evaluate(
                "() => { const real = window.WBDaemon.observe;"
                " window.__realObserve = real;"
                " window.WBDaemon.observe = (verb, args) => verb === 'changes.list'"
                "   ? Promise.reject(new Error('refused')) : real(verb, args); }"
            )
            page.evaluate(f"(s) => {SH}.loadChanges(s)", arg=slug_a)
            page.wait_for_function(
                f"(s) => {{ const b = {BADGE_EXPR}; return !!b && b.text === '—'; }}",
                arg=slug_a,
                timeout=15000,
            )
            failed = page.evaluate(f"(s) => {BADGE_EXPR}", arg=slug_a)
            check(
                "a failed read shows an em dash, never a quiet zero",
                failed and failed["text"] == "—",
                f"got={failed}",
            )
            page.evaluate(
                "() => { window.WBDaemon.observe = window.__realObserve;"
                " delete window.__realObserve; }"
            )

            # --- scenario 4: the diff tab is unchanged by the promotion --------
            open_changes(page, slug_a)
            wait_rows(page, 3)
            page.evaluate(
                f"() => {{ const v = {VIEW};"
                " const row = Array.from(v.querySelectorAll('.chg-row'))"
                "   .filter(e => e.offsetParent !== null)"
                "   .find(e => (e.querySelector('.chg-name') || {}).textContent.trim() === 'README.md');"
                " row.click(); }"
            )
            diff_id = f"diff:{slug_a}:README.md"
            page.wait_for_function(
                f"(want) => {SH}.tabs.map(t => t.id).join('|') === want",
                arg=f"consoles|{diff_id}",
                timeout=20000,
            )
            check(
                "clicking a changed path still opens the diff as a canvas tab",
                page.evaluate(f"() => {SH}.tabs.map(t => t.id)") == ["consoles", diff_id],
                f"got={page.evaluate(f'() => {SH}.tabs.map(t => t.id)')}",
            )
            # A diff editor exposes no readable renderSideBySide, so the two pane
            # classes are the oracle (KNOWLEDGE.md #311).
            page.wait_for_function(
                "() => document.querySelectorAll('.original-in-monaco-diff-editor').length === 1"
                " && document.querySelectorAll('.modified-in-monaco-diff-editor').length === 1",
                timeout=20000,
            )
            panes = page.evaluate(
                "() => document.querySelectorAll('.original-in-monaco-diff-editor')"
                ".length + document.querySelectorAll('.modified-in-monaco-diff-editor').length"
            )
            check("…with both of its Monaco panes mounted", panes == 2, f"got={panes}")

            # --- scenario 5: the accordion is GONE, not left standing ----------
            gone = page.evaluate(
                "() => ({ secs: document.querySelectorAll('.changes-sec').length,"
                " nested: document.querySelectorAll('li.project .changes-list').length,"
                " views: document.querySelectorAll('.changes-view .changes-list').length })"
            )
            check(
                "no accordion section is left in the sidebar",
                gone["secs"] == 0 and gone["nested"] == 0,
                f"got={gone}",
            )
            # …and the list did not vanish along with it: the assertion above must
            # not pass merely because the whole surface disappeared.
            check(
                "…because the list moved into the rail view, not because it went away",
                gone["views"] == 1,
                f"got={gone}",
            )

            # --- scenarios 6-7: the layout budget, at two widths ---------------
            open_changes(page, slug_d)
            wait_rows(page, 60)
            for label, size in (
                ("desktop", {"width": 1440, "height": 900}),
                ("phone", {"width": 390, "height": 844}),
            ):
                page.set_viewport_size(size)
                # the shell's grid-template-columns transition is 0.2s
                page.wait_for_timeout(400)
                # Vertical containment reads FLOATS: integer scrollWidth cannot
                # see a sub-pixel overflow (#315's HIGH defect hid behind that).
                box = page.evaluate(
                    f"() => {{ const v = {VIEW};"
                    " const r = (sel) => { const e = (sel === '.side' ? document : v)"
                    "     .querySelector(sel);"
                    "   if (!e || e.offsetParent === null) return null;"
                    "   const b = e.getBoundingClientRect();"
                    "   return { top: b.top, bottom: b.bottom, height: b.height }; };"
                    " const list = v.querySelector('.changes-list');"
                    " return { side: r('.side'), toolbar: r('.chg-toolbar'),"
                    "   compose: r('.chg-compose'), msg: r('.chg-msg'),"
                    "   list: r('.changes-list'),"
                    "   clientHeight: list.clientHeight, scrollHeight: list.scrollHeight,"
                    "   scrollWidth: list.scrollWidth, clientWidth: list.clientWidth }; }"
                )
                ok = all(box[k] for k in ("side", "toolbar", "compose", "msg", "list"))
                check(
                    f"[{label}] the toolbar stays inside the sidebar",
                    ok and box["toolbar"]["bottom"] <= box["side"]["bottom"],
                    f"got={box}",
                )
                check(
                    f"[{label}] the message box stays inside the sidebar",
                    ok and box["compose"]["bottom"] <= box["side"]["bottom"],
                    f"toolbar/compose vs side: {box}",
                )
                check(
                    f"[{label}] the file list is still usable, and scrolls its 60 rows",
                    ok
                    and box["clientHeight"] >= 120
                    and box["scrollHeight"] > box["clientHeight"],
                    f"clientHeight={box['clientHeight']} scrollHeight={box['scrollHeight']}",
                )
                # A strip squeezed to a hairline still satisfies `bottom <=
                # side.bottom`, so the containment checks above need a floor.
                check(
                    f"[{label}] …without squeezing the toolbar or the message box flat",
                    ok
                    and box["toolbar"]["height"] >= 24
                    and box["msg"]["height"] >= 24,
                    f"toolbar={box['toolbar']} msg={box['msg']}",
                )
                check(
                    f"[{label}] …with no horizontal bleed",
                    box["scrollWidth"] == box["clientWidth"],
                    f"scrollWidth={box['scrollWidth']} clientWidth={box['clientWidth']}",
                )
                # Neither viewport changes the sidebar's own WIDTH: `--side-w` is a
                # fixed 300px track and styles.css has no `@media`, so the two legs
                # above vary only the vertical budget. The toolbar and the message
                # box are horizontal strips, so their real risk is a narrow
                # sidebar — measured here with the repo's own squeeze technique
                # (wb_changes_315.py), at 160px, which IS narrower than the default.
                check(
                    f"[{label}] the toolbar and message box hold at a 160px sidebar",
                    squeezed_ok(page),
                    f"got={squeeze_read(page)}",
                )
                page.screenshot(
                    path=os.path.join(SHOT_DIR, f"317-changes-rail-{label}-2026-07-25.png")
                )

            info("fixtures", f"a={dir_a} b={dir_b} c={dir_c} d={dir_d}")
            info("slugs", f"a={slug_a} b={slug_b} d={slug_d}")
            check("the page threw nothing", not thrown, f"pageerrors={thrown}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The floor is the ACTUAL count, not a loose lower bound: a slack floor
    # lets assertions vanish silently and still print the banner.
    ok = all(results) and len(results) >= 35
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CHANGES RAIL LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
