"""#501 browser acceptance: adding a project from the workbench.

One Playwright pass over a REAL daemon that starts with an EMPTY registry.

Scenario a  the empty state shows "No projects yet" and an Add a project
            button, and the header button sits before Hosts
Scenario b  the dialog opens with the field filled with a start folder
Scenario c  a local repo: "Add project", the dialog closes, and the new
            project is selected, laid out and focused
Scenario d  a subfolder of a repo: "Add <repo>", and the repo root is what
            the registry holds
Scenario e  a plain folder: "Create repository and add" creates `.git`
Scenario f  a typing error in a parent folder: "This folder does not exist",
            disabled, and no folder is created
Scenario i  a new name in a folder that exists: "Create folder and
            repository" with the full path; Enter creates nothing; the click
            creates the folder and its repository, and adds it
Scenario g  a second clone of an added `owner/repo` is refused with the path
            of the first; the dialog stays open with the path and the reason
Scenario h  the first clone reads "Already in Projects", disabled

Boots a Localhost daemon on 7501 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own registry and login policy are untouched. The daemon is stopped
by its own subprocess handle, NEVER by name.

Writes .ralphy/screenshots/501-add-project.png.
Run: python tests/browser/projects/wb_add_project_501.py   (exit 0 = all pass)
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

PORT = 7501
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
SHOT = os.path.join(SHOT_DIR, "501-add-project.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
# The dialog's own component, nested in shell().
DLG = "Alpine.$data(document.querySelector('.add-project-dialog'))"
DIALOG = ".modal[aria-label='Add a project']"
PRIMARY = DIALOG + " .modal-foot .btn.accent"

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
    empty = tempfile.mkdtemp(prefix="wb501_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
        # `--init` commits; a host with no git identity must not fail it.
        GIT_AUTHOR_NAME="wb501",
        GIT_AUTHOR_EMAIL="wb501@example.com",
        GIT_COMMITTER_NAME="wb501",
        GIT_COMMITTER_EMAIL="wb501@example.com",
    )


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def seed(base, name, origin=None):
    d = base / name
    d.mkdir(parents=True)
    (d / "src").mkdir()
    (d / "src" / "a.txt").write_text("alpha\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb501@example.com")
    git(d, "config", "user.name", "wb501")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    if origin:
        git(d, "remote", "add", "origin", origin)
    return str(d)


def build():
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


def laid(page, selector):
    return page.evaluate(
        "(s) => { const e = document.querySelector(s);"
        "  return !!e && e.offsetParent !== null && e.clientWidth > 0; }",
        selector,
    )


def open_dialog(page):
    page.evaluate("() => window.dispatchEvent(new CustomEvent('workbench:add-project-open'))")
    page.wait_for_function(f"() => {DLG}.addProject.open === true", timeout=10000)
    page.wait_for_function(f"() => !{DLG}.addProject.needStart", timeout=10000)


def type_folder(page, path):
    page.fill("#add-project-folder", path)


def wait_label(page, label, enabled, timeout=10000):
    """Wait for the main button to read `label` with that enabled state. The
    idle dialog also reads "Add project", disabled, so the state is part of
    what is waited for."""
    try:
        page.wait_for_function(
            "([sel, want, on]) => { const b = document.querySelector(sel);"
            "  return b?.textContent.trim() === want && b.disabled === !on; }",
            arg=[PRIMARY, label, enabled],
            timeout=timeout,
        )
        return True
    except Exception:
        return False


def primary(page):
    return page.evaluate(
        "(sel) => { const b = document.querySelector(sel);"
        "  return { label: b?.textContent.trim(), disabled: !!b?.disabled }; }",
        PRIMARY,
    )


def repos(page):
    return page.evaluate("() => fetch('/api/repos').then(r => r.json())")


def same(a, b):
    return os.path.normcase(os.path.realpath(a)) == os.path.normcase(os.path.realpath(b))


def added_and_selected(page, path, name):
    """Click the main button, then wait for the dialog to close and for the
    project registered at `path` to be the open, focused row."""
    page.click(PRIMARY)
    page.wait_for_function(f"() => {DLG}.addProject.open === false", timeout=30000)
    page.wait_for_function(
        "(p) => fetch('/api/repos').then(r => r.json()).then(x => x.some(e => e.path && e.path.toLowerCase().replace(/\\\\/g, '/') === p))",
        arg=Path(path).resolve().as_posix().lower(),
        timeout=20000,
    )
    entry = next(e for e in repos(page) if same(e["path"], path))
    slug = entry["slug"]
    try:
        page.wait_for_function(
            "(slug) => " + SH + ".openSlug === slug"
            " && document.activeElement?.closest('li.project.open') !== null",
            arg=slug,
            timeout=15000,
        )
        selected = True
    except Exception:
        selected = False
    focus = page.evaluate(
        "() => { const h = document.querySelector('li.project.open .project-head');"
        "  return { laid: !!h && h.offsetParent !== null && h.clientWidth > 0,"
        "           focused: document.activeElement === h }; }"
    )
    check(f"{name}: the new project is selected", selected, f"slug={slug}")
    check(f"{name}: …laid out and focused", focus["laid"] and focus["focused"], f"focus={focus}")
    return slug


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb501_reg_")
    base = Path(tempfile.mkdtemp(prefix="wb501_disk_"))
    alpha = seed(base, "alpha")
    beta = seed(base, "beta")
    plain = base / "plain"
    plain.mkdir()
    clone1 = seed(base / "one", "clone", origin="https://github.com/wb501/clone.git")
    clone2 = seed(base / "two", "clone", origin="https://github.com/wb501/clone.git")
    sep = os.sep

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            page = browser.new_context(viewport={"width": 1440, "height": 900}).new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)

            # --- a: empty state and header -------------------------------
            page.wait_for_function(f"() => !{SH}.reposLoading", timeout=15000)
            check("a: the empty state is laid out", laid(page, ".projects-empty"))
            text = page.evaluate("() => document.querySelector('.projects-empty')?.textContent || ''")
            check("a: …it says No projects yet", "No projects yet" in text, repr(text.strip()))
            check("a: …and offers Add a project", laid(page, ".projects-empty .btn"))
            order = page.evaluate(
                "() => { const b = [...document.querySelectorAll('.projects-view .side-head button')];"
                "  return b.map(x => x.getAttribute('aria-label')); }"
            )
            check(
                "a: the header button comes before Hosts",
                order.index("Add a project") < order.index("Hosts") if "Add a project" in order else False,
                f"order={order}",
            )

            # --- b: the dialog opens on a start folder ----------------------
            page.click(".projects-empty .btn")
            page.wait_for_function(f"() => {DLG}.addProject.open === true", timeout=10000)
            # Alpine's x-show lands after the property write: wait for the box.
            try:
                page.wait_for_function(
                    "(s) => { const e = document.querySelector(s);"
                    "  return !!e && e.offsetParent !== null && e.clientWidth > 0; }",
                    arg=DIALOG,
                    timeout=5000,
                )
                shown = True
            except Exception:
                shown = False
            check("b: the dialog is laid out", shown)
            page.wait_for_function(f"() => !{DLG}.addProject.needStart", timeout=10000)
            start = page.input_value("#add-project-folder")
            check("b: the field opens on a start folder", start.endswith(("/", "\\")) and len(start) > 1, repr(start))
            page.screenshot(path=SHOT)
            print(f"[INFO] screenshot {SHOT}", flush=True)

            # --- c: a local repo -------------------------------------------
            type_folder(page, alpha)
            check("c: a repo reads Add project", wait_label(page, "Add project", True), str(primary(page)))
            added_and_selected(page, alpha, "c")

            # --- d: a subfolder of a repo ----------------------------------
            open_dialog(page)
            type_folder(page, beta + sep + "src")
            check("d: a subfolder reads Add beta", wait_label(page, "Add beta", True), str(primary(page)))
            added_and_selected(page, beta, "d")

            # --- e: a plain folder ------------------------------------------
            open_dialog(page)
            type_folder(page, str(plain))
            check("e: a plain folder reads Create repository and add", wait_label(page, "Create repository and add", True), str(primary(page)))
            added_and_selected(page, str(plain), "e")
            check("e: …and the folder is now a repository", (plain / ".git").exists())

            # --- f: a typing error ------------------------------------------
            open_dialog(page)
            typo = str(base / "no-such-folder")
            type_folder(page, typo + sep + "x")
            check("f: a missing parent reads This folder does not exist", wait_label(page, "This folder does not exist", False), str(primary(page)))
            check("f: …and the button is disabled", primary(page)["disabled"])
            check("f: …and no folder was created", not os.path.exists(typo))

            # --- g: the second clone ----------------------------------------
            type_folder(page, clone1)
            wait_label(page, "Add project", True)
            added_and_selected(page, clone1, "g")
            open_dialog(page)
            type_folder(page, clone2)
            check("g: the second clone reads Add project", wait_label(page, "Add project", True), str(primary(page)))
            page.click(PRIMARY)
            try:
                page.wait_for_function(f"() => !!{DLG}.addProject.error", timeout=30000)
                error = page.evaluate(f"() => {DLG}.addProject.error")
            except Exception:
                error = ""
            check("g: the add is refused with the first path", "wb501/clone is already added from" in error, repr(error))
            check("g: …the reason is shown as an alert", laid(page, DIALOG + " [role='alert']"))
            check("g: …the dialog stays open with the path", page.input_value("#add-project-folder") == clone2)
            entries = [e for e in repos(page) if e["slug"] == "wb501/clone"]
            check(
                "g: …and the registry still points at the first clone",
                len(entries) == 1 and same(entries[0]["path"], clone1),
                str(entries),
            )

            # --- h: already added -------------------------------------------
            type_folder(page, clone1)
            check("h: an added folder reads Already in Projects", wait_label(page, "Already in Projects", False), str(primary(page)))
            check("h: …and the button is disabled", primary(page)["disabled"])

            # --- i: a new folder --------------------------------------------
            fresh = base / "new project"
            type_folder(page, str(fresh))
            check("i: a new name reads Create folder and repository", wait_label(page, "Create folder and repository", True), str(primary(page)))
            hint = page.evaluate("() => document.querySelector('.modal[aria-label=\\'Add a project\\'] .run-help')?.textContent.trim()")
            check(
                "i: …the line under the field names the full path",
                hint == f"Ralphy creates the folder {fresh} and a git repository in it.",
                repr(hint),
            )
            page.focus("#add-project-folder")
            page.keyboard.press("Enter")
            time.sleep(1.5)
            check(
                "i: …Enter creates nothing and the dialog stays open",
                not fresh.exists() and page.evaluate(f"() => {DLG}.addProject.open"),
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "501-add-project-create-folder.png"))
            added_and_selected(page, str(fresh), "i")
            check("i: …the folder exists and is a repository", (fresh / ".git").exists())

            open_dialog(page)
            page.keyboard.press("Escape")
            page.wait_for_function(f"() => {DLG}.addProject.open === false", timeout=5000)
            check("Escape closes the dialog", True)
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite.
    check_floor = 37
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
