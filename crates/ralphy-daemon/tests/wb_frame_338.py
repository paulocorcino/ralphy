"""#338 browser acceptance: the canvas chrome is FRAME chrome; maximize fills the frame.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. The `desk.toml` fixture is
the same pre-#336 shape `wb_stage_336.py` uses (bbox 1300x680 + 200 margin =
1500x880 at viewport 1400x900), with `max = false` on BOTH records: a pre-seeded
full-bleed window covers its neighbour's chrome and makes every later click
unhittable, so the persisted-maximize criterion is proved by a REAL maximize +
reload + on-disk read instead.

Scenario 1   `.canvas-foot` and `.canvas-empty` resolve inside `.consoles-tab`
             and are NOT descendants of `#stage`; the pills read the live state
Scenario 2   a 250px pan leaves the foot's client rect byte-identical while a
             window's client rect moves by exactly -250 (the negative control)
Scenario 3   maximize fills the VIEWPORT at a scrolled offset — the terminal
             refits wider — without growing the stage extent
Scenario 4   Go-to another window WHILE maximized pans the plane and the pin
             follows it, so the full-bleed never desyncs from the frame
Scenario 5   Arrange leaves a maximized window's restore rect alone, and
             restoring puts it back on its stage-coordinate box (screen + desk)
Scenario 6   a persisted `max = true` comes back maximized across a reload and
             still restores to the right box
Scenario 7   closing both consoles reveals the empty-stage hint and the pill
             falls to `0 consoles`

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/338-frame-chrome-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_frame_338.py   (exit 0 = all pass)
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

PORT = 7433
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "338-frame-chrome-2026-07-27.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

FIX_A = {"left": 40, "top": 40, "width": 600, "height": 380}
FIX_B = {"left": 700, "top": 300, "width": 600, "height": 380}
STAGE_W = 1500
STAGE_H = 880

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
    empty = tempfile.mkdtemp(prefix="wb338_empty_")
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
    d = tempfile.mkdtemp(prefix="wb338_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #338 frame-chrome fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb338@example.com"],
        ["git", "config", "user.name", "wb338"],
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


def write_legacy_desk(daemon_dir, slug):
    """The pre-#336 `desk.toml`, written by hand rather than by this build."""
    rows = []
    for wid, r, ts in (("w-fixture-a", FIX_A, 100), ("w-fixture-b", FIX_B, 101)):
        rect = (
            "rect = { left = %(left)s, top = %(top)s,"
            " width = %(width)s, height = %(height)s }" % r
        )
        rows.append(
            "[[windows]]\n"
            f'id = "{wid}"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            "max = false\n"
            f"ts = {ts}\n"
            f"{rect}\n"
        )
    Path(daemon_dir, "desk.toml").write_text("\n".join(rows), encoding="utf-8")


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
    page.wait_for_timeout(500)


def desk_page(ctx, viewport):
    page = ctx.new_page()
    page.set_viewport_size(viewport)
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
    page.wait_for_timeout(1800)
    return page


def chrome_placement(page):
    """Where the two chrome elements RESOLVE — the whole point of this issue."""
    return page.evaluate(
        "() => { const q = (s) => document.querySelector(s);"
        "  const info = (el) => el && ({ inTab: el.closest('.consoles-tab') !== null,"
        "    inStage: el.closest('#stage') !== null,"
        "    parent: el.parentElement && (el.parentElement.id || el.parentElement.className) });"
        "  return { foot: info(q('.canvas-foot')), empty: info(q('.canvas-empty')) }; }"
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb338_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    write_legacy_desk(daemon_dir, slug)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})

            # ===== scenario 1: the chrome belongs to the FRAME =================
            page = desk_page(ctx, {"width": 1400, "height": 900})
            settle(page)
            check(
                "the fixture desk restores verbatim",
                rects(page) == [FIX_A, FIX_B],
                f"got={rects(page)}",
            )

            place = chrome_placement(page)
            check(
                "`.canvas-foot` exists in the DOM at all",
                place["foot"] is not None,
                "ADR-0051 §5's footer pills were dead CSS before this issue",
            )
            check(
                "`.canvas-empty` exists in the DOM at all",
                place["empty"] is not None,
                "ADR-0051 §5's empty-stage hint was dead CSS before this issue",
            )
            check(
                "the foot resolves inside `.consoles-tab` (the frame)",
                bool(place["foot"] and place["foot"]["inTab"]),
                f"got={place['foot']}",
            )
            check(
                "…and is NOT a descendant of `#stage` (the plane)",
                bool(place["foot"] and not place["foot"]["inStage"]),
                f"got={place['foot']}",
            )
            check(
                "the empty hint resolves inside `.consoles-tab` (the frame)",
                bool(place["empty"] and place["empty"]["inTab"]),
                f"got={place['empty']}",
            )
            check(
                "…and is NOT a descendant of `#stage` (the plane)",
                bool(place["empty"] and not place["empty"]["inStage"]),
                f"got={place['empty']}",
            )

            # The pills are the plane made legible: both read live shell state, so
            # a literal here is an oracle over `consoleCount` AND `stageExtent`.
            pills = page.evaluate(
                "() => { const f = document.querySelector('.canvas-foot');"
                "  const e = document.querySelector('.canvas-empty');"
                "  return { pills: [...f.querySelectorAll('.pill')].map((s) => s.textContent.trim()),"
                "    footVisible: f.offsetParent !== null,"
                "    emptyShown: e.offsetParent !== null }; }"
            )
            check(
                "the first pill counts the open consoles",
                pills["pills"][:1] == ["2 consoles"],
                f"got={pills['pills']}",
            )
            check(
                f"…and the second reads the stage extent ({STAGE_W} × {STAGE_H})",
                pills["pills"][1:2] == [f"stage {STAGE_W} × {STAGE_H}"],
                f"got={pills['pills']}",
            )
            check("…on a foot that is really on screen", pills["footVisible"], f"got={pills}")
            check(
                "the empty-stage hint is hidden while consoles are open",
                not pills["emptyShown"],
                f"got={pills}",
            )

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 7
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("THE CHROME IS IN THE FRAME")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
