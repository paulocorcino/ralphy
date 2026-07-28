"""#305 browser acceptance: the fixed canvas tab is Consoles, translation is gone.

One Playwright pass over a REAL daemon (no mocks): the fixed tab 0 renders as
"Consoles" (display name AND `id`), stays non-closable, and file tabs still
ride in after it as closable tabs. Separately proves the removed on-device
translation feature left no reachable surface: no `window.WBTranslate`, no
`.plan-xlate` / `[data-act="xlate"]` / `.md-xlate-note` element anywhere, and
`GET /wb-translate.js` 404s.

Scenario 1  tab strip has exactly one tab; its `.tab-title` reads "Consoles"
Scenario 2  that tab renders no `.tab-close`
Scenario 3  opening README.md appends a second tab AFTER it: tabs[0].id is
            still "consoles", the new tab is closable and shows a `.tab-close`
Scenario 4  `window.WBTranslate` is undefined
Scenario 5  `.plan-xlate` / `[data-act="xlate"]` / `.md-xlate-note` each count
            0, with the Runs panel open AND the markdown tab open
Scenario 6  `GET /wb-translate.js` -> 404
Scenario 7  zero `pageerror` events were captured over the whole pass

Boots a Localhost daemon on 7395 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as
the orchestrator on this host).

Writes docs/screenshots/305-consoles-tab-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_consoles_305.py   (exit 0 = all pass)
"""

import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

# The Windows console's default codepage (cp1252 here) cannot encode the glyphs
# this script prints in its detail strings; force utf-8 stdout so a PASSING
# assertion never dies on its own detail.
sys.stdout.reconfigure(encoding="utf-8")

PORT = 7395
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_consoles_305.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
WIN = os.name == "nt"
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if WIN else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
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


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb305_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #305 consoles-tab fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb305@example.com"],
        ["git", "config", "user.name", "wb305"],
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
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's bundle.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(["cargo", "build", "-p", "ralphy-daemon", "--bins"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb305_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            # A bare `return` here would skip the exit gate below and report
            # success with ZERO browser assertions run.
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows (KNOWLEDGE.md).
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page_errors = []
            page.on("pageerror", lambda exc: page_errors.append(str(exc)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)

            # --- scenario 1: the fixed tab is named Consoles -------------------
            tabs = page.locator(".tabstrip .tab")
            check("the tab strip renders exactly one tab", tabs.count() == 1, f"got={tabs.count()}")
            check(
                "the tab's title reads exactly Consoles",
                tabs.first.locator(".tab-title").inner_text() == "Consoles",
                f"got={tabs.first.locator('.tab-title').inner_text()!r}",
            )

            # --- scenario 2: the Consoles tab is not closable -------------------
            # `.tab-close` is `x-show`-gated (display:none, not removed), so a bare
            # `.count()` would pass on a hidden element — assert visibility.
            check(
                "the Consoles tab renders no VISIBLE close button",
                tabs.first.locator(".tab-close:visible").count() == 0,
                "",
            )

            # --- scenario 3: a file tab rides in after it, closable -------------
            page.evaluate(f"() => {{ {SH}.openSlug = '{slug}'; }}")
            page.wait_for_timeout(200)
            page.evaluate(
                f"""() => {{
                    const s = {SH};
                    s.openTab({{ project: '{slug}', path: 'README.md', title: 'README.md', ftype: 'markdown' }});
                }}"""
            )
            page.wait_for_timeout(500)
            tab_state = page.evaluate(f"() => {SH}.tabs.map((t) => ({{ id: t.id, closable: t.closable }}))")
            check(
                "tab 0 stays the fixed consoles tab after a file opens",
                len(tab_state) == 2 and tab_state[0] == {"id": "consoles", "closable": False},
                f"got={tab_state}",
            )
            check(
                "the new file tab is closable",
                tab_state[1]["closable"] is True,
                f"got={tab_state[1]}",
            )
            tabs = page.locator(".tabstrip .tab")
            check(
                "the second .tab renders a visible close button",
                tabs.nth(1).locator(".tab-close:visible").count() > 0,
                "",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "305-consoles-tab-2026-07-25.png"))

            # --- scenario 4: WBTranslate is gone from the runtime ---------------
            check(
                "window.WBTranslate is undefined",
                page.evaluate("() => typeof window.WBTranslate") == "undefined",
                "",
            )

            # --- scenario 5: no translate control anywhere in the DOM -----------
            page.evaluate(f"() => {{ {SH}.toggleRuns(); }}")
            page.wait_for_timeout(300)
            check(".plan-xlate count is 0", page.locator(".plan-xlate").count() == 0, "")
            check('[data-act="xlate"] count is 0', page.locator('[data-act="xlate"]').count() == 0, "")
            check(".md-xlate-note count is 0", page.locator(".md-xlate-note").count() == 0, "")

            # --- scenario 6: the module itself 404s ------------------------------
            resp = page.request.get(BASE + "wb-translate.js")
            check("GET /wb-translate.js returns 404", resp.status == 404, f"got={resp.status}")

            ctx.close()
            browser.close()

            # --- scenario 7: no uncaught error over the whole pass ---------------
            check("zero pageerror events captured", page_errors == [], f"got={page_errors}")
    finally:
        stop(proc)

    # The count floor is load-bearing: an early `sys.exit` or a scenario that
    # never ran must not report success on a handful of passing checks.
    ok = all(results) and len(results) >= 11
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CONSOLES TAB")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
