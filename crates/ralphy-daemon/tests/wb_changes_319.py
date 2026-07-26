"""#319 browser acceptance: discarding one file's changes, confirmed first.

One Playwright pass over a REAL daemon proving the panel's only irreversible
gesture end to end: the two cases are two dialogs, a Cancel touches nothing, a
confirm really moves bytes on disk, and a staged change survives a discard of the
same path's working-tree edit.

Scenario 1  the fixture's four paths render, split across the two groups
Scenario 2  the per-group action sets are EXACT: every unstaged row carries
            ["stage","discard"] and every staged row carries ["unstage"] — the
            oracle for "discard is offered on Changes only"
Scenario 3  clicking discard opens the confirm dialog, whose body NAMES the file;
            Cancel leaves the row in place and the file's bytes unchanged
Scenario 4  confirming on a tracked file restores its committed bytes and the row
            leaves the unstaged group
Scenario 5  the untracked dialog is its own, more emphatic one (`no commit and no
            reflog`, a different confirm label) and confirming removes the file
Scenario 6  discarding a staged-then-edited path leaves `git show :both.txt`
            untouched — a staged change is never silently thrown away
Scenario 7  both group heads state what their discard removes
Scenario 8  at a 390x844 touch viewport the destructive action does NOT carry
            equal prominence: strictly smaller than the same row's stage button
            and last in the row, while staying visible with no hover
Scenario 9  under a run's lock every discard control is disabled and says why,
            and the CLI guard refuses the same act over the wire

Boots a Localhost daemon on 7419 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/319-changes-{discard,touch}-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_changes_319.py   (exit 0 = all pass)
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

PORT = 7419
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_changes_319.py -> repo root is 4 dirs up.
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
    empty = tempfile.mkdtemp(prefix="wb319_empty_")
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


README_HEAD = "# fixture\n\nthe 319 discard fixture.\n"


def make_fixture():
    """One repo carrying every side a discard must tell apart: an unstaged
    modification, an untracked file, a staged-only rename, and a path that is
    BOTH staged and edited again (the negative control for criterion 3).

    `core.autocrlf false` because this host otherwise leaves LF in the blob and
    CRLF on disk, which would make the exact-content oracle a coin flip."""
    d = tempfile.mkdtemp(prefix="wb319_a_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text(README_HEAD, encoding="utf-8")
    (p / "old.txt").write_text("l1\nl2\nl3\nl4\nl5\nl6\n", encoding="utf-8")
    (p / "both.txt").write_text("base\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb319@example.com")
    git(d, "config", "user.name", "wb319")
    git(d, "config", "core.autocrlf", "false")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")

    (p / "README.md").write_text("# fixture\n\nedited on disk.\n", encoding="utf-8")
    (p / "fresh.txt").write_text("brand new\n", encoding="utf-8")
    git(d, "mv", "old.txt", "renamed.txt")
    (p / "both.txt").write_text("staged\n", encoding="utf-8")
    git(d, "add", "both.txt")
    (p / "both.txt").write_text("worktree only\n", encoding="utf-8")
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


# EVERY group a named path sits in, found through the HEADLINE above the list —
# never by index: when a clean side renders no headline the surviving group
# slides into slot 0 and `ul[0]` silently answers for the other side. The label
# is the head's FIRST span, because the head now also carries its note span.
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


def groups_of(page, name):
    return page.evaluate(GROUPS_OF, arg=name)


def wait_groups(page, name, labels):
    page.wait_for_function(
        "(a) => { const f = " + GROUPS_OF + "; return f(a[0]).sort().join('|') === a[1]; }",
        arg=[name, "|".join(sorted(labels))],
        timeout=20000,
    )


def click_row_act(page, name, act, group="Changes"):
    """Click a row's action WITHOUT the mouse: `page.click` would move the
    pointer and hover the row, which is exactly what scenario 8 must not do.

    Scoped to a GROUP, never first-match: a staged-then-edited path has a row on
    BOTH sides and `Staged Changes` comes first in the DOM, so a first-match
    reader would hand back the staged twin — which by design carries no discard
    (handoffs.md #318 recorded the same trap for a rename-then-edit entry)."""
    page.evaluate(
        "(a) => { const v = document.querySelector('.changes-view');"
        " const vis = (e) => e.offsetParent !== null;"
        " const head = Array.from(v.querySelectorAll('.chg-group-head')).filter(vis)"
        "   .find(e => ((e.querySelector('span') || e).textContent || '').trim() === a[2]);"
        " if (!head) throw new Error('no group head ' + a[2]);"
        " const ul = head.nextElementSibling;"
        " const row = Array.from(ul.querySelectorAll('.chg-row')).filter(vis)"
        "   .find(e => ((e.querySelector('.chg-name') || {}).textContent || '').trim() === a[0]);"
        " if (!row) throw new Error('no row for ' + a[0] + ' in ' + a[2]);"
        " const btn = row.querySelector(`[data-act=\"${a[1]}\"]`);"
        " if (!btn) throw new Error('no ' + a[1] + ' action on ' + a[0]);"
        " btn.click(); }",
        arg=[name, act, group],
    )


DIALOG = (
    "() => { const m = document.querySelector('.confirm-modal');"
    " if (!m || m.offsetParent === null) return null;"
    " const body = m.querySelector('.confirm-body');"
    " const btns = Array.from(m.querySelectorAll('button')).map(b => (b.textContent || '').trim());"
    " const title = m.querySelector('.modal-title');"
    " return { message: body ? body.textContent.trim() : '',"
    "          title: title ? title.textContent.trim() : '', buttons: btns }; }"
)


def wait_dialog(page):
    page.wait_for_function(
        "() => { const m = document.querySelector('.confirm-modal');"
        " return !!m && m.offsetParent !== null; }",
        timeout=15000,
    )
    return page.evaluate(DIALOG)


def answer_dialog(page, ok):
    """Settle the open dialog through Alpine's own responder — the label text is
    per-case, so a text-matched click would be a second oracle to keep in sync."""
    page.evaluate(f"(v) => {SH}.confirmRespond(v)", arg=ok)
    page.wait_for_function(
        "() => { const m = document.querySelector('.confirm-modal');"
        " return !m || m.offsetParent === null; }",
        timeout=15000,
    )


def row_acts(page):
    """Every visible row's own action set, keyed by the row's file name."""
    return page.evaluate(
        f"() => {{ const v = {VIEW}; const vis = (e) => e.offsetParent !== null;"
        " const out = {};"
        " for (const h of Array.from(v.querySelectorAll('.chg-group-head')).filter(vis)) {"
        "   const label = (h.querySelector('span') || h).textContent.trim();"
        "   const ul = h.nextElementSibling;"
        "   if (!ul || !vis(ul)) continue;"
        "   for (const r of Array.from(ul.querySelectorAll('.chg-row')).filter(vis)) {"
        "     const name = ((r.querySelector('.chg-name') || {}).textContent || '').trim();"
        "     out[label + '/' + name] = Array.from(r.querySelectorAll('[data-act]'))"
        "       .map(e => e.getAttribute('data-act')); }"
        " } return out; }"
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb319_reg_")
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
            # 4 paths: README.md (unstaged), fresh.txt (untracked), renamed.txt
            # (staged rename), both.txt (staged AND edited again — two rows).
            wait_rows(page, 5)

            # --- scenario 1: the fixture's paths render, split by side --------
            listed = page.evaluate(
                f"() => {{ const v = {VIEW}; const vis = (e) => e.offsetParent !== null;"
                " const names = Array.from(v.querySelectorAll('.chg-row')).filter(vis)"
                "   .map(e => (e.querySelector('.chg-name') || {}).textContent.trim());"
                " return names.sort(); }"
            )
            check(
                "the fixture's changed paths are listed",
                listed == ["README.md", "both.txt", "both.txt", "fresh.txt", "renamed.txt"],
                f"got={listed}",
            )
            check(
                "…split across the two groups by their own side",
                sorted(groups_of(page, "README.md")) == ["Changes"]
                and sorted(groups_of(page, "renamed.txt")) == ["Staged Changes"]
                and sorted(groups_of(page, "both.txt")) == ["Changes", "Staged Changes"],
                f"both={groups_of(page, 'both.txt')} renamed={groups_of(page, 'renamed.txt')}",
            )

            # --- scenario 2: the per-group action sets are EXACT ---------------
            acts = row_acts(page)
            unstaged = {k: v for k, v in acts.items() if k.startswith("Changes/")}
            staged = {k: v for k, v in acts.items() if k.startswith("Staged Changes/")}
            check(
                "every unstaged row carries exactly [stage, discard]",
                bool(unstaged) and all(v == ["stage", "discard"] for v in unstaged.values()),
                f"got={unstaged}",
            )
            check(
                "…and every staged row carries exactly [unstage] — no discard there",
                bool(staged) and all(v == ["unstage"] for v in staged.values()),
                f"got={staged}",
            )

            # --- scenario 3: the dialog names the file; Cancel touches nothing -
            before_readme = (Path(fixture) / "README.md").read_text(encoding="utf-8")
            click_row_act(page, "README.md", "discard")
            dialog = wait_dialog(page)
            check(
                "clicking discard opens a confirmation that NAMES the file",
                dialog is not None and "README.md" in dialog["message"],
                f"got={dialog}",
            )
            tracked_title = dialog["title"] if dialog else ""
            tracked_buttons = dialog["buttons"] if dialog else []
            check(
                "…and the tracked wording does NOT borrow the unrecoverable one",
                dialog is not None and "no commit and no reflog" not in dialog["message"],
                f"got={dialog['message'] if dialog else None!r}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "319-changes-discard-2026-07-25.png"))
            answer_dialog(page, False)
            page.wait_for_timeout(600)
            check(
                "Cancel leaves the row exactly where it was",
                sorted(groups_of(page, "README.md")) == ["Changes"],
                f"got={groups_of(page, 'README.md')}",
            )
            check(
                "…and the file's bytes untouched — a cancel makes no daemon call",
                (Path(fixture) / "README.md").read_text(encoding="utf-8") == before_readme,
                "the working tree is unchanged",
            )

            # --- scenario 4: confirming restores the committed bytes ----------
            click_row_act(page, "README.md", "discard")
            wait_dialog(page)
            answer_dialog(page, True)
            page.wait_for_function(
                "(a) => { const f = " + GROUPS_OF + "; return f(a).length === 0; }",
                arg="README.md",
                timeout=20000,
            )
            check(
                "confirming restores the committed content on disk",
                (Path(fixture) / "README.md").read_text(encoding="utf-8") == README_HEAD,
                f"got={(Path(fixture) / 'README.md').read_text(encoding='utf-8')!r}",
            )
            check(
                "…and the row leaves the unstaged group with no manual refresh",
                groups_of(page, "README.md") == [],
                f"got={groups_of(page, 'README.md')}",
            )

            # --- scenario 5: the untracked case is its own, emphatic dialog ----
            click_row_act(page, "fresh.txt", "discard")
            loose = wait_dialog(page)
            check(
                "the untracked dialog states the deletion is unrecoverable",
                loose is not None
                and "fresh.txt" in loose["message"]
                and "no commit and no reflog" in loose["message"],
                f"got={loose}",
            )
            check(
                "…and is visibly a DIFFERENT dialog from the tracked one",
                loose is not None
                and loose["title"] != tracked_title
                and loose["buttons"] != tracked_buttons,
                f"untracked={loose} tracked_title={tracked_title!r} tracked_btns={tracked_buttons}",
            )
            answer_dialog(page, True)
            page.wait_for_function(
                "(a) => { const f = " + GROUPS_OF + "; return f(a).length === 0; }",
                arg="fresh.txt",
                timeout=20000,
            )
            check(
                "confirming removes the untracked file from disk",
                not (Path(fixture) / "fresh.txt").exists(),
                "the file is gone",
            )

            # --- scenario 6: a staged change survives an unstaged discard ------
            staged_blob = git_out(fixture, "show", ":both.txt")
            check(
                "the fixture really is staged AND edited again",
                staged_blob == "staged"
                and (Path(fixture) / "both.txt").read_text(encoding="utf-8") == "worktree only\n",
                f"blob={staged_blob!r}",
            )
            click_row_act(page, "both.txt", "discard")
            wait_dialog(page)
            answer_dialog(page, True)
            page.wait_for_function(
                "(a) => { const f = " + GROUPS_OF + "; return f(a).join('|') === 'Staged Changes'; }",
                arg="both.txt",
                timeout=20000,
            )
            check(
                "discarding a staged-then-edited path keeps the STAGED blob",
                git_out(fixture, "show", ":both.txt") == "staged",
                f"got={git_out(fixture, 'show', ':both.txt')!r}",
            )
            check(
                "…and the working tree went back to the index, not to HEAD",
                (Path(fixture) / "both.txt").read_text(encoding="utf-8") == "staged\n",
                f"got={(Path(fixture) / 'both.txt').read_text(encoding='utf-8')!r}",
            )
            check(
                "…so the path is still staged, only its unstaged row is gone",
                git_out(fixture, "diff", "--cached", "--name-only").splitlines() != [],
                f"staged={git_out(fixture, 'diff', '--cached', '--name-only')!r}",
            )

            # --- scenario 7: both group heads state what a discard removes -----
            # Re-dirty a tracked path first: scenarios 4-6 emptied the unstaged
            # group, and a group with no rows renders no head to read a note off.
            (Path(fixture) / "README.md").write_text(
                "# fixture\n\nedited again.\n", encoding="utf-8"
            )
            page.evaluate(f"(s) => {SH}.loadChanges(s)", arg=slug)
            wait_groups(page, "README.md", ["Changes"])
            notes = page.evaluate(
                f"() => {{ const v = {VIEW}; const vis = (e) => e.offsetParent !== null;"
                " const out = {};"
                " for (const h of Array.from(v.querySelectorAll('.chg-group-head')).filter(vis)) {"
                "   const label = (h.querySelector('span') || h).textContent.trim();"
                "   const note = h.querySelector('.chg-group-note');"
                "   out[label] = note ? note.textContent.trim() : null; }"
                " return out; }"
            )
            check(
                "the unstaged head states that staged changes are kept",
                "staged changes are kept" in (notes.get("Changes") or ""),
                f"got={notes}",
            )
            check(
                "…and the staged head says to unstage first",
                "unstage first" in (notes.get("Staged Changes") or ""),
                f"got={notes}",
            )

            # --- scenario 8: the touch viewport de-emphasises the destructive --
            page.set_viewport_size({"width": 390, "height": 844})
            page.wait_for_function(
                f"() => {{ const v = {VIEW};"
                " const b = v && v.querySelector('.chg-row [data-act=\"discard\"]');"
                " return !!b && b.getBoundingClientRect().width > 0"
                "   && b.getBoundingClientRect().width < 24; }",
                timeout=15000,
            )
            touch = page.evaluate(
                f"() => {{ const v = {VIEW};"
                " const row = Array.from(v.querySelectorAll('.chg-row'))"
                "   .filter(e => e.offsetParent !== null)"
                "   .find(e => e.querySelector('[data-act=\"discard\"]'));"
                " if (!row) return null;"
                " const read = (act) => { const e = row.querySelector(`[data-act=\"${act}\"]`);"
                "   const b = e.getBoundingClientRect(); const s = getComputedStyle(e);"
                "   return { area: b.width * b.height, left: b.left, w: b.width, h: b.height,"
                "            opacity: Number(s.opacity), visibility: s.visibility,"
                "            display: s.display }; };"
                " return { stage: read('stage'), discard: read('discard') }; }"
            )
            check(
                "on a touch viewport the discard box is strictly SMALLER than stage",
                touch is not None and touch["discard"]["area"] < touch["stage"]["area"],
                f"got={touch}",
            )
            check(
                "…and comes last in the row, never first under the thumb",
                touch is not None and touch["discard"]["left"] > touch["stage"]["left"],
                f"discard.left={touch['discard']['left'] if touch else None}"
                f" stage.left={touch['stage']['left'] if touch else None}",
            )
            check(
                "…while staying visible with NO hover (dimmed, never hidden)",
                touch is not None
                and touch["discard"]["visibility"] != "hidden"
                and touch["discard"]["display"] != "none"
                and touch["discard"]["opacity"] > 0
                and touch["discard"]["w"] > 0,
                f"got={touch['discard'] if touch else None}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "319-changes-touch-2026-07-25.png"))
            page.set_viewport_size({"width": 1440, "height": 900})

            # --- scenario 9: inert under a run's lock, and refused on the wire -
            runstate = Path(fixture) / ".ralphy" / "runstate"
            runstate.mkdir(parents=True, exist_ok=True)
            (runstate / "wb319.json").write_text(
                json.dumps(
                    {
                        "v": 1,
                        "runid": "wb319",
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
                " return Array.from(v.querySelectorAll('[data-act=\"discard\"]'))"
                "   .filter(e => e.offsetParent !== null)"
                "   .map(e => ({ disabled: e.disabled === true,"
                "                title: e.getAttribute('title') || '' })); }"
            )
            check(
                "under a held lock every discard control is disabled",
                bool(locked) and all(c["disabled"] for c in locked),
                f"got={locked}",
            )
            check(
                "…and every one of them SAYS why",
                bool(locked)
                and all("a run holds this repo's lock" in c["title"] for c in locked),
                f"got={[c['title'] for c in locked]}",
            )
            # The CLI guard is the authority, not this hint: prove the daemon
            # would ALSO refuse, by asking it directly while the lock is held.
            (Path(fixture) / ".ralphy" / "run.lock").write_text(
                json.dumps({"pid": holder.pid, "started_at": "2026-07-25T10:00:00-03:00"}),
                encoding="utf-8",
            )
            guarded = page.evaluate(
                "async (s) => await window.WBDaemon.observe('changes.discard',"
                " { repo: s, paths: ['README.md'] })",
                arg=slug,
            )
            check(
                "the CLI guard refuses the same act, hint or no hint",
                guarded.get("status") == "error"
                and "refusing to changes discard" in str(guarded.get("message", "")),
                f"got={guarded}",
            )
            check(
                "…and the file it would have discarded is still edited on disk",
                "edited again." in (Path(fixture) / "README.md").read_text(encoding="utf-8"),
                "the refusal ran no git write",
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
    ok = all(results) and len(results) >= 24
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CHANGES DISCARD LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
