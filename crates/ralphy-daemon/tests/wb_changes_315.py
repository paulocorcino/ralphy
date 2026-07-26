"""#315 browser acceptance: the Changes list as a source-control panel.

One Playwright pass over a REAL daemon proving the list splits into a staged and
an unstaged group, that a staged-then-modified path renders in both, that each
row reads as a base name with its directory subordinate after it, and that the
directory — never the file name — is what truncates in a narrow sidebar.

Scenario 1  the two group headlines read exactly ["Staged Changes", "Changes"]
Scenario 2  a file staged and then modified again renders once under each
Scenario 3  a deep row splits into `readme.md` + a dimmer, smaller
            `docs/deep/nested` after it; the full path stays the hover title
Scenario 4  squeezed sidebar: the directory clips, the file name does not
Scenario 5  a clean tree renders NO group headline (not two empty ones)
Scenario 6  the badge still counts PATHS: 4, while 5 rows are on screen
Scenario 7  a failed `changes.list` read empties both groups behind the `—`

Boots a Localhost daemon on 7415 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/315-changes-groups-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_changes_315.py   (exit 0 = all pass)
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

PORT = 7415
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_changes_315.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

# The sidebar width at which the deep row is genuinely too narrow. Measured on
# this host: at the default 300px (and at 320px) `docs/deep/nested` fits with
# room to spare, so a truncation assertion there would pass vacuously.
SQUEEZED_SIDE_W = "160px"

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
    empty = tempfile.mkdtemp(prefix="wb315_empty_")
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


def make_fixture_repo(tag, dirt):
    """A committed git repo, then `dirt` applied on top. `.gitignore` hides
    `.ralphy/`, so the fixture's change count is exactly what `dirt` made."""
    d = tempfile.mkdtemp(prefix=f"wb315_{tag}_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text(f"# {tag}\n\nThe #315 changes fixture repo.\n", encoding="utf-8")
    (p / "docs" / "deep" / "nested").mkdir(parents=True)
    (p / "docs" / "deep" / "nested" / "readme.md").write_text("nested\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb315@example.com"],
        ["git", "config", "user.name", "wb315"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    dirt(p, d)
    return d


def four_paths_five_rows(p, d):
    """4 changed paths rendering 5 rows: `both.txt` is staged AND then edited
    again, so it belongs to both groups while counting once."""
    (p / "docs" / "deep" / "nested" / "readme.md").write_text("nested, edited\n", encoding="utf-8")  # .M
    (p / "added.txt").write_text("staged\n", encoding="utf-8")
    (p / "both.txt").write_text("staged\n", encoding="utf-8")
    subprocess.run(["git", "add", "added.txt", "both.txt"], cwd=d, check=True, capture_output=True)
    (p / "both.txt").write_text("staged\nand edited again\n", encoding="utf-8")  # AM
    (p / "untracked.txt").write_text("loose\n", encoding="utf-8")  # ?


def clean(p, d):
    pass


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


# #317 promoted the Changes section out of the project accordion into a rail
# VIEW of the sidebar, and moved the count badge onto the Projects row. The two
# now live in DIFFERENT sidebar views, so a reader of one has to be standing in
# that view — hence `in_projects`.
VISIBLE_SECS = (
    "Array.from(document.querySelectorAll('li.project.open .chg-badge'))"
    ".filter(e => e.offsetParent !== null)"
)
# The Changes surface. Scoped to the open project by construction, so there is
# one of it — but still `x-show`-gated, hence the offsetParent check.
OPEN_LI = (
    "(() => { const v = document.querySelector('.changes-view');"
    " return v && v.offsetParent !== null ? v : null; })()"
)


def show_view(page, view):
    """Clicking the rail button of the view already showing COLLAPSES the
    sidebar, so every switch is guarded on that view's own visibility."""
    page.evaluate(
        "(v) => { const el = document.querySelector('.' + v + '-view');"
        " if (!el || el.offsetParent === null)"
        "   document.querySelector(`nav.rail button[title=\"${v === 'changes' ? 'Changes' : 'Projects'}\"]`)"
        "     .click(); }",
        arg=view,
    )
    page.wait_for_function(
        "(v) => { const el = document.querySelector('.' + v + '-view');"
        " return !!el && el.offsetParent !== null; }",
        arg=view,
        timeout=15000,
    )


def in_projects(page, fn):
    """Run a badge read in the Projects view, then restore the caller's view."""
    prev = page.evaluate(f"() => {SH}.sideView")
    show_view(page, "projects")
    try:
        return fn()
    finally:
        if prev == "changes":
            show_view(page, "changes")


def badge_text(page):
    return in_projects(
        page,
        lambda: page.evaluate(
            f"() => {{ const els = {VISIBLE_SECS};"
            " if (els.length > 1) return 'MULTI';"
            " return els.length ? els[0].textContent.trim() : null; }"
        ),
    )


def wait_badge(page, expected, timeout=15000):
    in_projects(
        page,
        lambda: page.wait_for_function(
            f"(want) => {{ const els = {VISIBLE_SECS};"
            " return els.length === 1 && els[0].textContent.trim() === want; }",
            arg=expected,
            timeout=timeout,
        ),
    )


def open_project(page, slug, expected):
    show_view(page, "projects")
    # `toggle` is a TOGGLE: calling it on the already-open project closes it.
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    wait_badge(page, expected)


def wait_row_count(page, expected, timeout=8000):
    # Gate on VISIBLE rows (offsetParent), never the raw querySelectorAll count:
    # an Alpine `x-show` flip is not visible to the very next evaluate, so a
    # plain count check resolves BEFORE the flip (handoffs.md #307/#309).
    page.wait_for_function(
        f"(want) => {{ const li = {OPEN_LI};"
        " return !!li && Array.from(li.querySelectorAll('.chg-row')).filter(r => r.offsetParent !== null).length === want; }",
        arg=expected,
        timeout=timeout,
    )


def wait_head_count(page, expected, timeout=8000):
    page.wait_for_function(
        f"(want) => {{ const li = {OPEN_LI};"
        " return !!li && Array.from(li.querySelectorAll('.chg-group-head')).filter(h => h.offsetParent !== null).length === want; }",
        arg=expected,
        timeout=timeout,
    )


def groups(page):
    """[{head, paths}] over the VISIBLE headlines and the list each one owns.
    Plain data only — Playwright's sync `evaluate` cannot round-trip a DOM
    handle."""
    return page.evaluate(
        f"() => {{ const li = {OPEN_LI};"
        " const kids = Array.from(li.querySelector('.changes-list').children);"
        " const out = [];"
        " kids.forEach((k, i) => {"
        "   if (!k.classList.contains('chg-group-head') || k.offsetParent === null) return;"
        "   const ul = kids[i + 1];"
        "   const rows = ul ? Array.from(ul.querySelectorAll('.chg-row')).filter(r => r.offsetParent !== null) : [];"
        # The headline's LABEL, not the head's whole text: #318 hung a
        # stage-all/unstage-all button in the same box, so `k.textContent`
        # now trails the button's glyph. The label lives in the head's own
        # `<span>`; the `|| k` fallback keeps this reading a bare head too.
        "   const label = (k.querySelector('span') || k).textContent.trim();"
        "   out.push({ head: label,"
        "              paths: rows.map(r => { const d = r.querySelector('.chg-dir');"
        "                const dir = d && d.offsetParent !== null ? d.textContent.trim() : '';"
        "                const name = r.querySelector('.chg-name').textContent.trim();"
        "                return dir ? dir + '/' + name : name; }) });"
        " });"
        " return out; }"
    )


# Measured FRACTIONALLY, never from `scrollWidth`/`clientWidth`: those are
# integer-rounded, so a name laid out at 61.734px holding 61.766px of text reads
# 62 == 62 and "uncut" passes on a row Chrome actually paints as `readme.…`.
# The natural width comes from an off-screen span carrying the same computed
# font, so the oracle never hard-codes this host's metrics.
NATURAL_WIDTH = (
    "const natural = (el) => { const s = document.createElement('span');"
    "  const cs = getComputedStyle(el);"
    "  s.style.position = 'absolute'; s.style.left = '-9999px';"
    "  s.style.whiteSpace = 'nowrap'; s.style.fontStyle = cs.fontStyle;"
    "  s.style.fontVariant = cs.fontVariant; s.style.fontWeight = cs.fontWeight;"
    "  s.style.fontSize = cs.fontSize; s.style.fontFamily = cs.fontFamily;"
    "  s.style.letterSpacing = cs.letterSpacing; s.textContent = el.textContent;"
    "  document.body.appendChild(s);"
    "  const w = s.getBoundingClientRect().width; s.remove(); return w; };"
)


def deep_row(page):
    """Texts, geometry and computed style of the `docs/deep/nested/readme.md`
    row, read in ONE evaluate. `*W` are laid-out widths, `*Nat` the unconstrained
    natural width of the same text — a box narrower than its natural width is a
    box whose text is ellipsised."""
    return page.evaluate(
        f"() => {{ const li = {OPEN_LI};"
        " const r = Array.from(li.querySelectorAll('.chg-row')).filter(e => e.offsetParent !== null)"
        "   .find(e => e.querySelector('.chg-name').textContent.trim() === 'readme.md');"
        " if (!r) return null;"
        " const n = r.querySelector('.chg-name'), d = r.querySelector('.chg-dir');"
        " const list = li.querySelector('.changes-list');"
        " const cs = (e) => getComputedStyle(e);"
        " const px = (v) => parseFloat(v);"
        f" {NATURAL_WIDTH}"
        " return { name: n.textContent.trim(), dir: d.textContent.trim(),"
        "          title: r.getAttribute('title'),"
        "          nameX: n.getBoundingClientRect().x, dirX: d.getBoundingClientRect().x,"
        "          nameColor: cs(n).color, dirColor: cs(d).color,"
        "          nameSize: px(cs(n).fontSize), dirSize: px(cs(d).fontSize),"
        "          nameW: n.getBoundingClientRect().width, nameNat: natural(n),"
        "          dirW: d.getBoundingClientRect().width, dirNat: natural(d),"
        "          listScroll: list.scrollWidth, listClient: list.clientWidth }; }"
    )


def set_side_width(page, width):
    page.evaluate(
        "(w) => document.documentElement.style.setProperty('--side-w', w)", arg=width
    )
    page.wait_for_timeout(400)  # the grid-template-columns transition is 0.2s


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb315_reg_")
    dir_a = make_fixture_repo("mixed", four_paths_five_rows)
    dir_b = make_fixture_repo("clean", clean)
    slug_a = register_fixture(daemon_dir, dir_a)
    slug_b = register_fixture(daemon_dir, dir_b)

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
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 2", timeout=15000)

            open_project(page, slug_a, "4")
            show_view(page, "changes")
            wait_row_count(page, 5)

            # --- scenario 1: two headlines, in reading order -------------------
            gs = groups(page)
            check(
                "the list reads as two groups: Staged Changes, then Changes",
                [g["head"] for g in gs] == ["Staged Changes", "Changes"],
                f"got={[g['head'] for g in gs]}",
            )

            # --- scenario 2: staged-then-modified is in BOTH -------------------
            staged = gs[0]["paths"] if gs else []
            unstaged = gs[1]["paths"] if len(gs) > 1 else []
            check(
                "the staged group holds exactly the two staged paths",
                staged == ["added.txt", "both.txt"],
                f"got={staged}",
            )
            check(
                "the unstaged group holds exactly the three worktree paths",
                unstaged == ["both.txt", "docs/deep/nested/readme.md", "untracked.txt"],
                f"got={unstaged}",
            )
            check(
                "both.txt renders exactly once under each headline",
                staged.count("both.txt") == 1 and unstaged.count("both.txt") == 1,
                f"staged={staged} unstaged={unstaged}",
            )

            # --- scenario 3: base name first, directory subordinate ------------
            row = deep_row(page)
            check("the deep row is on screen", row is not None)
            if row is not None:
                check(
                    "the row leads with the base name",
                    row["name"] == "readme.md",
                    f"got={row['name']!r}",
                )
                check(
                    "its directory follows, without the base name",
                    row["dir"] == "docs/deep/nested",
                    f"got={row['dir']!r}",
                )
                check(
                    "the name is painted before the directory",
                    row["nameX"] < row["dirX"],
                    f"nameX={row['nameX']} dirX={row['dirX']}",
                )
                check(
                    "the full repo-relative path stays the hover title",
                    row["title"] == "docs/deep/nested/readme.md",
                    f"got={row['title']!r}",
                )
                check(
                    "the directory is visually subordinate: dimmer AND smaller",
                    row["dirColor"] != row["nameColor"] and row["dirSize"] < row["nameSize"],
                    f"name=({row['nameColor']}, {row['nameSize']}) dir=({row['dirColor']}, {row['dirSize']})",
                )
                info(
                    "at the default sidebar width nothing is cut",
                    f"nameW={row['nameW']:.3f}/{row['nameNat']:.3f} dirW={row['dirW']:.3f}/{row['dirNat']:.3f}",
                )

            page.screenshot(path=os.path.join(SHOT_DIR, "315-changes-groups-2026-07-25.png"))

            # --- scenario 4: squeezed, the DIRECTORY is what truncates ---------
            set_side_width(page, SQUEEZED_SIDE_W)
            # Poll for the clip rather than trusting the transition timeout: on a
            # slower host or a different font stack a fixed wait can read the
            # pre-squeeze layout and the negative control goes vacuous.
            page.wait_for_function(
                f"() => {{ const li = {OPEN_LI}; if (!li) return false;"
                " const r = Array.from(li.querySelectorAll('.chg-row')).filter(e => e.offsetParent !== null)"
                "   .find(e => e.querySelector('.chg-name').textContent.trim() === 'readme.md');"
                " if (!r) return false; const d = r.querySelector('.chg-dir');"
                f" {NATURAL_WIDTH}"
                " return d.getBoundingClientRect().width < natural(d) - 2; }",
                timeout=8000,
            )
            squeezed = deep_row(page)
            check(
                f"at a {SQUEEZED_SIDE_W} sidebar the file name is never the part cut",
                # Fractional, with NO tolerance upward: a name box even a
                # fraction narrower than its text is a name Chrome ellipsises.
                squeezed is not None and squeezed["nameW"] >= squeezed["nameNat"] - 0.02,
                f"nameW={squeezed and round(squeezed['nameW'], 3)} nameNat={squeezed and round(squeezed['nameNat'], 3)}",
            )
            check(
                "…and the directory is what clips (the row really was too narrow)",
                squeezed is not None and squeezed["dirW"] < squeezed["dirNat"] - 2,
                f"dirW={squeezed and round(squeezed['dirW'], 3)} dirNat={squeezed and round(squeezed['dirNat'], 3)}",
            )
            check(
                "…and holding the name intact does not bleed the row past the sidebar",
                # The control against "satisfy 'never cut' by overflowing": a
                # bare `flex-shrink: 0` passes both checks above while pushing
                # the list wider than its container.
                squeezed is not None and squeezed["listScroll"] <= squeezed["listClient"],
                f"listScroll={squeezed and squeezed['listScroll']} listClient={squeezed and squeezed['listClient']}",
            )
            # Freezing the name only stays safe because `.chg-row` clips: swap a
            # 200-char name in and the list must still not scroll sideways. The
            # fixture cannot carry such a path (Windows MAX_PATH), so it is
            # injected into the rendered row and reverted.
            bleed = page.evaluate(
                f"() => {{ const li = {OPEN_LI};"
                " const r = Array.from(li.querySelectorAll('.chg-row')).filter(e => e.offsetParent !== null)"
                "   .find(e => e.querySelector('.chg-name').textContent.trim() === 'readme.md');"
                " const n = r.querySelector('.chg-name'); const was = n.textContent;"
                " n.textContent = 'x'.repeat(200) + '.md';"
                " const list = li.querySelector('.changes-list');"
                " const side = document.querySelector('.side');"
                " const out = { listScroll: list.scrollWidth, listClient: list.clientWidth,"
                "               sideScroll: side.scrollWidth, sideClient: side.clientWidth,"
                "               nameW: n.getBoundingClientRect().width };"
                " n.textContent = was; return out; }"
            )
            check(
                "a 200-character file name is clipped, not bled out of the sidebar",
                # `.chg-row` itself DOES overflow internally — that is the clip
                # doing its job. What must not move is the list or the sidebar.
                bleed["listScroll"] <= bleed["listClient"]
                and bleed["sideScroll"] <= bleed["sideClient"],
                f"got={bleed}",
            )
            set_side_width(page, "300px")

            # --- scenario 6: the badge counts paths, not rows ------------------
            # (asserted here, on the same open project, before switching away)
            visible_rows = page.evaluate(
                f"() => {{ const li = {OPEN_LI};"
                " return Array.from(li.querySelectorAll('.chg-row')).filter(r => r.offsetParent !== null).length; }"
            )
            check(
                "the badge counts 4 paths while 5 rows are on screen",
                badge_text(page) == "4" and visible_rows == 5,
                f"badge={badge_text(page)!r} rows={visible_rows}",
            )

            # --- scenario 7a/7b: a failed read empties BOTH groups -------------
            # Run on the POPULATED project, with 2 headlines and 5 rows already
            # on screen: on the clean fixture the groups are `[]` before the read
            # ever fails, so deleting the clear from `app.js` would stay green.
            for label, stub in (
                (
                    "a rejected changes.list",
                    "() => { window.__realObserve = window.WBDaemon.observe;"
                    " window.WBDaemon.observe = (verb, payload) => verb === 'changes.list'"
                    "   ? Promise.reject(new Error('transport down'))"
                    "   : window.__realObserve(verb, payload); }",
                ),
                (
                    "a non-ok changes.list reply",
                    "() => { window.__realObserve = window.WBDaemon.observe;"
                    " window.WBDaemon.observe = (verb, payload) => verb === 'changes.list'"
                    "   ? Promise.resolve({ status: 'error', message: 'query read failed' })"
                    "   : window.__realObserve(verb, payload); }",
                ),
            ):
                # Reload the real rows first, so each leg starts from a populated
                # list rather than inheriting the previous leg's emptied one.
                page.evaluate(f"() => {SH}.loadChanges('{slug_a}')")
                wait_badge(page, "4")
                wait_row_count(page, 5)
                page.evaluate(stub)
                page.evaluate(f"() => {SH}.loadChanges('{slug_a}')")
                wait_badge(page, "—")
                # The badge and the group `x-show`s flip in separate Alpine
                # effects, so reading heads right after the badge catches the
                # pre-flip DOM (KNOWLEDGE.md #307/#309). Wait for the flip; if
                # the groups were NOT cleared this times out, which is the
                # failure this leg exists to catch.
                try:
                    wait_head_count(page, 0)
                except Exception as exc:  # noqa: BLE001 - reported as a check
                    print(f"[INFO] {label}: group headlines never cleared ({type(exc).__name__})", flush=True)
                # The badge and the groups now live in DIFFERENT sidebar views
                # (#317), so they are two reads: the count from the Projects row,
                # the emptied groups from the Changes view.
                failed = in_projects(
                    page,
                    lambda: page.evaluate(
                        f"() => {{ const el = {VISIBLE_SECS}[0];"
                        " return el ? { text: el.textContent.trim(),"
                        "               title: el.getAttribute('title') } : null; }"
                    ),
                )
                failed = dict(
                    failed or {"text": None, "title": None},
                    **page.evaluate(
                        f"() => {{ const li = {OPEN_LI}; if (!li) return {{ heads: -1, rows: -1 }};"
                        " return { heads: Array.from(li.querySelectorAll('.chg-group-head')).filter(h => h.offsetParent !== null).length,"
                        "          rows: Array.from(li.querySelectorAll('.chg-row')).filter(r => r.offsetParent !== null).length }; }"
                    ),
                )
                check(
                    f"{label} reads as `—` and explains itself",
                    failed["text"] == "—" and "could not read changes" in (failed["title"] or ""),
                    f"got={failed}",
                )
                check(
                    f"{label} leaves no group behind the `—`",
                    failed["heads"] == 0 and failed["rows"] == 0,
                    f"got={failed}",
                )
                page.evaluate("() => { window.WBDaemon.observe = window.__realObserve; }")

            # --- scenario 5: a clean tree renders NO headline ------------------
            open_project(page, slug_b, "0")
            show_view(page, "changes")
            # Positive control FIRST: without it `heads == 0` holds whether or
            # not the click ever expanded the section (the Alpine x-show race).
            page.wait_for_function(
                f"() => {{ const li = {OPEN_LI}; if (!li) return false;"
                " const list = li.querySelector('.changes-list');"
                " return !!list && list.offsetParent !== null; }",
                timeout=8000,
            )
            wait_head_count(page, 0)
            clean_state = page.evaluate(
                f"() => {{ const li = {OPEN_LI};"
                " return { listVisible: li.querySelector('.changes-list').offsetParent !== null,"
                "          heads: Array.from(li.querySelectorAll('.chg-group-head')).filter(h => h.offsetParent !== null).length,"
                "          rows: Array.from(li.querySelectorAll('.chg-row')).filter(r => r.offsetParent !== null).length }; }"
            )
            check(
                "a clean tree renders no group headline, not two empty ones",
                clean_state["listVisible"]
                and clean_state["heads"] == 0
                and clean_state["rows"] == 0,
                f"got={clean_state}",
            )
            check("the clean badge still reads 0", badge_text(page) == "0")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 15
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CHANGES GROUPS LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
