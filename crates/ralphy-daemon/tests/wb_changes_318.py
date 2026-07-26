"""#318 browser acceptance: a commit composed without a terminal.

One Playwright pass over a REAL daemon proving the write gesture end to end:
stage a path, unstage it, refuse honestly, commit with a message, and go inert
under a run's lock while reading and diffing keep working.

Scenario 1  the fixture's paths render, split across the two groups
Scenario 2  clicking a row's `data-act="stage"` moves that path into Staged
            Changes with NO manual refresh, and the `data-act="unstage"` twin
            moves it back
Scenario 3  every visible row action is reachable with NO hover: computed
            `opacity === "1"`, `visibility !== "hidden"`, and a rect at least
            24x24 — measured with no mouse move performed in the page
Scenario 4  a refused action reports the failure and leaves the list TRUTHFUL:
            with `changes.stage` stubbed to answer `{status:"error",…}` the row
            stays in its original group (no optimistic move)
Scenario 5  the commit box names the branch; typing a message and clicking it
            records a real commit in the fixture repo and empties the staged
            group
Scenario 6  a group stage over a rename-then-edit (`RM`) entry stages EVERY
            path in the group — the regression oracle for the abort `git add`
            performs on one unmatched pathspec — and unstage-all still sends the
            rename's old path
Scenario 7  with a live-pid run snapshot in the repo, EVERY write control is
            disabled and its title says why, while the row count, the count
            badge and click-to-diff all keep working

Boots a Localhost daemon on 7418 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/318-changes-{write,locked}-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_changes_318.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7418
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_changes_318.py -> repo root is 4 dirs up.
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
    empty = tempfile.mkdtemp(prefix="wb318_empty_")
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


def git_out(cwd, *args):
    return subprocess.run(
        ["git", *args], cwd=cwd, check=True, capture_output=True, encoding="utf-8"
    ).stdout.strip()


def make_fixture():
    """One repo carrying every side the panel can act on: an unstaged
    modification, an untracked file, and a STAGED rename (whose old path the
    daemon must also be able to send, or `restore --staged` leaves half of it)."""
    d = tempfile.mkdtemp(prefix="wb318_a_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text("# fixture\n\nthe 318 write fixture.\n", encoding="utf-8")
    (p / "old.txt").write_text("l1\nl2\nl3\nl4\nl5\nl6\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb318@example.com")
    git(d, "config", "user.name", "wb318")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")

    (p / "README.md").write_text("# fixture\n\nedited on disk.\n", encoding="utf-8")
    (p / "fresh.txt").write_text("brand new\n", encoding="utf-8")
    git(d, "mv", "old.txt", "renamed.txt")
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
    """Open a project, then reach its change set the way an operator does."""
    show_projects(page)
    # The slug rides as an ARGUMENT, never interpolated: a repo registered from a
    # Windows path carries backslashes a string literal would swallow (#316).
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


# Which group a named path currently sits in, found through the HEADLINE above
# the list — never by index: when a clean side renders no headline the surviving
# group slides into slot 0 and `ul[0]` silently answers for the other side.
GROUP_OF = (
    "(name) => { const v = document.querySelector('.changes-view'); if (!v) return null;"
    " const vis = (e) => e.offsetParent !== null;"
    " const heads = Array.from(v.querySelectorAll('.chg-group-head')).filter(vis);"
    " for (const h of heads) {"
    "   const label = (h.querySelector('span') || h).textContent.trim();"
    "   const ul = h.nextElementSibling;"
    "   if (!ul || !vis(ul)) continue;"
    "   const hit = Array.from(ul.querySelectorAll('.chg-row')).filter(vis)"
    "     .some(r => ((r.querySelector('.chg-name') || {}).textContent || '').trim() === name);"
    "   if (hit) return label;"
    " } return null; }"
)


# EVERY group a named path sits in. `GROUP_OF` answers the FIRST one, which is
# wrong for a rename-then-edit (`RM`) entry: it is in both, and `Staged Changes`
# comes first in the DOM, so the first-match reader can never see the other side.
GROUPS_OF = (
    "(name) => { const v = document.querySelector('.changes-view'); if (!v) return [];"
    " const vis = (e) => e.offsetParent !== null;"
    " const out = [];"
    " for (const h of Array.from(v.querySelectorAll('.chg-group-head')).filter(vis)) {"
    "   const label = (h.querySelector('span') || h).textContent.trim();"
    "   const ul = h.nextElementSibling;"
    "   if (!ul || !vis(ul)) continue;"
    "   if (Array.from(ul.querySelectorAll('.chg-row')).filter(vis)"
    "     .some(r => ((r.querySelector('.chg-name') || {}).textContent || '').trim() === name))"
    "     out.push(label);"
    " } return out; }"
)


def group_of(page, name):
    return page.evaluate(GROUP_OF, arg=name)


def groups_of(page, name):
    return page.evaluate(GROUPS_OF, arg=name)


def wait_groups(page, name, labels):
    page.wait_for_function(
        "(a) => { const f = " + GROUPS_OF + "; return f(a[0]).sort().join('|') === a[1]; }",
        arg=[name, "|".join(sorted(labels))],
        timeout=20000,
    )


def wait_group(page, name, label):
    page.wait_for_function(
        "(a) => { const f = " + GROUP_OF + "; return f(a[0]) === a[1]; }",
        arg=[name, label],
        timeout=20000,
    )


def click_group_act(page, label, act):
    """Click a group headline's all-at-once action, also without the mouse."""
    page.evaluate(
        "(a) => { const v = document.querySelector('.changes-view');"
        " const head = Array.from(v.querySelectorAll('.chg-group-head'))"
        "   .filter(e => e.offsetParent !== null)"
        "   .find(e => ((e.querySelector('span') || e).textContent || '').trim() === a[0]);"
        " if (!head) throw new Error('no group head ' + a[0]);"
        " const btn = head.querySelector(`[data-act=\"${a[1]}\"]`);"
        " if (!btn) throw new Error('no ' + a[1] + ' on ' + a[0]);"
        " btn.click(); }",
        arg=[label, act],
    )


def click_row_act(page, name, act):
    """Click a row's action WITHOUT the mouse: `page.click` would move the
    pointer and hover the row, which is exactly what scenario 3 must not do."""
    page.evaluate(
        "(a) => { const v = document.querySelector('.changes-view');"
        " const row = Array.from(v.querySelectorAll('.chg-row'))"
        "   .filter(e => e.offsetParent !== null)"
        "   .find(e => ((e.querySelector('.chg-name') || {}).textContent || '').trim() === a[0]);"
        " if (!row) throw new Error('no row for ' + a[0]);"
        " const btn = row.querySelector(`[data-act=\"${a[1]}\"]`);"
        " if (!btn) throw new Error('no ' + a[1] + ' action on ' + a[0]);"
        " btn.click(); }",
        arg=[name, act],
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb318_reg_")
    fixture = make_fixture()
    slug = register_fixture(daemon_dir, fixture)
    branch = git_out(fixture, "rev-parse", "--abbrev-ref", "HEAD")

    proc = launch(daemon_dir)
    # A process whose pid is ALIVE, so the snapshot reader classifies the run as
    # live rather than sweeping it as an orphan (ADR-0047 §7).
    holder = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(600)"])
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
            page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)

            open_changes(page, slug)
            # 3 paths: README.md (unstaged), fresh.txt (untracked), renamed.txt
            # (staged rename) — one row each.
            wait_rows(page, 3)

            # --- scenario 1: the fixture's paths render, split by side --------
            listed = page.evaluate(
                f"() => {{ const v = {VIEW}; const vis = (e) => e.offsetParent !== null;"
                " const names = Array.from(v.querySelectorAll('.chg-row')).filter(vis)"
                "   .map(e => (e.querySelector('.chg-name') || {}).textContent.trim());"
                " return names.sort(); }"
            )
            check(
                "the fixture's three changed paths are listed",
                listed == ["README.md", "fresh.txt", "renamed.txt"],
                f"got={listed}",
            )
            check(
                "…split across the two groups by their own side",
                group_of(page, "README.md") == "Changes"
                and group_of(page, "fresh.txt") == "Changes"
                and group_of(page, "renamed.txt") == "Staged Changes",
                f"README={group_of(page, 'README.md')} renamed={group_of(page, 'renamed.txt')}",
            )

            # --- scenario 2: stage / unstage move the row, with no refresh ----
            click_row_act(page, "README.md", "stage")
            wait_group(page, "README.md", "Staged Changes")
            check(
                "clicking a row's stage action moves it to Staged Changes",
                group_of(page, "README.md") == "Staged Changes",
                "with no manual refresh",
            )
            check(
                "…and the index really moved, not just the DOM",
                "README.md" in git_out(fixture, "diff", "--cached", "--name-only"),
                f"staged={git_out(fixture, 'diff', '--cached', '--name-only')!r}",
            )
            click_row_act(page, "README.md", "unstage")
            wait_group(page, "README.md", "Changes")
            check(
                "…and the unstage twin moves it back",
                group_of(page, "README.md") == "Changes",
                f"staged={git_out(fixture, 'diff', '--cached', '--name-only')!r}",
            )
            check(
                "…leaving the file itself on disk, edited",
                (Path(fixture) / "README.md").read_text(encoding="utf-8").strip().endswith("edited on disk."),
                "unstage touches the index, never the working tree",
            )

            # --- scenario 3: reachable with NO hover (the touch criterion) ----
            # Nothing above moved the mouse — every click went through the
            # element's own `.click()`.
            acts = page.evaluate(
                f"() => {{ const v = {VIEW};"
                " return Array.from(v.querySelectorAll('.chg-row [data-act]'))"
                "   .filter(e => e.offsetParent !== null)"
                "   .map(e => { const s = getComputedStyle(e); const b = e.getBoundingClientRect();"
                "     return { act: e.getAttribute('data-act'), opacity: s.opacity,"
                "              visibility: s.visibility, w: b.width, h: b.height }; }); }"
            )
            check(
                "every row carries its own action",
                len(acts) == 3,
                f"got={[a['act'] for a in acts]}",
            )
            check(
                "…fully opaque and visible with no hover",
                bool(acts)
                and all(a["opacity"] == "1" and a["visibility"] != "hidden" for a in acts),
                f"got={acts}",
            )
            check(
                "…and at least 24x24, so a finger can hit it",
                bool(acts) and all(a["w"] >= 24 and a["h"] >= 24 for a in acts),
                f"got={[(a['w'], a['h']) for a in acts]}",
            )
            group_acts = page.evaluate(
                f"() => {{ const v = {VIEW};"
                " return Array.from(v.querySelectorAll('.chg-group-head [data-act]'))"
                "   .filter(e => e.offsetParent !== null).map(e => e.getAttribute('data-act')); }"
            )
            check(
                "…and each group head carries its all-at-once twin",
                sorted(group_acts) == ["stage-all", "unstage-all"],
                f"got={group_acts}",
            )

            # --- scenario 4: a refused action leaves the list truthful --------
            page.evaluate(
                "() => { const real = window.WBDaemon.observe;"
                " window.__realObserve = real;"
                " window.WBDaemon.observe = (verb, args) => verb === 'changes.stage'"
                "   ? Promise.resolve({ status: 'error', message: 'boom' }) : real(verb, args); }"
            )
            click_row_act(page, "README.md", "stage")
            page.wait_for_function(f"() => ({SH}.runsActionMsg || '').includes('boom')", timeout=15000)
            flash = page.evaluate(
                f"() => ({{ state: {SH}.runsActionMsg,"
                " dom: (document.querySelector('.runs-action-status') || {}).textContent || '' })"
            )
            check(
                "a refused action reports the daemon's own message",
                "boom" in flash["state"] and "boom" in flash["dom"],
                f"got={flash}",
            )
            # The list must be re-read, not moved optimistically: the row is
            # still where it was, and so is the index.
            page.wait_for_timeout(600)
            check(
                "…and the row stays in its ORIGINAL group (no optimistic move)",
                group_of(page, "README.md") == "Changes",
                f"got={group_of(page, 'README.md')}",
            )
            check(
                "…with the repo's index untouched by the refused click",
                "README.md" not in git_out(fixture, "diff", "--cached", "--name-only"),
                f"staged={git_out(fixture, 'diff', '--cached', '--name-only')!r}",
            )
            page.evaluate(
                "() => { window.WBDaemon.observe = window.__realObserve;"
                " delete window.__realObserve; }"
            )

            # --- scenario 5: the commit box names the branch, and commits -----
            label = page.evaluate(
                f"() => {{ const b = {VIEW}.querySelector('.chg-commit');"
                " return b ? b.textContent.trim() : null; }"
            )
            check(
                "the commit button names the branch the commit will land on",
                label == f"Commit to {branch}",
                f"got={label!r} branch={branch!r}",
            )
            # A commit with no MESSAGE must not be offered at all — asserted
            # before anything is typed, so an always-enabled button reds here.
            empty = page.evaluate(
                f"() => {{ const b = {VIEW}.querySelector('.chg-commit');"
                " return { disabled: b.disabled, title: b.getAttribute('title') || '' }; }"
            )
            check(
                "with no message typed, Commit is disabled and says what is missing",
                empty["disabled"] and "message" in empty["title"],
                f"got={empty}",
            )
            click_row_act(page, "README.md", "stage")
            wait_group(page, "README.md", "Staged Changes")
            before_commits = git_out(fixture, "rev-list", "--count", "HEAD")

            # A REFUSED commit must report the failure and keep the message —
            # without this leg, a UI that cleared `commitMsg` on every attempt
            # would satisfy the "cleared on success only" check below.
            page.evaluate(
                "() => { const real = window.WBDaemon.observe;"
                " window.__realObserve = real;"
                " window.WBDaemon.observe = (verb, args) => verb === 'changes.commit'"
                "   ? Promise.resolve({ status: 'error', message: 'hook said no' }) : real(verb, args); }"
            )
            page.evaluate(f"() => {{ {SH}.commitMsg = 'feat: composed in the panel'; }}")
            page.wait_for_function(
                f"() => {{ const b = {VIEW}.querySelector('.chg-commit'); return b && !b.disabled; }}",
                timeout=15000,
            )
            page.evaluate(f"() => {VIEW}.querySelector('.chg-commit').click()")
            page.wait_for_function(
                f"() => ({SH}.runsActionMsg || '').includes('hook said no')", timeout=15000
            )
            check(
                "a refused commit reports the failure",
                "hook said no" in page.evaluate(f"() => {SH}.runsActionMsg"),
                "the daemon's own message, not a generic one",
            )
            check(
                "…and does NOT eat the message the operator typed",
                page.evaluate(f"() => {SH}.commitMsg") == "feat: composed in the panel",
                f"got={page.evaluate(f'() => {SH}.commitMsg')!r}",
            )
            check(
                "…and records no commit",
                git_out(fixture, "rev-list", "--count", "HEAD") == before_commits,
                f"count={git_out(fixture, 'rev-list', '--count', 'HEAD')}",
            )
            page.evaluate(
                "() => { window.WBDaemon.observe = window.__realObserve;"
                " delete window.__realObserve; }"
            )
            page.wait_for_function(
                f"() => {{ const b = {VIEW}.querySelector('.chg-commit'); return b && !b.disabled; }}",
                timeout=15000,
            )
            # The evidence frame: rows with their own actions, both groups, and
            # the branch-named Commit button ENABLED over a typed message —
            # captured before the click that empties it.
            page.screenshot(path=os.path.join(SHOT_DIR, "318-changes-write-2026-07-25.png"))
            page.evaluate(f"() => {VIEW}.querySelector('.chg-commit').click()")
            page.wait_for_function(
                f"() => {{ const f = {GROUP_OF}; return f('README.md') === null"
                f" && f('renamed.txt') === null; }}",
                timeout=20000,
            )
            after_commits = git_out(fixture, "rev-list", "--count", "HEAD")
            check(
                "clicking Commit records a real commit in the repo",
                int(after_commits) == int(before_commits) + 1,
                f"before={before_commits} after={after_commits}",
            )
            check(
                "…carrying the message the operator typed",
                git_out(fixture, "log", "-1", "--format=%s") == "feat: composed in the panel",
                f"got={git_out(fixture, 'log', '-1', '--format=%s')!r}",
            )
            check(
                "…and the staged group empties without a manual refresh",
                group_of(page, "README.md") is None and group_of(page, "renamed.txt") is None,
                "both staged paths left the panel",
            )
            check(
                "…with the message box cleared only on success",
                page.evaluate(f"() => {SH}.commitMsg") == "",
                "a refused commit must not eat the message",
            )
            # `fresh.txt` was never staged, so it survives the commit — which is
            # what keeps the next scenario measuring a populated list.
            wait_rows(page, 1)

            # --- scenario 6: a group stage over a rename-then-edit entry ------
            # The regression oracle for the self-review's HIGH-1. A `2 RM`
            # record (renamed in the index, then edited on disk) lands in BOTH
            # groups carrying its ORIGINAL path. Sending that old path on the
            # STAGE direction is fatal to `git add`, which aborts the WHOLE
            # invocation — so before the fix this click staged NOTHING, not even
            # the innocent sibling.
            (Path(fixture) / "old2.txt").write_text("l1\nl2\nl3\nl4\nl5\nl6\n", encoding="utf-8")
            git(fixture, "add", "old2.txt")
            git(fixture, "commit", "-m", "add old2")
            git(fixture, "mv", "old2.txt", "renamed2.txt")
            (Path(fixture) / "renamed2.txt").write_text(
                "l1\nl2\nl3\nl4\nl5\nl6\nl7\n", encoding="utf-8"
            )
            page.evaluate(f"(s) => {SH}.loadChanges(s)", arg=slug)
            wait_groups(page, "renamed2.txt", ["Changes", "Staged Changes"])
            check(
                "a rename-then-edit entry really lands in BOTH groups",
                sorted(groups_of(page, "renamed2.txt")) == ["Changes", "Staged Changes"],
                f"got={groups_of(page, 'renamed2.txt')} — the fixture must exercise the abort case",
            )
            page.evaluate(f"() => {{ {SH}.runsActionMsg = ''; }}")
            click_group_act(page, "Changes", "stage-all")
            wait_groups(page, "fresh.txt", ["Staged Changes"])
            staged_now = git_out(fixture, "diff", "--cached", "--name-only").split("\n")
            check(
                "stage-all stages EVERY path in the group, rename included",
                "fresh.txt" in staged_now and "renamed2.txt" in staged_now,
                f"staged={staged_now}",
            )
            check(
                "…and no failure was flashed",
                page.evaluate(f"() => {SH}.runsActionMsg") == "",
                f"flash={page.evaluate(f'() => {SH}.runsActionMsg')!r}",
            )
            # …and the reverse direction still sends the old path, which is the
            # half `git restore --staged` genuinely needs.
            click_group_act(page, "Staged Changes", "unstage-all")
            wait_groups(page, "fresh.txt", ["Changes"])
            check(
                "unstage-all empties the index, both halves of the rename with it",
                git_out(fixture, "diff", "--cached", "--name-only") == "",
                f"staged={git_out(fixture, 'diff', '--cached', '--name-only')!r}",
            )
            wait_rows(page, 3)

            # --- scenario 7: inert under a run's lock, still readable ---------
            runstate = Path(fixture) / ".ralphy" / "runstate"
            runstate.mkdir(parents=True, exist_ok=True)
            (runstate / "wb318.json").write_text(
                json.dumps(
                    {
                        "v": 1,
                        "runid": "wb318",
                        "pid": holder.pid,
                        "title": "holding the lock",
                        "repo": slug,
                        "branch": branch,
                    }
                ),
                encoding="utf-8",
            )
            page.evaluate(f"() => {SH}.hydrateRuns()")
            page.wait_for_function(
                f"(s) => (({SH}.runsByProject[s]) || []).length === 1", arg=slug, timeout=20000
            )
            page.wait_for_function(f"() => {SH}.writeLocked() === true", timeout=15000)
            locked = page.evaluate(
                f"() => {{ const v = {VIEW};"
                " const sel = '[data-act^=\"stage\"], [data-act^=\"unstage\"], .chg-commit, .chg-msg';"
                " return Array.from(v.querySelectorAll(sel))"
                "   .filter(e => e.offsetParent !== null)"
                "   .map(e => ({ what: e.getAttribute('data-act') || e.className,"
                "                disabled: e.disabled === true,"
                "                title: e.getAttribute('title') || '' })); }"
            )
            check(
                "under a held lock every visible write control is present",
                # The EXACT measured set, not a `>= n` floor: a loose floor lets
                # a regression hide exactly one control and still satisfy every
                # `all()` below it.
                sorted(c["what"] for c in locked)
                == ["chg-commit", "chg-msg", "stage", "stage", "stage", "stage-all"],
                f"got={sorted(c['what'] for c in locked)}",
            )
            check(
                "…and every one of them is disabled",
                bool(locked) and all(c["disabled"] for c in locked),
                f"got={locked}",
            )
            check(
                "…and every one of them SAYS why",
                bool(locked)
                and all("a run holds this repo's lock" in c["title"] for c in locked),
                f"got={[c['title'] for c in locked]}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "318-changes-locked-2026-07-25.png"))
            # …while reading keeps working: the rows, the count badge and the
            # diff are untouched by the lock.
            still = page.evaluate(
                f"() => {{ const v = {VIEW};"
                " const rows = Array.from(v.querySelectorAll('.chg-row'))"
                "   .filter(e => e.offsetParent !== null).length;"
                " const badge = v.querySelector('.side-head .count');"
                " return { rows, badge: badge ? badge.textContent.trim() : null }; }"
            )
            check(
                "…while the file list still renders its rows",
                still["rows"] == 3,
                f"got={still}",
            )
            check(
                "…and the count badge still reads a number, not an em dash",
                still["badge"] is not None and still["badge"].isdigit(),
                f"got={still['badge']!r}",
            )
            page.evaluate(
                f"() => {{ const v = {VIEW};"
                " const row = Array.from(v.querySelectorAll('.chg-row'))"
                "   .filter(e => e.offsetParent !== null)[0];"
                " row.click(); }"
            )
            page.wait_for_function(
                "() => document.querySelectorAll('.diff-viewer').length >= 1", timeout=20000
            )
            check(
                "…and clicking a row still opens its diff",
                page.evaluate("() => document.querySelectorAll('.diff-viewer').length") >= 1,
                "reading is never gated on the lock",
            )
            # The CLI guard is the authority, not this hint: prove the daemon
            # would ALSO refuse, by asking it directly while the lock is held.
            (Path(fixture) / ".ralphy" / "run.lock").write_text(
                json.dumps({"pid": holder.pid, "started_at": "2026-07-25T10:00:00-03:00"}),
                encoding="utf-8",
            )
            guarded = page.evaluate(
                "async (s) => await window.WBDaemon.observe('changes.stage',"
                " { repo: s, paths: ['fresh.txt'] })",
                arg=slug,
            )
            check(
                "the CLI guard refuses the same act, hint or no hint",
                guarded.get("status") == "error"
                and "refusing to changes stage" in str(guarded.get("message", "")),
                f"got={guarded}",
            )

            info("fixture", fixture)
            info("slug", slug)
            check("the page threw nothing", not thrown, f"pageerrors={thrown}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)
        holder.terminate()

    # The floor is the ACTUAL measured count, not a loose lower bound: a slack
    # floor lets assertions vanish silently and still print the banner.
    ok = all(results) and len(results) >= 35
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CHANGES WRITE LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
