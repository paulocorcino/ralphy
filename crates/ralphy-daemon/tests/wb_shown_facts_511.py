"""#511 browser acceptance: the workbench shows only what it has read (ADR-0070).

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched.

Scenario 1   a corrupt `desk.toml`: the page says it could not read the saved
             desk, and "Start a new desk" moves the old file aside as
             `desk.toml.unreadable-<date>`; the notice then goes away
Scenario 2   the project list read once, then `/api/repos` fails: a change to
             `repos.toml` pushes `repos.dirty`, the row stays, and the sidebar
             says the list is not current
Scenario 3   a page served with another build id reloads itself once
Scenario 4   with an unsaved edit in a file tab, a build mismatch shows a notice
             that stays, and the page does not reload
Scenario 5   an unsaved edit in a DETACHED file window holds the main tab's
             reload (the window saves through that tab): the notice shows,
             the window's Save writes the file, and then the main tab reloads
             and closes the window

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes .ralphy/screenshots/2026-09-30-511-*.png.
Run: python crates/ralphy-daemon/tests/wb_shown_facts_511.py   (exit 0 = all pass)
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

PORT = 7511
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
DATE = "2026-09-30"

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
    empty = tempfile.mkdtemp(prefix="wb511_empty_")
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


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb511_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #511 fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb511@example.com"],
        ["git", "config", "user.name", "wb511"],
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
    # the previous build's page.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def shot(page, slug):
    path = os.path.join(SHOT_DIR, f"{DATE}-511-{slug}.png")
    page.screenshot(path=path)
    return path


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb511_store_")
    fixture = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture)
    desk_toml = Path(daemon_dir, "desk.toml")
    corrupt = b"windows = [\n"
    desk_toml.write_bytes(corrupt)
    proc = launch(daemon_dir)
    try:
        check("the daemon starts with a corrupt desk.toml", wait_listening(BASE))
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            errors = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)

            # --- scenario 1: the desk failure and its one action -------------
            failure = page.locator(".desk-failure")
            try:
                failure.wait_for(state="visible", timeout=15000)
                shown = True
            except Exception:
                shown = False
            text = failure.inner_text() if shown else ""
            check(
                "an unreadable desk says so, with the reason",
                shown and "Could not read the saved desk" in text and "parsing desk layout" in text,
                f"text={text!r}",
            )
            check("…and the file is untouched", desk_toml.read_bytes() == corrupt)
            shot(page, "desk-failure")
            page.click(".desk-failure button:has-text('Start a new desk')")
            try:
                failure.wait_for(state="hidden", timeout=10000)
                gone = True
            except Exception:
                gone = False
            aside = sorted(p.name for p in Path(daemon_dir).glob("desk.toml.unreadable-*"))
            check("Start a new desk moves the old file aside", len(aside) == 1, f"aside={aside}")
            check(
                "…keeping its bytes",
                len(aside) == 1 and Path(daemon_dir, aside[0]).read_bytes() == corrupt,
            )
            check("…and the notice goes away", gone)

            # --- scenario 2: a failed project read after a good one ----------
            row = page.locator("li.project")
            row.first.wait_for(timeout=15000)
            rows_before = row.count()
            page.route("**/api/repos", lambda route: route.fulfill(status=500, body="boom"))
            registry = Path(daemon_dir, "repos.toml")
            # Same bytes, new change time: the daemon's stat sees a change.
            time.sleep(1.1)
            registry.write_bytes(registry.read_bytes() + b"\n")
            stale = page.locator(".side-error", has_text="Not current")
            try:
                stale.first.wait_for(state="visible", timeout=10000)
                marked = True
            except Exception:
                marked = False
            check(
                "repos.dirty re-reads, and the failure marks the list not current",
                marked,
                f"text={stale.first.inner_text() if marked else ''!r}",
            )
            check("…while the project row stays", row.count() == rows_before and rows_before >= 1, f"rows={row.count()}")
            shot(page, "projects-not-current")
            page.unroute("**/api/repos")

            # --- scenario 4: unsaved work holds the reload back ---------------
            page.evaluate(
                "([project]) => " f"{SH}.openTab({{ project, path: 'README.md', title: 'README.md', ftype: 'code' }})",
                [slug],
            )
            page.wait_for_function(
                "() => { const m = window.monaco && window.monaco.editor.getModels()"
                ".find(m => m.uri.path.endsWith('README.md')); return !!m && !!document.querySelector('.monaco-editor'); }",
                timeout=30000,
            )
            page.click(".code-viewer .view-lines")
            page.keyboard.press("Control+Home")
            page.keyboard.type("unsaved line\n")
            check("the edit is unsaved", page.evaluate("() => window.WBViewer.anyDirty()"))
            loads = {"n": 0}
            page.on("framenavigated", lambda f: loads.__setitem__("n", loads["n"] + 1) if f == page.main_frame else None)
            page.evaluate(f"() => {{ {SH}.pageBuild = 'old-build'; }}")
            notice = page.locator(".build-notice")
            try:
                notice.wait_for(state="visible", timeout=8000)
                up = True
            except Exception:
                up = False
            check(
                "a build mismatch with unsaved work shows the notice",
                up and "Ralphy was updated" in notice.inner_text(),
            )
            page.wait_for_timeout(6000)
            check("…which stays for 6 s", notice.is_visible())
            check("…and the page does not reload", loads["n"] == 0, f"navigations={loads['n']}")
            check(
                "…and the change-set writes are locked",
                page.evaluate(f"() => {SH}.writeLocked()"),
            )
            shot(page, "build-notice")
            ctx.close()

            # --- scenario 3: another build reloads a tab with nothing unsaved -
            ctx3 = browser.new_context(viewport={"width": 1400, "height": 900})
            page3 = ctx3.new_page()
            served = {"n": 0}
            page3.on("request", lambda r: served.__setitem__("n", served["n"] + 1) if r.url == BASE else None)
            # The first load runs as an older build: its `ralphy-build` tag is
            # rewritten before app.js reads it. Not `page.route`: a fulfilled
            # document fails Chromium's local network checks, and its
            # WebSocket never opens. `sessionStorage` survives the reload, so
            # the second load keeps the daemon's own id.
            page3.add_init_script(
                """
                if (!sessionStorage.getItem("wb511-old")) {
                  sessionStorage.setItem("wb511-old", "1");
                  new MutationObserver((_, obs) => {
                    const m = document.querySelector('meta[name="ralphy-build"]');
                    if (m) { m.content = "old-build"; obs.disconnect(); }
                  }).observe(document, { childList: true, subtree: true });
                }
                """
            )
            page3.goto(BASE)
            deadline = time.time() + 15
            while time.time() < deadline and served["n"] < 2:
                page3.wait_for_timeout(250)
            page3.wait_for_timeout(5000)
            check("a page from another build reloads itself once", served["n"] == 2, f"served={served['n']}")
            ctx3.close()

            # --- scenario 5: a detached window's unsaved edit survives the reload
            ctx5 = browser.new_context(viewport={"width": 1400, "height": 900})
            page5 = ctx5.new_page()
            page5.on("pageerror", lambda e: errors.append(str(e)))
            page5.goto(BASE)
            page5.wait_for_selector("[x-data]", timeout=15000)
            page5.wait_for_timeout(1500)
            page5.evaluate(
                "([project]) => " f"{SH}.openTab({{ project, path: 'README.md', title: 'README.md', ftype: 'code' }})",
                [slug],
            )
            page5.wait_for_function(
                "() => !!document.querySelector('.code-viewer .monaco-editor')",
                timeout=30000,
            )
            with ctx5.expect_page(timeout=15000) as pop_info:
                page5.locator(".code-viewer").locator("xpath=..").get_by_role("button", name="Detach").first.click()
            pop = pop_info.value
            pop.on("pageerror", lambda e: errors.append(str(e)))
            pop.wait_for_function(
                "() => !!document.querySelector('.code-viewer .monaco-editor')",
                timeout=30000,
            )
            pop.click(".code-viewer .view-lines")
            pop.keyboard.press("Control+Home")
            pop.keyboard.type("detached edit\n")
            check("the detached window holds an unsaved edit", pop.evaluate("() => window.WBViewer.anyDirty()"))
            loads5 = {"n": 0}
            page5.on("framenavigated", lambda f: loads5.__setitem__("n", loads5["n"] + 1) if f == page5.main_frame else None)
            page5.evaluate(f"() => {{ {SH}.pageBuild = 'old-build'; }}")
            notice5 = page5.locator(".build-notice")
            try:
                notice5.wait_for(state="visible", timeout=8000)
                up5 = True
            except Exception:
                up5 = False
            check("…the main tab shows the notice instead of reloading", up5 and loads5["n"] == 0, f"navigations={loads5['n']}")
            pop.get_by_role("button", name="Save").first.click()
            deadline = time.time() + 10
            saved = False
            while time.time() < deadline and not saved:
                saved = "detached edit" in Path(fixture, "README.md").read_text(encoding="utf-8")
                if not saved:
                    page5.wait_for_timeout(250)
            check("…the window's Save writes the file", saved)
            deadline = time.time() + 15
            while time.time() < deadline and loads5["n"] < 1:
                page5.wait_for_timeout(250)
            check("…then the main tab reloads", loads5["n"] >= 1, f"navigations={loads5['n']}")
            check("…and the detached window is closed", pop.is_closed())
            ctx5.close()
            browser.close()
            check("no page error", not errors, f"errors={errors[:3]}")
    finally:
        stop(proc)

    ok = all(results) and len(results) == 20
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("ALL CHECKS PASSED")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
