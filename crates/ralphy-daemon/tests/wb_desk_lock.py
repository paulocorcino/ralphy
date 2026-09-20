"""Lock amendment (ADR-0050 / ADR-0051, 2026-09-20) browser acceptance: a drag
starts only past a threshold, and a console or a fence can be locked in place.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7460, so this can run
beside the other suites without either daemon stealing the other's port.

The fixture `desk.toml` is written BEFORE the daemon starts: one fence holding
one console (its centre inside the fence) and one free console clear of it.
`kind = "agent"` restores as a PLACEHOLDER: full chrome and deterministic
geometry, no PTY, no vendor CLI.

Scenario 1  a finger that slips 6px on a titlebar moves NOTHING, and desk.toml
            is byte-identical after — a tap is not a drag, and it does not
            refresh `ts` either
Scenario 2  the same with a mouse and 2px
Scenario 3  a 30px touch drag lands the full 30px: the threshold delays the
            start, it does not swallow the delta
Scenario 4  the console's lock button: the drag and the corner resize are
            refused, the bands are gone, maximize still works; a reload keeps
            the lock and desk.toml carries `locked = true`; unlock frees it and
            the key leaves the file
Scenario 5  the fence's lock button: the grab and an edge are refused, the
            member console refuses a drag while the free console still moves,
            tile is disabled; a reload keeps it; unlock frees everything
Scenario 6  a SECOND browser context sees both locks — shared desk state, not
            a per-client preference

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_desk_lock.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7460
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
VIEW = {"width": 1400, "height": 900}

FENCE = {"left": 40, "top": 40, "width": 500, "height": 360}
MEMBER = {"left": 80, "top": 90, "width": 300, "height": 200}  # centre (230,190): inside
FREE = {"left": 700, "top": 450, "width": 300, "height": 200}  # clear of the fence

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
    empty = tempfile.mkdtemp(prefix="wblock_empty_")
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
    d = tempfile.mkdtemp(prefix="wblock_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe desk-lock fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wblock@example.com"],
        ["git", "config", "user.name", "wblock"],
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


def rect_toml(r):
    return "rect = { left = %(left)s, top = %(top)s, width = %(width)s, height = %(height)s }" % r


def window_toml(wid, slug, rect, ts):
    return (
        "[[windows]]\n"
        f'id = "{wid}"\n'
        f'repo = "{slug}"\n'
        'agent = "claude"\n'
        'kind = "agent"\n'
        "max = false\n"
        f"ts = {ts}\n"
        f"{rect_toml(rect)}\n\n"
    )


def write_fixture_desk(daemon_dir, slug):
    Path(daemon_dir, "desk.toml").write_text(
        window_toml("w-member", slug, MEMBER, 100)
        + window_toml("w-free", slug, FREE, 101)
        + "[[fences]]\n"
        'id = "f-pen"\n'
        'name = "pen"\n'
        "ts = 102\n"
        f"{rect_toml(FENCE)}\n",
        encoding="utf-8",
    )


def build():
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


def settle_windows(page, want):
    page.wait_for_function(
        "(n) => { const ws = [...document.querySelectorAll('.session-window')];"
        " return ws.length === n && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        arg=want,
        timeout=20000,
    )
    page.wait_for_timeout(500)


def desk_page(ctx):
    page = ctx.new_page()
    page.set_viewport_size(dict(VIEW))
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    page.wait_for_timeout(1800)
    settle_windows(page, 2)
    page.wait_for_function("() => document.querySelectorAll('.fence').length === 1", timeout=15000)
    page.evaluate(
        "() => { const ws = document.getElementById('workspace'); ws.scrollLeft = 0; ws.scrollTop = 0; }"
    )
    page.wait_for_timeout(200)
    return page


def boxes(page):
    return page.evaluate(
        "() => { const box = (el) => ({ left: el.offsetLeft, top: el.offsetTop,"
        "   width: el.offsetWidth, height: el.offsetHeight });"
        " const out = { fence: null, wins: {} };"
        " const f = document.querySelector('.fence');"
        " if (f) out.fence = { rect: box(f), locked: f.classList.contains('locked'),"
        "   tileDisabled: !!f.querySelector('.fence-arrange')?.disabled };"
        " for (const w of document.querySelectorAll('.session-window'))"
        "   out.wins[w._deskId] = { rect: box(w), locked: w.classList.contains('locked'),"
        "     max: w.classList.contains('maximized'),"
        "     handle: getComputedStyle(w.querySelector('.session-handle.h-se')).display };"
        " return out; }"
    )


# Synthetic pointer streams, dispatched exactly as the browser would: a
# `pointerdown` on the handle, moves and the up on the document.
POINTER_JS = (
    "([sel, kind, dx, dy, steps, pid]) => {"
    "  const h = document.querySelector(sel);"
    "  if (!h) return null;"
    "  const r = h.getBoundingClientRect();"
    "  let x = r.left + Math.min(30, r.width / 2), y = r.top + r.height / 2;"
    "  const ev = (type, target, cx, cy, buttons) => target.dispatchEvent("
    "    new PointerEvent(type, { pointerId: pid, pointerType: kind, isPrimary: true,"
    "      button: 0, buttons, clientX: cx, clientY: cy, bubbles: true, cancelable: true }));"
    "  ev('pointerdown', h, x, y, 1);"
    "  for (let s = 0; s < steps; s++) {"
    "    x += dx / steps; y += dy / steps;"
    "    ev('pointermove', document, x, y, 1);"
    "  }"
    "  ev('pointerup', document, x, y, 0);"
    "  return true;"
    "}"
)


def pointer_drag(page, selector, kind, dx, dy, steps=4, pid=7):
    ok = page.evaluate(POINTER_JS, [selector, kind, dx, dy, steps, pid])
    page.wait_for_timeout(350)
    return ok


def win_sel(wid):
    return f".session-window[data-desk-id='{wid}']"


def tag_windows(page):
    """Give each window a selectable attribute from its stable desk id."""
    page.evaluate(
        "() => { for (const w of document.querySelectorAll('.session-window'))"
        "  w.dataset.deskId = w._deskId; }"
    )


def quiet(desk_file, still=1.6, timeout=15):
    deadline = time.time() + timeout
    last = None
    since = time.time()
    while time.time() < deadline:
        try:
            now = (desk_file.stat().st_mtime_ns, desk_file.stat().st_size)
        except OSError:
            now = None
        if now != last:
            last = now
            since = time.time()
        elif time.time() - since >= still:
            return
        time.sleep(0.15)


def stored(desk_file, want, timeout=12):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        try:
            last = json.loads(http("GET", "api/desk")[1])
        except Exception:
            time.sleep(0.3)
            continue
        if want(last):
            return last
        time.sleep(0.3)
    return last


def record(desk, wid):
    for w in desk.get("windows", []):
        if w["id"] == wid:
            return w
    return None


def main():
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wblock_reg_")
    desk_file = Path(daemon_dir, "desk.toml")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    write_fixture_desk(daemon_dir, slug)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport=dict(VIEW), has_touch=True)
            page = desk_page(ctx)
            tag_windows(page)
            b0 = boxes(page)
            check(
                "the fixture restores two consoles and one fence at the seeded rects",
                b0["wins"]["w-member"]["rect"] == MEMBER
                and b0["wins"]["w-free"]["rect"] == FREE
                and b0["fence"]["rect"] == FENCE,
                f"got={b0}",
            )
            quiet(desk_file)
            before = desk_file.read_text(encoding="utf-8")

            # ===== scenario 1: a finger's slip is a tap =========================
            pointer_drag(page, win_sel("w-free") + " .session-titlebar", "touch", 4, 4, steps=2)
            b = boxes(page)
            check(
                "a 6px finger slip on a titlebar moves the console NOT AT ALL",
                b["wins"]["w-free"]["rect"] == FREE,
                f"got={b['wins']['w-free']['rect']}",
            )
            quiet(desk_file)
            check(
                "…and desk.toml is byte-identical: a tap does not refresh ts",
                desk_file.read_text(encoding="utf-8") == before,
            )

            # ===== scenario 2: a mouse under 4px ================================
            pointer_drag(page, win_sel("w-free") + " .session-titlebar", "mouse", 2, 1, steps=2, pid=1)
            b = boxes(page)
            check(
                "a 2px mouse wobble on a titlebar moves nothing either",
                b["wins"]["w-free"]["rect"] == FREE,
                f"got={b['wins']['w-free']['rect']}",
            )
            quiet(desk_file)
            check("…and persists nothing", desk_file.read_text(encoding="utf-8") == before)

            # ===== scenario 3: past the threshold the whole delta lands =========
            pointer_drag(page, win_sel("w-free") + " .session-titlebar", "touch", 30, 20, steps=4)
            b = boxes(page)
            got = b["wins"]["w-free"]["rect"]
            check(
                "a 30x20 touch drag lands the full delta — the threshold delays, it does not swallow",
                abs(got["left"] - (FREE["left"] + 30)) <= 1 and abs(got["top"] - (FREE["top"] + 20)) <= 1,
                f"got={got}",
            )
            desk = stored(desk_file, lambda d: (record(d, "w-free") or {}).get("rect", {}).get("left") != FREE["left"])
            check(
                "…and THAT gesture persisted",
                abs(record(desk, "w-free")["rect"]["left"] - (FREE["left"] + 30)) <= 1,
                f"stored={record(desk, 'w-free')['rect']}",
            )
            moved_free = dict(got)
            # put it back for the later scenarios, by the same gesture
            pointer_drag(page, win_sel("w-free") + " .session-titlebar", "touch", -30, -20, steps=4)
            stored(desk_file, lambda d: (record(d, "w-free") or {}).get("rect", {}).get("left") == FREE["left"])

            # ===== scenario 4: lock a console ===================================
            page.locator(win_sel("w-free") + " .session-lock").click()
            page.wait_for_timeout(300)
            b = boxes(page)
            check(
                "the lock button locks the console: class on, resize bands gone",
                b["wins"]["w-free"]["locked"] and b["wins"]["w-free"]["handle"] == "none",
                f"got={b['wins']['w-free']}",
            )
            pointer_drag(page, win_sel("w-free") + " .session-titlebar", "touch", 60, 40, steps=4)
            b = boxes(page)
            check(
                "a 60px touch drag on a locked console is refused",
                b["wins"]["w-free"]["rect"] == FREE,
                f"got={b['wins']['w-free']['rect']}",
            )
            pointer_drag(page, win_sel("w-free") + " .session-handle.h-se", "mouse", 60, 40, steps=4, pid=1)
            b = boxes(page)
            check(
                "…and so is a resize through its (hidden) corner band",
                b["wins"]["w-free"]["rect"] == FREE,
                f"got={b['wins']['w-free']['rect']}",
            )
            page.locator(win_sel("w-free") + " .session-max").click()
            page.wait_for_timeout(300)
            b = boxes(page)
            check("maximize still works on a locked console", b["wins"]["w-free"]["max"])
            page.locator(win_sel("w-free") + " .session-max").click()
            page.wait_for_timeout(300)
            b = boxes(page)
            check(
                "…and restore brings back the locked rect untouched",
                not b["wins"]["w-free"]["max"] and b["wins"]["w-free"]["rect"] == FREE,
                f"got={b['wins']['w-free']}",
            )
            desk = stored(desk_file, lambda d: (record(d, "w-free") or {}).get("locked") is True)
            check("the lock is served by the daemon", record(desk, "w-free").get("locked") is True)
            text = desk_file.read_text(encoding="utf-8")
            check("…and written to desk.toml as `locked = true`", "locked = true" in text)

            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
            page.wait_for_timeout(1800)
            settle_windows(page, 2)
            tag_windows(page)
            b = boxes(page)
            check(
                "a reload restores the console LOCKED",
                b["wins"]["w-free"]["locked"] and b["wins"]["w-free"]["handle"] == "none",
                f"got={b['wins']['w-free']}",
            )
            page.locator(win_sel("w-free") + " .session-lock").click()
            page.wait_for_timeout(300)
            pointer_drag(page, win_sel("w-free") + " .session-titlebar", "touch", 30, 20, steps=4)
            b = boxes(page)
            check(
                "unlock frees it: the same drag moves it again",
                not b["wins"]["w-free"]["locked"]
                and abs(b["wins"]["w-free"]["rect"]["left"] - (FREE["left"] + 30)) <= 1,
                f"got={b['wins']['w-free']}",
            )
            desk = stored(desk_file, lambda d: (record(d, "w-free") or {}).get("locked") is None)
            check(
                "…and the key leaves the wire and the file",
                "locked" not in record(desk, "w-free") and "locked = true" not in desk_file.read_text(encoding="utf-8"),
                f"record={record(desk, 'w-free')}",
            )
            pointer_drag(page, win_sel("w-free") + " .session-titlebar", "touch", -30, -20, steps=4)
            stored(desk_file, lambda d: (record(d, "w-free") or {}).get("rect", {}).get("left") == FREE["left"])

            # ===== scenario 5: lock a fence =====================================
            page.locator(".fence .fence-lock").click()
            page.wait_for_timeout(300)
            b = boxes(page)
            check(
                "the fence's lock button locks it: class on, tile disabled",
                b["fence"]["locked"] and b["fence"]["tileDisabled"],
                f"got={b['fence']}",
            )
            pointer_drag(page, ".fence .fence-grab", "touch", 80, 50, steps=4)
            b = boxes(page)
            check(
                "a drag by the grab is refused, and the member did not move with it",
                b["fence"]["rect"] == FENCE and b["wins"]["w-member"]["rect"] == MEMBER,
                f"got fence={b['fence']['rect']} member={b['wins']['w-member']['rect']}",
            )
            pointer_drag(page, ".fence .fence-edge[data-dir='e']", "mouse", 80, 0, steps=4, pid=1)
            b = boxes(page)
            check("an edge drag is refused too", b["fence"]["rect"] == FENCE, f"got={b['fence']['rect']}")
            pointer_drag(page, win_sel("w-member") + " .session-titlebar", "touch", 60, 40, steps=4)
            b = boxes(page)
            check(
                "the console INSIDE the locked fence refuses a drag — the lock freezes the group",
                b["wins"]["w-member"]["rect"] == MEMBER and b["wins"]["w-member"]["handle"] == "none",
                f"got={b['wins']['w-member']}",
            )
            pointer_drag(page, win_sel("w-free") + " .session-titlebar", "touch", 30, 20, steps=4)
            b = boxes(page)
            check(
                "…while the console OUTSIDE it still moves",
                abs(b["wins"]["w-free"]["rect"]["left"] - (FREE["left"] + 30)) <= 1,
                f"got={b['wins']['w-free']['rect']}",
            )
            pointer_drag(page, win_sel("w-free") + " .session-titlebar", "touch", -30, -20, steps=4)
            page.locator(".fence .fence-arrange").click(force=True)
            page.wait_for_timeout(400)
            b = boxes(page)
            check(
                "tile on a locked fence changes no rect",
                b["wins"]["w-member"]["rect"] == MEMBER,
                f"got={b['wins']['w-member']['rect']}",
            )
            desk = stored(desk_file, lambda d: (d.get("fences") or [{}])[0].get("locked") is True)
            check("the fence lock is served by the daemon", desk["fences"][0].get("locked") is True)

            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
            page.wait_for_timeout(1800)
            settle_windows(page, 2)
            tag_windows(page)
            b = boxes(page)
            check(
                "a reload restores the fence LOCKED, with the member's bands gone",
                b["fence"]["locked"] and b["fence"]["tileDisabled"] and b["wins"]["w-member"]["handle"] == "none",
                f"got fence={b['fence']} member={b['wins']['w-member']}",
            )

            # ===== scenario 6: a second context sees the lock ===================
            ctx2 = browser.new_context(viewport=dict(VIEW))
            page2 = desk_page(ctx2)
            tag_windows(page2)
            b2 = boxes(page2)
            check(
                "a second browser sees the fence locked: shared desk state, not a per-client preference",
                b2["fence"]["locked"],
                f"got={b2['fence']}",
            )
            ctx2.close()

            page.locator(".fence .fence-lock").click()
            page.wait_for_timeout(300)
            pointer_drag(page, win_sel("w-member") + " .session-titlebar", "touch", 30, 20, steps=4)
            b = boxes(page)
            check(
                "unlocking the fence frees its member",
                not b["fence"]["locked"]
                and abs(b["wins"]["w-member"]["rect"]["left"] - (MEMBER["left"] + 30)) <= 1,
                f"got={b}",
            )
            desk = stored(desk_file, lambda d: (d.get("fences") or [{}])[0].get("locked") is None)
            check("…and the fence's key leaves the wire", "locked" not in desk["fences"][0])

            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed")
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
