"""Browser acceptance: the file tree draws gitignored entries muted.

The operator must see which entries are outside the repo without opening
`.gitignore`. The tree still lists every entry; an ignored one only carries
`wb-ignored`.

Scenario a  at the root, an ignored folder and an ignored file are marked, a
            tracked folder, a tracked file and `.gitignore` itself are not
Scenario b  the children of an ignored folder are marked too
Scenario c  an edit to the root `.gitignore` re-marks a folder that is ALREADY
            open, through the real watcher nudge, without reopening the project

Boots a Localhost daemon on 7461 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Run: python tests/browser/files/wb_tree_ignored.py   (exit 0 = all pass)
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

PORT = 7461
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
)
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
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
    empty = tempfile.mkdtemp(prefix="wbignored_empty_")
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


def seed():
    d = Path(tempfile.mkdtemp(prefix="wbignored_repo_")) / "ignored-fixture"
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\ndist/\n*.log\n", encoding="utf-8")
    (d / "keep-me.txt").write_text("keep\n", encoding="utf-8")
    (d / "run.log").write_text("log\n", encoding="utf-8")
    (d / "dist").mkdir()
    (d / "dist" / "out.js").write_text("x\n", encoding="utf-8")
    (d / "src").mkdir()
    (d / "src" / "main.txt").write_text("main\n", encoding="utf-8")
    (d / "src" / "gen.txt").write_text("gen\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wbignored@example.com")
    git(d, "config", "user.name", "wbignored")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", str(fixture_dir)],
        env=env,
        check=True,
        capture_output=True,
        encoding="utf-8",
    )
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # yesterday's app.js and the whole run is vacuous.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


# title → carries `wb-ignored`, for every LAID-OUT row. A present-but-unlaid-out
# row is excluded, so a check cannot pass on a zero-height row.
MARKS = (
    "() => Object.fromEntries([...document.querySelectorAll('.wb-host .wb-row')]"
    "  .filter(r => r.offsetParent !== null && r.clientWidth > 0)"
    "  .map(r => [r.querySelector('.wb-title')?.textContent.trim(), r.classList.contains('wb-ignored')]))"
)

# The painted title opacity of one row each, to prove the class reaches the eye.
OPACITY = (
    "() => { const o = (t) => { const r = [...document.querySelectorAll('.wb-host .wb-row')]"
    "  .find(r => r.querySelector('.wb-title')?.textContent.trim() === t);"
    "  return r ? Number(getComputedStyle(r.querySelector('.wb-title')).opacity) : null; };"
    "  return { ignored: o('run.log'), tracked: o('keep-me.txt') }; }"
)


def wait_row(page, title):
    page.wait_for_function(
        f"(t) => ({MARKS})()[t] !== undefined", arg=title, timeout=15000
    )


def main():
    build()
    fixture = seed()
    daemon_dir = tempfile.mkdtemp(prefix="wbignored_daemon_")
    slug = register_fixture(daemon_dir, fixture)
    proc = subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
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
            page.wait_for_function(f"() => Alpine.store('projects').projects.length === 1", timeout=15000)
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
            page.wait_for_function(f"(s) => Alpine.store('projects').openSlug === s", arg=slug, timeout=15000)
            wait_row(page, "keep-me.txt")

            # --- scenario a: the root level -------------------------------------
            m = page.evaluate(MARKS)
            check(
                "an ignored folder and an ignored file are marked",
                m.get("dist") is True and m.get("run.log") is True,
                f"marks={m}",
            )
            check(
                "a tracked folder, a tracked file and .gitignore are not",
                m.get("src") is False and m.get("keep-me.txt") is False and m.get(".gitignore") is False,
                f"marks={m}",
            )
            opacity = page.evaluate(OPACITY)
            check(
                "an ignored row is painted faded, a tracked one is not",
                opacity["ignored"] is not None and opacity["ignored"] < 1 and opacity["tracked"] == 1,
                f"opacity={opacity}",
            )

            # --- scenario b: inside an ignored folder ---------------------------
            page.evaluate(
                f"async () => {{ const t = {SH}.rawTree();"
                "  await t.findFirst(n => n.title === 'dist').setExpanded(true);"
                "  await t.findFirst(n => n.title === 'src').setExpanded(true); }"
            )
            wait_row(page, "out.js")
            wait_row(page, "gen.txt")
            m = page.evaluate(MARKS)
            check("a child of an ignored folder is marked", m.get("out.js") is True, f"marks={m}")
            check("a child of a tracked folder is not", m.get("gen.txt") is False, f"marks={m}")

            shot = os.path.join(tempfile.gettempdir(), "wb_tree_ignored.png")
            page.locator(".wb-host").first.screenshot(path=shot)
            print(f"screenshot: {shot}", flush=True)

            # --- scenario c: an edited .gitignore re-marks an open folder -------
            (fixture / ".gitignore").write_text(".ralphy/\ndist/\n*.log\nsrc/gen.txt\n", encoding="utf-8")
            remarked = True
            try:
                page.wait_for_function(f"() => ({MARKS})()['gen.txt'] === true", timeout=15000)
            except Exception:
                remarked = False
            m = page.evaluate(MARKS)
            check(
                "an open folder is re-marked after .gitignore changes, without a reopen",
                remarked and m.get("main.txt") is False,
                f"marks={m}",
            )

            check("no uncaught page errors", not thrown, f"thrown={thrown}")
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
