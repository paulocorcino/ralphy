"""#487 browser acceptance: the `x-icon` directive draws every icon.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR` (own
port, own registry, so the operator's own desk and login policy are untouched).
Nothing here calls `lucide.createIcons()`: the page has no such call left, and
a scenario that did would mask the defect it checks for.

Scenario 1  on first paint, with no click: every visible `x-icon` is drawn
Scenario 2  an icon inside an `x-if` that opens later (the empty Spend view)
            is drawn, with the `lucide lucide-<name>` classes the CSS expects
Scenario 3  rows an `x-for` rebuilds (filtering the projects) are drawn
Scenario 4  a changed icon name draws the new icon
Scenario 5  a sweep over the tabs, panels and modals finds no empty icon and
            no unknown icon name
Scenario 6  no page error

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/workbench/wb_icons_487.py   (exit 0 = all pass)
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

PORT = 7488
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
# The Settings dialog's own component, nested in shell().
SET = "Alpine.$data(document.querySelector('.settings-dialog'))"

# Every icon on screen, split into drawn and empty. An icon behind `x-show`
# is not on screen and is not counted: its turn comes when a scenario shows it.
SWEEP = (
    "() => { const on = Array.from(document.querySelectorAll('svg[x-icon]'))"
    "  .filter((e) => e.getClientRects().length > 0);"
    "  const empty = on.filter((e) => e.childElementCount === 0);"
    "  return { shown: on.length,"
    "    empty: empty.map((e) => e.getAttribute('x-icon') + ' in ' +"
    "      (e.parentElement.className || e.parentElement.tagName)) }; }"
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
    empty = tempfile.mkdtemp(prefix="wb487i_empty_")
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


def seed(name):
    d = Path(tempfile.mkdtemp(prefix="wb487i_")) / name
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "README.md").write_text(f"# {name}\n\nThe #487 icon fixture.\n", encoding="utf-8")
    for args in (
        ["git", "init", "-b", "main"],
        ["git", "config", "user.email", "wb487@example.com"],
        ["git", "config", "user.name", "wb487"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    return str(d)


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # the previous build's markup.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def settle(page):
    page.evaluate("() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")


def rows_drawn(page):
    return page.evaluate(
        "() => { const rows = Array.from(document.querySelectorAll('li.project'))"
        "  .filter((e) => e.offsetParent !== null);"
        "  return { rows: rows.length,"
        "    chevrons: rows.filter((r) => r.querySelector('.chevron svg[x-icon] path')).length }; }"
    )


def main():
    if port_already_listening(PORT):
        check(f"port {PORT} free before launch", False, "a listener is already bound")
        sys.exit(1)

    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb487i_reg_")
    slug_a = register_fixture(daemon_dir, seed("icons-alpha"))
    register_fixture(daemon_dir, seed("icons-beta"))

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check("daemon listening", False, BASE)
            sys.exit(1)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            thrown, unknown = [], []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.on("console", lambda m: "x-icon:" in m.text and unknown.append(m.text))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 2", timeout=15000)
            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('li.project'))"
                "  .filter((e) => e.offsetParent !== null).length === 2",
                timeout=15000,
            )
            settle(page)

            # --- scenario 1: first paint ------------------------------------
            first = page.evaluate(SWEEP)
            check(
                "1 every visible icon is drawn on first paint",
                first["shown"] >= 10 and first["empty"] == [],
                f"got={first}",
            )

            # --- scenario 2: an x-if that opens later ------------------------
            # The empty Spend view means "no project open". Opening a project
            # tears that `x-if` down; closing it again builds a NEW branch.
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug_a)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug_a, timeout=15000)
            page.wait_for_function("() => document.querySelector('.spend-blank-icon') === null", timeout=5000)
            check("2 the empty Spend view leaves the DOM while a project is open", True)
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug_a)
            page.wait_for_function(f"() => {SH}.openSlug === null", timeout=15000)
            page.evaluate(f"() => {{ {SH}.active = 'spend'; }}")
            page.wait_for_selector(".spend-blank-icon", timeout=5000)
            settle(page)
            blank = page.evaluate(
                "() => { const e = document.querySelector('.spend-blank-icon');"
                "  return { paths: e.querySelectorAll('path, circle, ellipse, line, rect').length,"
                "    cls: e.getAttribute('class'), name: e.getAttribute('data-lucide') }; }"
            )
            check(
                "2 the icon inside the x-if is drawn, with the classes the CSS expects",
                blank["paths"] > 0
                and blank["name"] == "coins"
                and set(blank["cls"].split()) >= {"lucide", "lucide-coins", "spend-blank-icon"},
                f"got={blank}",
            )

            # --- scenario 3: rows an x-for rebuilds ---------------------------
            page.evaluate(f"() => {{ {SH}.projectQuery = 'icons-beta'; }}")
            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('li.project'))"
                "  .filter((e) => e.offsetParent !== null).length === 1",
                timeout=10000,
            )
            page.evaluate(f"() => {{ {SH}.projectQuery = ''; }}")
            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('li.project'))"
                "  .filter((e) => e.offsetParent !== null).length === 2",
                timeout=10000,
            )
            settle(page)
            drawn = rows_drawn(page)
            check("3 the rebuilt rows are drawn", drawn == {"rows": 2, "chevrons": 2}, f"got={drawn}")

            # --- scenario 4: a changed name draws the new icon ----------------
            head = "document.querySelector('.kanban-col-head svg[x-icon]')"
            read_head = f"() => ({{ cls: {head}.getAttribute('class') || '', body: {head}.innerHTML }})"
            before = page.evaluate(read_head)
            page.evaluate(f"() => {{ {SH}.KANBAN.COLUMNS[0].lucide = 'star'; }}")
            settle(page)
            after = page.evaluate(read_head)
            check(
                "4 a changed name draws the new icon",
                "lucide-inbox" in before["cls"]
                and "lucide-star" in after["cls"]
                and "lucide-inbox" not in after["cls"]
                and after["body"] != before["body"]
                and after["body"] != "",
                f"before={before['cls']} after={after['cls']}",
            )
            page.evaluate(f"() => {{ {SH}.KANBAN.COLUMNS[0].lucide = 'inbox'; }}")

            # --- scenario 5: a sweep over every surface -----------------------
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug_a)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug_a, timeout=15000)
            views = [
                ("spend", f"{SH}.openSpend()"),
                ("consoles", f"{SH}.active = 'consoles'"),
                ("kanban", f"{SH}.toggleKanban()"),
                ("runs", f"{SH}.toggleRuns()"),
                ("changes", f"{SH}.showSideView('changes')"),
                ("settings", f"{SET}.openSettings()"),
                ("about", f"{SET}.closeSettings(); {SH}.openAbout()"),
                ("run modal", f"{SH}.closeAbout(); {SH}.openRunModal()"),
            ]
            for name, act in views:
                page.evaluate(f"() => {{ {act}; }}")
                page.wait_for_timeout(600)
                settle(page)
                sweep = page.evaluate(SWEEP)
                check(f"5 {name}: no empty icon on screen", sweep["empty"] == [], f"got={sweep}")
            check("5 no unknown icon name", unknown == [], f"got={unknown}")

            check("6 no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
