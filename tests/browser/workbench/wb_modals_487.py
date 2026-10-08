"""#487 browser acceptance: the ten modals share one binding.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR` (own
port, own registry, so the operator's own desk and login policy are untouched).
`RALPHY_DAEMON_AGENT_OVERRIDE` makes the console a `session_test_child`, which
echoes each typed line back as `GOT:<line>`.

Scenario 1  Settings, opened from the keyboard, takes focus into the dialog (not
            onto its ✕), Escape closes it, and focus returns to the rail button
Scenario 1b the same for Hosts, whose dialog is its own Alpine component
            nested in shell(): it opens on the event its sidebar button sends
Scenario 1c the same for Add a project, the second dialog with its own component
Scenario 2  Escape closes Security; Escape in the step-up field cancels only the
            step-up and leaves Security open
Scenario 3  Branch and Prompt keep the field they focus themselves
Scenario 4  a confirm over the Plan modal: focus starts on Cancel, Enter on it
            cancels, Enter elsewhere confirms without clicking the button focus
            returns to, and one Escape closes the confirm alone
Scenario 5  a modal opened while a console has focus hands focus back to the
            console on close, and the next keys reach the child
Scenario 6  no page error

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/workbench/wb_modals_487.py   (exit 0 = all pass)
"""

import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7487
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
# The Hosts dialog's own component, nested in shell().
HOSTS = "Alpine.$data(document.querySelector('.hosts-dialog'))"
# The Add a project dialog's own component, nested in shell().
ADDPROJECT = "Alpine.$data(document.querySelector('.add-project-dialog'))"
# The Security dialog's own component, nested in shell().
SECURITY = "Alpine.$data(document.querySelector('.security-dialog'))"
# The Settings dialog's own component, nested in shell().
SETTINGS = "Alpine.$data(document.querySelector('.settings-dialog'))"
# The About, What's new and update dialogs' own component, nested in shell().
RELEASE = "Alpine.$data(document.querySelector('.release-dialogs'))"

# What has focus, in words a failure line can print.
ACTIVE = (
    "() => { const a = document.activeElement; if (!a) return null;"
    "  return { tag: a.tagName, id: a.id, cls: a.className && String(a.className),"
    "    text: (a.textContent || '').trim().slice(0, 30), title: a.getAttribute('title'),"
    "    modal: a.closest('[role=dialog],[role=alertdialog]')?.getAttribute('aria-label') || null }; }"
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


def port_already_listening(port, host="127.0.0.1"):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(0.5)
    try:
        return s.connect_ex((host, port)) == 0
    finally:
        s.close()


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def empty_env(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wb487_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_DAEMON_AGENT_OVERRIDE=CHILD,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb487_fixture_")
    (Path(d) / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (Path(d) / "README.md").write_text("# fixture\n\nThe #487 modal fixture.\n", encoding="utf-8")
    for args in (
        ["git", "init", "-b", "main"],
        ["git", "config", "user.email", "wb487@example.com"],
        ["git", "config", "user.name", "wb487"],
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
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # the previous build's modals.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True
    )


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def active(page):
    return page.evaluate(ACTIVE)


def flag(page, expr):
    return page.evaluate(f"() => {SH}.{expr}")


def wait_flag(page, expr, value, timeout=5000):
    page.wait_for_function(f"(v) => {SH}.{expr} === v", arg=value, timeout=timeout)


def settle(page):
    # The binding focuses one frame after the flag flips; two frames cover it.
    page.evaluate("() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")


def screen(page, i=0):
    return page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " const b = w && w._term && w._term.term && w._term.term.buffer.active;"
        " if (!b) return '';"
        " let out = '';"
        " for (let y = 0; y < b.length; y++) {"
        "   const line = b.getLine(y);"
        "   if (line) out += line.translateToString(true) + '\\n';"
        " }"
        " return out; }",
        i,
    )


def wait_screen(page, text, timeout=15000):
    deadline = time.time() + timeout / 1000
    while time.time() < deadline:
        if text in screen(page):
            return True
        page.wait_for_timeout(250)
    return False


def main():
    if port_already_listening(PORT):
        check(f"port {PORT} free before launch", False, "a listener is already bound")
        sys.exit(1)

    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb487_reg_")
    slug = register_fixture(daemon_dir, make_fixture_repo())

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check("daemon listening", False, BASE)
            sys.exit(1)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => Alpine.store('projects').projects.length === 1", timeout=15000)

            # --- scenario 1: Settings from the keyboard ----------------------
            gear = page.locator('button[title="Settings"]').first
            gear.focus()
            page.keyboard.press("Enter")
            page.wait_for_function(f"() => {SETTINGS}.settingsOpen === true", timeout=5000)
            check("1 the rail button shows Settings active", page.locator('button[title="Settings"].active').count() == 1)
            settle(page)
            a = active(page)
            check(
                "1 Settings takes focus into the dialog, not onto its ✕",
                a and a["modal"] == "Settings" and "modal-x" not in (a["cls"] or ""),
                f"active={a}",
            )
            page.keyboard.press("Escape")
            page.wait_for_function(f"() => {SETTINGS}.settingsOpen === false", timeout=5000)
            settle(page)
            a = active(page)
            check("1 Escape closes Settings", page.evaluate(f"() => {SETTINGS}.settingsOpen") is False)
            check("1 focus returns to the Settings button", a and a["title"] == "Settings", f"active={a}")

            # --- scenario 1b: Hosts, a dialog with its own Alpine component ---
            hosts_open = f"() => {HOSTS}.addHost.open"
            page.locator("button.side-add-host").focus()
            page.keyboard.press("Enter")
            page.wait_for_function(f"() => {HOSTS}.addHost.open === true", timeout=5000)
            settle(page)
            a = active(page)
            check("1b the sidebar button opens Hosts, and focus goes into it", a and a["modal"] == "Hosts", f"active={a}")
            page.keyboard.press("Escape")
            page.wait_for_function(f"() => {HOSTS}.addHost.open === false", timeout=5000)
            settle(page)
            a = active(page)
            check("1b Escape closes Hosts", page.evaluate(hosts_open) is False)
            check("1b focus returns to the Hosts button", a and a["title"] == "Hosts", f"active={a}")
            check("1b the modal stack is empty", page.evaluate(f"() => {SH}._modalStack.length") == 0)

            # --- scenario 1c: Add a project, its own Alpine component too ------
            page.locator("button.side-add-project").focus()
            page.keyboard.press("Enter")
            page.wait_for_function(f"() => {ADDPROJECT}.addProject.open === true", timeout=5000)
            settle(page)
            a = active(page)
            check("1c the sidebar button opens Add a project, and focus goes into it", a and a["modal"] == "Add a project", f"active={a}")
            page.keyboard.press("Escape")
            page.wait_for_function(f"() => {ADDPROJECT}.addProject.open === false", timeout=5000)
            settle(page)
            a = active(page)
            check("1c Escape closes Add a project", page.evaluate(f"() => {ADDPROJECT}.addProject.open") is False)
            check("1c focus returns to the Add a project button", a and a["title"] == "Add a project", f"active={a}")
            check("1c the modal stack is empty", page.evaluate(f"() => {SH}._modalStack.length") == 0)

            # --- scenario 2: Security, and its step-up field ------------------
            # The account menu's item closes the menu and opens the dialog.
            page.locator(".avatar-btn").click()
            page.wait_for_function(f"() => {SH}.avatarMenu === true", timeout=5000)
            page.locator(".account-menu .dropdown-item", has_text="Security settings").click()
            page.wait_for_function(f"() => {SECURITY}.securityOpen === true", timeout=5000)
            check("2 the account menu closes when it opens Security", flag(page, "avatarMenu") is False)
            settle(page)
            page.evaluate(f"() => {{ {SECURITY}.stepUp = {{ open: true, code: '', label: 'test' }}; }}")
            page.wait_for_function(
                "() => { const i = document.querySelector('.step-up input'); return !!i && i.offsetParent !== null; }",
                timeout=5000,
            )
            page.locator(".step-up input").focus()
            page.keyboard.press("Escape")
            page.wait_for_timeout(200)
            step_up = page.evaluate(f"() => {SECURITY}.stepUp.open")
            sec_open = page.evaluate(f"() => {SECURITY}.securityOpen")
            check(
                "2 Escape in the step-up field cancels the step-up and keeps Security open",
                step_up is False and sec_open is True,
                f"stepUp={step_up} security={sec_open}",
            )
            page.keyboard.press("Escape")
            page.wait_for_timeout(200)
            check("2 the next Escape closes Security", page.evaluate(f"() => {SECURITY}.securityOpen") is False)

            # Log off closes Security with an event. On loopback there is no
            # login gate, so the page stays usable for the next scenarios.
            check("2 loopback has no login gate", flag(page, "security.policy") != "session", flag(page, "security.policy"))
            page.evaluate("() => document.dispatchEvent(new CustomEvent('workbench:security-open', { bubbles: true }))")
            page.wait_for_function(f"() => {SECURITY}.securityOpen === true", timeout=5000)
            page.evaluate(f"() => {SH}.logOff()")
            page.wait_for_function(f"() => {SECURITY}.securityOpen === false", timeout=5000)
            settle(page)
            check("2 log off closes Security", page.evaluate(f"() => {SECURITY}.securityOpen") is False)
            check("2 the modal stack is empty after log off", page.evaluate(f"() => {SH}._modalStack.length") == 0)

            # Log off closes Settings the same way.
            page.evaluate("() => document.dispatchEvent(new CustomEvent('workbench:settings-open', { bubbles: true }))")
            page.wait_for_function(f"() => {SETTINGS}.settingsOpen === true", timeout=5000)
            page.evaluate(f"() => {SH}.logOff()")
            page.wait_for_function(f"() => {SETTINGS}.settingsOpen === false", timeout=5000)
            settle(page)
            check("2 log off closes Settings", page.evaluate(f"() => {SETTINGS}.settingsOpen") is False)
            check("2 the modal stack is empty after Settings", page.evaluate(f"() => {SH}._modalStack.length") == 0)

            # --- scenario 3: Branch and Prompt keep their own field -----------
            page.evaluate(f"(s) => {{ if (Alpine.store('projects').openSlug !== s) {SH}.toggle(s); }}", arg=slug)
            page.wait_for_function(f"(s) => Alpine.store('projects').openSlug === s", arg=slug, timeout=15000)
            page.wait_for_function(
                "() => { const c = document.querySelector('li.project.open .project-head .branch-chip');"
                "  return !!c && c.offsetParent !== null; }",
                timeout=15000,
            )
            page.locator("li.project.open .project-head .branch-chip").click()
            wait_flag(page, "branchOpen", True)
            settle(page)
            a = active(page)
            check(
                "3 Branch keeps focus on its filter",
                page.evaluate(f"() => document.activeElement === {SH}.$refs.branchFilter"),
                f"active={a}",
            )
            page.keyboard.press("Escape")
            wait_flag(page, "branchOpen", False)

            page.evaluate(f"() => {{ window.__prompt = {SH}.askPrompt({{ title: 'Name' }}); }}")
            wait_flag(page, "promptModal.open", True)
            settle(page)
            a = active(page)
            check("3 Prompt keeps focus on its input", a and a["id"] == "prompt-input", f"active={a}")
            page.keyboard.press("Escape")
            check("3 Escape cancels the prompt", page.evaluate("() => window.__prompt") is None)

            # --- scenario 4: a confirm over the Plan modal --------------------
            page.evaluate(f"() => {{ {SH}.planModal.open = true; }}")
            wait_flag(page, "planModal.open", True)
            settle(page)
            page.evaluate(f"() => {{ window.__ans = {SH}.askConfirm({{ title: 'Discard' }}); }}")
            wait_flag(page, "confirmModal.open", True)
            settle(page)
            a = active(page)
            check("4 the confirm starts on Cancel", a and a["text"] == "Cancel", f"active={a}")
            page.keyboard.press("Enter")
            check("4 Enter on the focused Cancel cancels", page.evaluate("() => window.__ans") is False)
            check("4 …and the plan under it stays open", flag(page, "planModal.open") is True)

            page.evaluate(f"() => {{ window.__ans = {SH}.askConfirm({{ title: 'Discard' }}); }}")
            wait_flag(page, "confirmModal.open", True)
            settle(page)
            page.evaluate("() => document.activeElement.blur()")
            page.keyboard.press("Enter")
            check("4 Enter with no button focused confirms", page.evaluate("() => window.__ans") is True)
            # Focus goes back to the Plan's button; the same Enter's keypress
            # must not click it.
            page.wait_for_timeout(200)
            check("4 …and that Enter does not click the button focus returns to", flag(page, "planModal.open") is True)

            page.evaluate(f"() => {{ window.__ans = {SH}.askConfirm({{ title: 'Discard' }}); }}")
            wait_flag(page, "confirmModal.open", True)
            settle(page)
            page.keyboard.press("Escape")
            page.wait_for_timeout(200)
            check(
                "4 one Escape closes the confirm alone",
                page.evaluate("() => window.__ans") is False and flag(page, "planModal.open") is True,
                f"plan={flag(page, 'planModal.open')}",
            )
            page.keyboard.press("Escape")
            page.wait_for_timeout(200)
            check("4 the next Escape closes the plan", flag(page, "planModal.open") is False)

            # --- scenario 5: a console keeps its keys across a modal ----------
            page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
            before = page.locator(".session-window").count()
            page.evaluate(f"() => window.WBConsole.open({{ repo: '{slug}', plain: true }})")
            page.wait_for_function(
                f"() => document.querySelectorAll('.session-window').length === {before + 1}", timeout=8000
            )
            page.locator(".session-window").nth(before).locator(".xterm").wait_for(timeout=15000)
            check("5 the child booted", wait_screen(page, "READY"))
            page.locator(".session-window").nth(before).locator(".xterm").click()
            a = active(page)
            check(
                "5 the console has focus before the modal",
                a and "xterm-helper-textarea" in (a["cls"] or ""),
                f"active={a}",
            )
            page.evaluate("() => document.dispatchEvent(new CustomEvent('workbench:about-open', { bubbles: true }))")
            page.wait_for_function(f"() => {RELEASE}.aboutOpen === true", timeout=5000)
            settle(page)
            a = active(page)
            check("5 About takes focus from the console", a and a["modal"] == "About Ralphy", f"active={a}")
            page.keyboard.press("Escape")
            page.wait_for_function(f"() => {RELEASE}.aboutOpen === false", timeout=5000)
            settle(page)
            a = active(page)
            check(
                "5 closing About gives focus back to the console",
                a and "xterm-helper-textarea" in (a["cls"] or ""),
                f"active={a}",
            )
            page.keyboard.type("after487")
            page.keyboard.press("Enter")
            check("5 the next keys reach the child", wait_screen(page, "GOT:after487"))

            check("6 no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
