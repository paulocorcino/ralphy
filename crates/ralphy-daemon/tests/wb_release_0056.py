"""ADR-0056 browser acceptance: the release badge and the What's new panel.

One Playwright pass over a REAL daemon on a scratch store, proving the workbench
half of the release watch renders what the daemon computed and nothing it did
not.

The running binary is built from a working tree, so its standing is always
`ahead` and the daemon will never hand this page a gap — that is the feature
working, not a gap in the test. So the *rendering* is driven by poking the
Alpine state directly (the pattern every wb_*.py here uses), while the *daemon's
own answer* is asserted separately off `/api/release`.

Scenario a  a fresh page draws NO dot: this build is ahead of its tag, and a
            development build is never offered an update
Scenario b  the daemon's own /api/release agrees (`standing` is not `behind`)
Scenario c  a quiet (fixes-only) view draws the dot and the menu row
Scenario d  opening the panel lists the WHOLE gap, newest first, with the
            highlights of each release — not just the newest
Scenario e  closing it clears the dot for a quiet view (dismissed)
Scenario f  an urgent view's dot SURVIVES the dismissal
Scenario g  a release with no recorded summary says so rather than rendering
            an empty bullet list

Boots a Localhost daemon on 7443 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own registry and login policy are untouched. The daemon is stopped by
its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/0056-whats-new-2026-09-08.png.
Run: python crates/ralphy-daemon/tests/wb_release_0056.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7443
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
)
EXE = os.path.join(
    REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy"
)
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "0056-whats-new-2026-09-08.png")
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
    empty = tempfile.mkdtemp(prefix="wb0056_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
        # The watch must not reach github.com from a test.
        RALPHY_RELEASE_OFFLINE="1",
    )


def view(severity, gap):
    return {
        "current": "v0.1.0-rc.19",
        "channel": "rc",
        "standing": "behind",
        "severity": severity,
        "latest": gap[0]["version"] if gap else None,
        "gap": gap,
        "disabled": False,
    }


def entry(version, date, highlights):
    return {
        "version": version,
        "title": version,
        "date": date,
        "url": f"https://example.invalid/{version}",
        "kinds": ["new"] if highlights else [],
        "highlights": highlights,
    }


def main():
    daemon_dir = tempfile.mkdtemp(prefix="wb0056_store_")
    proc = subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        if not wait_listening(BASE):
            check("daemon listens", False, "never came up")
            return 1

        with urllib.request.urlopen(BASE + "api/release", timeout=5) as r:
            served = json.loads(r.read().decode("utf-8"))
        check(
            "b · the daemon does not offer a development build an update",
            served["standing"] != "behind" and served["severity"] == "none",
            f"standing={served['standing']} severity={served['severity']}",
        )

        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_page(viewport={"width": 1280, "height": 860})
            page.goto(BASE, wait_until="domcontentloaded")
            page.wait_for_function(f"() => !!{SH}", timeout=15000)
            page.wait_for_timeout(400)

            check(
                "a · a fresh page draws no dot",
                page.locator(".rel-dot").count() == 0
                or not page.locator(".rel-dot").first.is_visible(),
            )

            quiet = view("quiet", [entry("v0.1.0-rc.20", "2026-09-07", ["A fix."])])
            page.evaluate(f"{SH}.release = {json.dumps(quiet)}")
            page.wait_for_timeout(250)
            dot = page.locator(".rel-dot")
            check(
                "c · a quiet view draws the dot",
                dot.count() == 1
                and dot.first.is_visible()
                and dot.first.evaluate("el => el.clientWidth > 0"),
            )

            page.click(".avatar-btn")
            page.wait_for_timeout(200)
            row = page.locator(".dropdown-item.rel-item")
            check(
                "c · and the menu row naming the count",
                row.is_visible() and "1 new release" in row.inner_text(),
                row.inner_text() if row.count() else "",
            )

            gap = [
                entry("v0.1.0-rc.21", "2026-09-08", ["Paste a screenshot into a console."]),
                entry("v0.1.0-rc.20", "2026-09-07", []),
            ]
            page.evaluate(f"{SH}.release = {json.dumps(view('notable', gap))}")
            page.wait_for_timeout(200)
            page.click(".dropdown-item.rel-item")
            page.wait_for_timeout(300)

            versions = page.locator(".rel-version").all_inner_texts()
            check(
                "d · the whole gap, newest first",
                versions == ["v0.1.0-rc.21", "v0.1.0-rc.20"],
                str(versions),
            )
            check(
                "d · with the highlights of each release",
                "Paste a screenshot into a console."
                in page.locator(".rel-highlights").first.inner_text(),
            )
            # `x-show` hides rather than removes, so the release that DOES have
            # highlights keeps an invisible `.rel-none` in the DOM. Count what is
            # on screen, not what is in the tree.
            shown = page.locator(".rel-none:visible")
            check(
                "g · a release with no summary says so, and only that one",
                shown.count() == 1
                and "No summary was recorded" in shown.first.inner_text(),
                f"visible={shown.count()}",
            )

            os.makedirs(SHOT_DIR, exist_ok=True)
            page.screenshot(path=SHOT)

            page.click(".rel-modal .modal-x")
            page.wait_for_timeout(250)
            check(
                "e · a dismissed quiet view clears the dot",
                not page.locator(".rel-dot").first.is_visible(),
            )

            urgent = view("urgent", [entry("v0.1.0-rc.21", "2026-09-08", ["Closed a hole."])])
            urgent["gap"][0]["kinds"] = ["security"]
            page.evaluate(f"{SH}.release = {json.dumps(urgent)}")
            page.wait_for_timeout(250)
            check(
                "f · an urgent dot survives the dismissal",
                page.locator(".rel-dot").first.is_visible()
                and "urgent" in page.locator(".rel-dot").first.get_attribute("class"),
            )

            browser.close()
    finally:
        stop(proc)

    print(f"\nscreenshot: {SHOT}")
    passed = sum(1 for r in results if r)
    print(f"{passed}/{len(results)} passed")
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main())
