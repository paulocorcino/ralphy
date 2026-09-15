"""#405 browser acceptance: creating a worktree from the branch picker.

One Playwright pass over a REAL daemon proving the create path end to end
(ADR-0063 §2): the picker's Worktrees section offers a "+ new worktree from
<branch>" row even when the project has no worktrees yet; typing a name and
Enter in that row runs `worktree.add`, the listing refreshes with the new row,
the worktree exists on disk with its base recorded, and a refusal is surfaced
verbatim inside the modal.

Scenario 1  the daemon is listening
Scenario 2  on a fixture with NO worktrees the picker renders the section with
            zero `.worktree-item` rows and one laid-out `.worktree-create` row
            whose label is `+ new worktree from main` and whose note reads
            `gitignored files are not copied`
Scenario 3  typing `wt-new` + Enter yields two rows (`primary`, `wt-new ·
            wt-new`), `<fixture>/.ralphy/worktrees/wt-new` is a directory,
            `git config branch.wt-new.base` is `main`, the field clears, no
            error is set
Scenario 4  typing `taken` (an existing branch) + Enter leaves two rows, sets
            `branchError` to a message containing `already exists`, rendered
            inside the modal in `.worktree-create-error`; no directory; the
            name stays in the field
Scenario 5  typing `a/b` + Enter is refused with `single path segment`; no
            directory
Scenario 6  no page errors

Boots a Localhost daemon on 7452 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/405-worktree-create-2026-09-15.png.
Run: python crates/ralphy-daemon/tests/wb_worktree_405.py   (exit 0 = all pass)
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

PORT = 7452
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_worktree_405.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
NAME_INPUT = ".branch-modal .worktree-create-name"

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
    empty = tempfile.mkdtemp(prefix="wb405_empty_")
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
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, encoding="utf-8").stdout.strip()


def seed(parent_prefix, name):
    """A committed git repo on `main` at a CHOSEN directory name, `.ralphy/`
    gitignored so a worktree under it never dirties the primary tree, plus a
    branch `taken` that is NOT checked out anywhere (the `already exists`
    refusal, distinct from `is checked out at`)."""
    d = Path(tempfile.mkdtemp(prefix=parent_prefix)) / name
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "README.md").write_text(f"# {name}\n\nThe #405 picker fixture repo.\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb405@example.com")
    git(d, "config", "user.name", "wb405")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    git(d, "branch", "taken")
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
    # after any assets/ui edit or the browser loads yesterday's picker.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# The rendered Worktrees rows, laid-out ones only.
ROWS_EXPR = (
    "() => Array.from(document.querySelectorAll('.branch-modal .worktree-item'))"
    "  .filter(e => e.offsetParent !== null && e.clientWidth > 0)"
    "  .map(e => ({ name: e.querySelector('.worktree-name').textContent.trim(),"
    "    branch: e.querySelector('.worktree-branch').textContent.trim() }))"
)
ROW_COUNT_IS = (
    "(n) => Array.from(document.querySelectorAll('.branch-modal .worktree-item'))"
    "  .filter(e => e.offsetParent !== null && e.clientWidth > 0).length === n"
)


def open_picker(page, slug):
    # The slug rides as an ARGUMENT, never interpolated: a repo registered from
    # a Windows path carries backslashes a string literal would swallow (#316).
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    page.wait_for_function(
        "() => { const c = document.querySelector('li.project.open .files-sec .branch-chip');"
        "  return !!c && c.offsetParent !== null && c.clientWidth > 0; }",
        timeout=15000,
    )
    page.evaluate("() => document.querySelector('li.project.open .files-sec .branch-chip').click()")
    page.wait_for_function(f"() => {SH}.branchOpen === true", timeout=10000)
    # The listing arrives by round trip: gate on the reply having landed, not
    # on the modal being open (an open modal samples an empty section).
    page.wait_for_function(f"() => {SH}.branchModal.checkouts !== null", timeout=15000)


def close_picker(page, slug):
    page.evaluate(f"() => {{ {SH}.branchOpen = false; }}")
    page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
    page.wait_for_function(f"() => {SH}.openSlug === null", timeout=10000)


def type_name_and_enter(page, name):
    page.fill(NAME_INPUT, name)
    page.press(NAME_INPUT, "Enter")


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb405_reg_")
    fixture = seed("wb405_", "plain")
    slug = register_fixture(daemon_dir, str(fixture))

    proc = launch(daemon_dir)
    try:
        # --- scenario 1 ---------------------------------------------------
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
            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('li.project'))"
                "  .filter(e => e.offsetParent !== null).length === 1",
                timeout=15000,
            )

            # --- scenario 2: the create row on a fixture with no worktrees ----
            open_picker(page, slug)
            page.wait_for_function(
                "() => { const c = document.querySelector('.branch-modal .worktree-create');"
                "  return !!c && c.offsetParent !== null && c.clientWidth > 0; }",
                timeout=15000,
            )
            sec = page.evaluate(
                "() => ({ secs: document.querySelectorAll('.branch-modal .worktree-sec').length,"
                "  items: Array.from(document.querySelectorAll('.branch-modal .worktree-item'))"
                "    .filter(e => e.offsetParent !== null && e.clientWidth > 0).length,"
                "  create: (() => { const c = document.querySelector('.branch-modal .worktree-create');"
                "    return !!c && c.offsetParent !== null && c.clientWidth > 0; })(),"
                "  label: (document.querySelector('.branch-modal .worktree-create-label') || {}).textContent,"
                "  note: (document.querySelector('.branch-modal .worktree-create-note') || {}).textContent })"
            )
            check(
                "with no worktrees the section renders zero rows and one laid-out create row",
                sec["secs"] == 1 and sec["items"] == 0 and sec["create"] is True,
                "got={}".format(sec),
            )
            check(
                "the create row's label names the current branch",
                (sec["label"] or "").strip() == "+ new worktree from main",
                "got={!r}".format(sec["label"]),
            )
            check(
                "the create row's note is exactly `gitignored files are not copied`",
                (sec["note"] or "").strip() == "gitignored files are not copied",
                "got={!r}".format(sec["note"]),
            )

            # --- scenario 3: a name + Enter creates the worktree --------------
            type_name_and_enter(page, "wt-new")
            page.wait_for_function(ROW_COUNT_IS, arg=2, timeout=20000)
            rows = page.evaluate(ROWS_EXPR)
            check(
                "after Enter the listing refreshes to primary + wt-new · wt-new",
                rows == [{"name": "primary", "branch": "main"}, {"name": "wt-new", "branch": "wt-new"}],
                "got={}".format(rows),
            )
            wt_new = fixture / ".ralphy" / "worktrees" / "wt-new"
            check("the worktree directory exists on disk", wt_new.is_dir(), "path={}".format(wt_new))
            base = git(fixture, "config", "branch.wt-new.base") if wt_new.is_dir() else None
            check("branch.wt-new.base records the current branch", base == "main", "got={!r}".format(base))
            after = page.evaluate(
                f"() => ({{ value: document.querySelector('{NAME_INPUT}').value, err: {SH}.branchError,"
                f"  open: {SH}.branchOpen }})"
            )
            check(
                "on success the field clears, no error is set and the modal stays open",
                after["value"] == "" and after["err"] == "" and after["open"] is True,
                "got={}".format(after),
            )
            shot = os.path.join(SHOT_DIR, "405-worktree-create-2026-09-15.png")
            page.screenshot(path=shot)
            print(f"[INFO] screenshot {shot}", flush=True)

            # --- scenario 4: an existing branch is refused, verbatim ----------
            type_name_and_enter(page, "taken")
            page.wait_for_function(f"() => {SH}.branchError !== ''", timeout=20000)
            # The refresh after a refusal is a round trip too: let it land
            # before counting rows, or a still-in-flight listing reads as 2 by
            # luck rather than by fact.
            page.wait_for_timeout(500)
            refused = page.evaluate(
                f"() => ({{ err: {SH}.branchError,"
                "  shown: (() => { const e = document.querySelector('.branch-modal .worktree-create-error');"
                "    return e && e.offsetParent !== null && e.clientWidth > 0 ? e.textContent.trim() : null; })(),"
                f"  value: document.querySelector('{NAME_INPUT}').value }})"
            )
            rows = page.evaluate(ROWS_EXPR)
            check(
                "a taken name sets branchError to the verb's message (contains `already exists`)",
                "already exists" in (refused["err"] or ""),
                "got={!r}".format(refused["err"]),
            )
            check(
                "the refusal is rendered inside the modal with the same text",
                refused["shown"] is not None and refused["shown"] == (refused["err"] or "").strip(),
                "got={}".format(refused),
            )
            check("the listing still has two rows", len(rows) == 2, "got={}".format(rows))
            check(
                "nothing was created for the refused name",
                not (fixture / ".ralphy" / "worktrees" / "taken").exists(),
            )
            check("the refused name stays in the field", refused["value"] == "taken", "got={}".format(refused))

            # --- scenario 5: a path separator is refused ----------------------
            page.evaluate(f"() => {{ {SH}.branchError = ''; }}")
            type_name_and_enter(page, "a/b")
            page.wait_for_function(f"() => {SH}.branchError !== ''", timeout=20000)
            err = page.evaluate(f"() => {SH}.branchError")
            check(
                "a name with a separator is refused with `single path segment`",
                "single path segment" in (err or ""),
                "got={!r}".format(err),
            )
            check(
                "nothing was created for the separator name",
                not (fixture / ".ralphy" / "worktrees" / "a").exists()
                and not (fixture / ".ralphy" / "worktrees" / "a/b").exists(),
            )
            close_picker(page, slug)

            # --- scenario 6 ---------------------------------------------------
            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 16
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
