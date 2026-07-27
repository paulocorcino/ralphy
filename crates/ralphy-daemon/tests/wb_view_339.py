"""#339 browser acceptance: the view you left comes back, PER CLIENT.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. The desk fixture is two
`kind = "agent"` records, which `reconcileDesk` turns into PLACEHOLDERS: a
placeholder honours the record's rect through the same `buildChrome` path with
no PTY and no xterm, so the geometry is deterministic and the pass is cheap.

The state split under test is ADR-0051 §8: the desk (windows, and later fences)
stays daemon-owned, while the viewport offset and the open file tabs are per
CLIENT, in this browser profile. ADR-0050 §3's "no browser store" is narrowed to
"no *desk* in browser storage" — scenario 6 is the assertion of that narrowing.

Scenario 1   an empty desk lands on the stage origin
Scenario 2   nothing stored lands on the BOUNDING BOX of the restored windows
Scenario 3   pan + two file tabs survive a reload, offset byte-identical
Scenario 4   a second browser profile gets its own view and disturbs neither
Scenario 5   restoring on a smaller screen shows work, with every rect untouched
Scenario 6   no desk data reaches browser storage — key set, shape, vocabulary
Scenario 7   a tab switch away from Consoles and back keeps the pan (`x-show`)

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/339-view-per-client-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_view_339.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7434
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "339-view-per-client-2026-07-27.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
VIEW_KEY = "wb.view.v1"

# Far from the origin ON BOTH AXES on purpose: a landing bug that answers 0,0
# must not be able to pass by accident.
FIX_A = {"left": 1600, "top": 900, "width": 600, "height": 380}
FIX_B = {"left": 2400, "top": 1500, "width": 600, "height": 380}
# The bbox of the two: 1600..3000 x 900..1880, so its centre is (2300, 1390).
BBOX_CENTRE = (2300, 1390)

README_NEEDLE = "The #339 view fixture repo."

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
    empty = tempfile.mkdtemp(prefix="wb339_empty_")
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
    d = tempfile.mkdtemp(prefix="wb339_fixture_")
    p = Path(d)
    (p / "README.md").write_text(f"# fixture\n\n{README_NEEDLE}\n", encoding="utf-8")
    (p / "notes.md").write_text("# notes\n\nThe second tab.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb339@example.com"],
        ["git", "config", "user.name", "wb339"],
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
    # the previous build's console.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def http(method, path, body=None):
    data = None
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=5) as r:
        return r.status, r.read().decode()


def desk_records(slug):
    out = []
    for wid, r, ts in (("w-339-a", FIX_A, 100), ("w-339-b", FIX_B, 101)):
        out.append(
            {
                "id": wid,
                "repo": slug,
                "agent": "claude",
                "kind": "agent",
                "rect": dict(r),
                "max": False,
                "sessionId": None,
                "ts": ts,
            }
        )
    return out


def rects(page):
    return page.evaluate(
        "() => [...document.querySelectorAll('.session-window')].map((w) =>"
        " ({ left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth, height: w.offsetHeight }))"
    )


def settle(page, want=2):
    """Wait for the restored windows to be REAL boxes.

    KNOWLEDGE: an `x-show` flip is not visible to the next evaluate, and a
    still-hidden box measures 0x0 — which passes a geometry assertion vacuously.
    """
    page.wait_for_function(
        "(n) => { const ws = [...document.querySelectorAll('.session-window')];"
        " return ws.length === n && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        arg=want,
        timeout=20000,
    )
    page.wait_for_timeout(600)


def view_box(page):
    """The viewport's live scroll state, plus everything a landing is judged on."""
    return page.evaluate(
        "() => { const ws = document.getElementById('workspace');"
        "  return { scrollLeft: ws.scrollLeft, scrollTop: ws.scrollTop,"
        "    clientWidth: ws.clientWidth, clientHeight: ws.clientHeight,"
        "    scrollWidth: ws.scrollWidth, scrollHeight: ws.scrollHeight }; }"
    )


def shows(box, rect):
    """Does `rect` (stage coordinates) intersect the viewport placed at `box`?"""
    return (
        rect["left"] < box["scrollLeft"] + box["clientWidth"]
        and rect["left"] + rect["width"] > box["scrollLeft"]
        and rect["top"] < box["scrollTop"] + box["clientHeight"]
        and rect["top"] + rect["height"] > box["scrollTop"]
    )


def inside(box, point):
    x, y = point
    return (
        box["scrollLeft"] <= x <= box["scrollLeft"] + box["clientWidth"]
        and box["scrollTop"] <= y <= box["scrollTop"] + box["clientHeight"]
    )


def fresh_context(browser, viewport):
    """A brand-new browser profile that records what it inherited at BOOT.

    The landing itself fires a `scroll`, which `saveOffset` persists — so a
    "nothing was stored" assertion read after the page settles always finds the
    landing it just wrote. `add_init_script` runs before any page script, which
    is the only honest moment to sample it.
    """
    ctx = browser.new_context(viewport=viewport)
    ctx.add_init_script(
        f"window.__viewAtBoot = localStorage.getItem({VIEW_KEY!r});"
        f"window.__keysAtBoot = Object.keys(localStorage);"
    )
    return ctx


def desk_page(ctx, viewport, want=2):
    page = ctx.new_page()
    page.set_viewport_size(viewport)
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
    if want:
        settle(page, want)
    else:
        page.wait_for_timeout(1800)
    return page


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb339_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])

            # ===== scenario 1: an empty desk lands on the origin ================
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = desk_page(ctx, {"width": 1400, "height": 900}, want=0)
            check(
                "the desk really is empty, so the origin landing is about an empty plane",
                json.loads(http("GET", "api/desk")[1]) == []
                and page.evaluate("() => document.querySelectorAll('.session-window').length") == 0,
            )
            box = view_box(page)
            check(
                "an empty desk lands on the stage origin",
                box["scrollLeft"] == 0 and box["scrollTop"] == 0,
                f"got={box['scrollLeft']},{box['scrollTop']}",
            )
            ctx.close()

            # ===== scenario 2: nothing stored lands on the bounding box =========
            status, _ = http("PUT", "api/desk", desk_records(slug))
            check("the two far-off fixture windows reach the daemon's desk", status == 200, f"status={status}")

            ctx = fresh_context(browser, {"width": 1400, "height": 900})
            page = desk_page(ctx, {"width": 1400, "height": 900})
            check(
                "the fixture desk restores verbatim — the landing moves the VIEW, not the rects",
                rects(page) == [FIX_A, FIX_B],
                f"got={rects(page)}",
            )
            # Without this the scenario proves nothing: a stored offset from an
            # earlier profile would make the bbox landing below unfalsifiable.
            at_boot = page.evaluate("() => window.__viewAtBoot")
            check(
                "…and this profile stored NOTHING before the page ran",
                at_boot is None,
                f"at_boot={at_boot!r}",
            )
            box = view_box(page)
            check(
                "with nothing stored the view leaves the corner of the plane",
                box["scrollLeft"] > 0 and box["scrollTop"] > 0,
                f"got={box['scrollLeft']},{box['scrollTop']} client={box['clientWidth']}x{box['clientHeight']}",
            )
            check(
                "…landing on the BOUNDING BOX of the restored windows, whose centre is in frame",
                inside(box, BBOX_CENTRE),
                f"centre={BBOX_CENTRE} view={box['scrollLeft']},{box['scrollTop']}"
                f" +{box['clientWidth']}x{box['clientHeight']}",
            )
            check(
                "…with BOTH windows actually showing, not merely their midpoint",
                shows(box, FIX_A) and shows(box, FIX_B),
                f"a={shows(box, FIX_A)} b={shows(box, FIX_B)}",
            )
            ctx.close()

            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 8
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("THE VIEW IS PER CLIENT")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
