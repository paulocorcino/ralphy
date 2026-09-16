"""#405 browser acceptance: creating a worktree from the branch picker.

One Playwright pass over a REAL daemon proving the create path end to end
(ADR-0063 §2, picker reshaped 2026-09-16): a new name typed in the picker's
search box offers a `Create worktree “<name>” from <base>` row beside the
`Create branch` one, even when the project has no worktrees yet; clicking it
runs `worktree.add`, the Checkouts section appears above the branches with
the new row, the worktree exists on disk with its base recorded, the branch
list marks the branch as living there, and a refusal is surfaced verbatim
inside the modal. A branch row's `+` picks that branch as the base.

Scenario 1  the daemon is listening
Scenario 2  on a fixture with NO worktrees the picker renders NO checkouts
            section; typing `wt-new` offers both create rows, the worktree
            one labelled `Create worktree “wt-new” from main` and titled
            with the carry-over note
Scenario 3  clicking it yields two rows (`primary`, `wt-new · wt-new`),
            `<fixture>/.ralphy/worktrees/wt-new` is a directory,
            `git config branch.wt-new.base` is `main`, the box clears, no
            error is set, the branch list tags `wt-new` as `in wt-new`
Scenario 4  typing `taken` (an existing branch) and clicking leaves two rows,
            sets `branchError` to a message containing `already exists`,
            rendered inside the modal in `.worktree-create-error`; no
            directory; the name stays in the box
Scenario 5  typing `a/b` offers no worktree row at all (the shape gate is
            client-side); no directory
Scenario 6  the `+` on the `taken` branch row sets the base chip; typing
            `wt-off-taken` labels the row `from taken`; clicking records
            `branch.wt-off-taken.base = taken` and clears the chip
Scenario 7  no page errors

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
FILTER_INPUT = ".branch-modal .branch-search input"
CREATE_WT_ROW = ".branch-modal .branch-item.create-worktree"
CREATE_BR_ROW = ".branch-modal .branch-item.create"
CARRY_NOTE = "Ignored files come along only via worktree.copy / worktree.share in settings.json."
VISIBLE = "(sel) => { const e = document.querySelector(sel); return !!e && e.offsetParent !== null && e.clientWidth > 0; }"
# The laid-out branch rows: name + the `in <checkout>` tag when one shows.
BRANCH_ROWS = (
    "() => Array.from(document.querySelectorAll('.branch-modal .branch-list .branch-item'))"
    "  .filter(e => e.offsetParent !== null && e.clientWidth > 0 && !e.classList.contains('create'))"
    "  .map(e => ({ name: e.querySelector('.branch-name').textContent.trim(),"
    "    tag: (() => { const t = e.querySelector('.branch-tag.in');"
    "      return t && t.offsetParent !== null ? t.textContent.trim() : null; })() }))"
)

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


def type_name(page, name):
    page.fill(FILTER_INPUT, name)


def type_name_and_create(page, name):
    type_name(page, name)
    page.wait_for_selector(CREATE_WT_ROW, state="visible", timeout=5000)
    page.click(CREATE_WT_ROW)


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

            # --- scenario 2: no checkouts section; a new name offers both rows --
            open_picker(page, slug)
            before = page.evaluate(
                "() => ({ secs: document.querySelectorAll('.branch-modal .worktree-sec').length,"
                "  head: !!document.querySelector('.branch-modal .branch-head')"
                "    && document.querySelector('.branch-modal .branch-head').offsetParent !== null,"
                "  wtRow: !!document.querySelector('.branch-modal .branch-item.create-worktree')"
                "    && document.querySelector('.branch-modal .branch-item.create-worktree').offsetParent !== null })"
            )
            check(
                "with no worktrees there is no checkouts section, no Branches head, no create row",
                before["secs"] == 0 and before["head"] is False and before["wtRow"] is False,
                "got={}".format(before),
            )
            type_name(page, "wt-new")
            page.wait_for_selector(CREATE_WT_ROW, state="visible", timeout=5000)
            offered = page.evaluate(
                "() => ({ branch: (() => { const e = document.querySelector('.branch-modal .branch-item.create');"
                "    return e && e.offsetParent !== null ? e.textContent.replace(/\\s+/g, ' ').trim() : null; })(),"
                "  worktree: (() => { const e = document.querySelector('.branch-modal .branch-item.create-worktree');"
                "    return e && e.offsetParent !== null ? { label: e.querySelector('.branch-name').textContent.trim(),"
                "      title: e.getAttribute('title') } : null; })() })"
            )
            check(
                "a new name offers `Create branch` and `Create worktree` side by side",
                (offered["branch"] or "").startswith("Create branch “wt-new”") and offered["worktree"] is not None,
                "got={}".format(offered),
            )
            check(
                "the worktree row's label names the name and the current branch",
                (offered["worktree"] or {}).get("label") == "Create worktree “wt-new” from main",
                "got={!r}".format(offered["worktree"]),
            )
            check(
                "the worktree row's tooltip is the carry-over note",
                (offered["worktree"] or {}).get("title") == CARRY_NOTE,
                "got={!r}".format(offered["worktree"]),
            )

            # --- scenario 3: the click creates the worktree --------------------
            page.click(CREATE_WT_ROW)
            page.wait_for_function(ROW_COUNT_IS, arg=2, timeout=20000)
            rows = page.evaluate(ROWS_EXPR)
            check(
                "after the click the checkouts section reads primary + wt-new · wt-new",
                rows == [{"name": "primary", "branch": "main"}, {"name": "wt-new", "branch": "wt-new"}],
                "got={}".format(rows),
            )
            wt_new = fixture / ".ralphy" / "worktrees" / "wt-new"
            check("the worktree directory exists on disk", wt_new.is_dir(), "path={}".format(wt_new))
            base = git(fixture, "config", "branch.wt-new.base") if wt_new.is_dir() else None
            check("branch.wt-new.base records the current branch", base == "main", "got={!r}".format(base))
            after = page.evaluate(
                f"() => ({{ value: document.querySelector('{FILTER_INPUT}').value, err: {SH}.branchError,"
                f"  open: {SH}.branchOpen, head: (() => {{ const h = document.querySelector('.branch-modal .branch-head');"
                "    return !!h && h.offsetParent !== null; })() })"
            )
            check(
                "on success the box clears, no error is set, the modal stays open and the Branches head shows",
                after["value"] == "" and after["err"] == "" and after["open"] is True and after["head"] is True,
                "got={}".format(after),
            )
            # `branch.list` re-reads after the add: the new branch lands with
            # its checkout tag.
            page.wait_for_function(
                "() => (" + BRANCH_ROWS + ")().some(r => r.name === 'wt-new')", timeout=15000
            )
            brows = page.evaluate(BRANCH_ROWS)
            check(
                "the branch list tags wt-new as living in wt-new and main as current (no tag)",
                any(r["name"] == "wt-new" and r["tag"] == "in wt-new" for r in brows)
                and any(r["name"] == "main" and r["tag"] is None for r in brows),
                "got={}".format(brows),
            )
            shot = os.path.join(SHOT_DIR, "405-worktree-create-2026-09-15.png")
            page.screenshot(path=shot)
            print(f"[INFO] screenshot {shot}", flush=True)

            # --- scenario 4: an existing branch is refused, verbatim ----------
            type_name_and_create(page, "taken")
            page.wait_for_function(f"() => {SH}.branchError !== ''", timeout=20000)
            # The refresh after a refusal is a round trip too: let it land
            # before counting rows, or a still-in-flight listing reads as 2 by
            # luck rather than by fact.
            page.wait_for_timeout(500)
            refused = page.evaluate(
                f"() => ({{ err: {SH}.branchError,"
                "  shown: (() => { const e = document.querySelector('.branch-modal .worktree-create-error');"
                "    return e && e.offsetParent !== null && e.clientWidth > 0 ? e.textContent.trim() : null; })(),"
                f"  value: document.querySelector('{FILTER_INPUT}').value }})"
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
            check("the refused name stays in the box", refused["value"] == "taken", "got={}".format(refused))

            # --- scenario 5: a path separator gets no row at all ---------------
            page.evaluate(f"() => {{ {SH}.branchError = ''; }}")
            type_name(page, "a/b")
            page.wait_for_timeout(200)
            sep = page.evaluate(
                "() => ({ wt: (() => { const e = document.querySelector('.branch-modal .branch-item.create-worktree');"
                "    return !!e && e.offsetParent !== null; })(),"
                "  br: (() => { const e = document.querySelector('.branch-modal .branch-item.create');"
                "    return !!e && e.offsetParent !== null; })() })"
            )
            check(
                "a name with a separator offers a branch row but no worktree row",
                sep["wt"] is False and sep["br"] is True,
                "got={}".format(sep),
            )
            check(
                "nothing was created for the separator name",
                not (fixture / ".ralphy" / "worktrees" / "a").exists()
                and not (fixture / ".ralphy" / "worktrees" / "a/b").exists(),
            )

            # --- scenario 6: the `+` on a branch row picks the base ------------
            type_name(page, "")
            page.evaluate(
                "() => { const row = Array.from(document.querySelectorAll('.branch-modal .branch-list .branch-item'))"
                "  .find(e => e.querySelector('.branch-name') && e.querySelector('.branch-name').textContent.trim() === 'taken');"
                "  row.querySelector('.branch-worktree').click(); }"
            )
            page.wait_for_selector(".branch-modal .branch-base", state="visible", timeout=5000)
            chip = page.evaluate("() => document.querySelector('.branch-modal .branch-base').textContent.replace(/\\s+/g, ' ').trim()")
            check("the base chip names the picked branch", chip == "worktree from taken", "got={!r}".format(chip))
            type_name(page, "wt-off-taken")
            page.wait_for_selector(CREATE_WT_ROW, state="visible", timeout=5000)
            label = page.evaluate("() => document.querySelector('.branch-modal .branch-item.create-worktree .branch-name').textContent.trim()")
            check(
                "the create row's label follows the picked base",
                label == "Create worktree “wt-off-taken” from taken",
                "got={!r}".format(label),
            )
            page.click(CREATE_WT_ROW)
            page.wait_for_function(ROW_COUNT_IS, arg=3, timeout=20000)
            base = git(fixture, "config", "branch.wt-off-taken.base")
            chip_gone = page.evaluate("() => { const c = document.querySelector('.branch-modal .branch-base'); return !c || c.offsetParent === null; }")
            check(
                "the worktree is cut from the picked branch and the chip clears",
                base == "taken" and chip_gone is True,
                "base={!r} chip_gone={}".format(base, chip_gone),
            )
            close_picker(page, slug)

            # --- scenario 7 ---------------------------------------------------
            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 21
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
