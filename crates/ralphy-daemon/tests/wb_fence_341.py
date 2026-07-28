"""#341 browser acceptance: a fence is a GROUP.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7436, so this can run
beside #340's suite without either daemon stealing the other's port.

The fixture `desk.toml` is written BEFORE the daemon starts, with two DISJOINT
fences and one window clear of both, at rects this file names — every geometric
assertion below is against those literals, not against whatever the shell
happened to draw.

Scenario 1  a window dragged so its CENTRE enters a fence becomes a member, and
            no `fenceId` is written anywhere: membership is derived
Scenario 2  dragged back out, it stops being one — same gesture, no extra step
Scenario 3  a fence dragged onto its neighbour is REFUSED: `fence-invalid` is on
            the element mid-gesture, and the stored rect is byte-identical after
Scenario 4  a fence moved by a known delta carries its member: the member's
            PERSISTED rect moved by exactly that delta, its size untouched
Scenario 5  resizing a fence resizes NO member, and a member whose centre falls
            outside the new rect leaves the fence — this is where the evidence
            screenshot is taken

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/341-fence-is-a-group-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_fence_341.py   (exit 0 = all pass)
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

PORT = 7436
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "341-fence-is-a-group-2026-07-27.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

VIEW = {"width": 1400, "height": 900}

# The fixture geometry. Both fences fit inside `#workspace` (~1052 px wide once
# the sidebar has taken its share), so every handle below is really on screen —
# a fence wider than that puts its own controls off the viewport, where
# `elementFromPoint` answers null and a drag lands nowhere (#340).
FENCE_A = {"left": 40, "top": 40, "width": 400, "height": 300}
FENCE_B = {"left": 600, "top": 40, "width": 400, "height": 300}
# Clear of both fences on Y: the window starts a member of NOTHING.
WIN = {"left": 40, "top": 450, "width": 300, "height": 200}
MOVE = {"dx": 120, "dy": 80}

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
    empty = tempfile.mkdtemp(prefix="wb341_empty_")
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
    d = tempfile.mkdtemp(prefix="wb341_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #341 fence-group fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb341@example.com"],
        ["git", "config", "user.name", "wb341"],
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


def write_fixture_desk(daemon_dir, slug):
    """One window clear of two disjoint fences. `kind = "agent"` restores as a
    PLACEHOLDER: full chrome and deterministic geometry, no PTY, no vendor CLI."""
    Path(daemon_dir, "desk.toml").write_text(
        "[[windows]]\n"
        'id = "w-fixture-a"\n'
        f'repo = "{slug}"\n'
        'agent = "claude"\n'
        'kind = "agent"\n'
        "max = false\n"
        "ts = 100\n"
        f"{rect_toml(WIN)}\n"
        "\n"
        "[[fences]]\n"
        'id = "f-alpha"\n'
        'name = "alpha"\n'
        "ts = 101\n"
        f"{rect_toml(FENCE_A)}\n"
        "\n"
        "[[fences]]\n"
        'id = "f-beta"\n'
        'name = "beta"\n'
        "ts = 102\n"
        f"{rect_toml(FENCE_B)}\n",
        encoding="utf-8",
    )


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


def settle_windows(page, want):
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


def desk_page(ctx):
    page = ctx.new_page()
    page.set_viewport_size(dict(VIEW))
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    # `activate` and not a raw `active =` write: only the former reaches
    # `refitAll()`, the path that re-applies a stored offset after `display:none`
    # threw the scroll position away (KNOWLEDGE, #339).
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    page.wait_for_timeout(1800)
    return page


def unscroll(page):
    """Pin the plane at 0,0 so a stage rect and a client rect differ only by the
    workspace's own origin — every point below is taken from a live
    `getBoundingClientRect`, but a mid-suite auto-pan would still move the boxes
    under a measurement taken a moment earlier."""
    page.evaluate(
        "() => { const ws = document.getElementById('workspace');"
        " ws.scrollLeft = 0; ws.scrollTop = 0; }"
    )
    page.wait_for_timeout(200)


def geometry(page):
    """Fences, windows and the DERIVED membership, in ONE evaluate.

    `fenceMembership` is called with what the SCREEN measures, so this asserts
    the shipped fold against the shipped layout, not against a fixture.
    """
    return page.evaluate(
        "() => { const box = (el) => ({ left: el.offsetLeft, top: el.offsetTop,"
        "   width: el.offsetWidth, height: el.offsetHeight });"
        " const fences = [...document.querySelectorAll('.fence')]"
        "   .map((f) => ({ id: f.dataset.fenceId, rect: box(f),"
        "      invalid: f.classList.contains('fence-invalid') }));"
        " const wins = [...document.querySelectorAll('.session-window')]"
        "   .map((w) => ({ id: w._deskId, rect: box(w) }));"
        " return { fences, wins,"
        "   membership: window.WBConsole.fenceMembership(fences, wins) }; }"
    )


def client_point(page, selector, index=0):
    return page.evaluate(
        "([sel, i]) => { const el = document.querySelectorAll(sel)[i];"
        " if (!el) return null;"
        " const r = el.getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2,"
        "   w: r.width, h: r.height }; }",
        [selector, index],
    )


def drag(page, start, dx, dy):
    page.mouse.move(start["x"], start["y"])
    page.mouse.down()
    page.mouse.move(start["x"] + dx / 3, start["y"] + dy / 3, steps=5)
    page.mouse.move(start["x"] + dx * 2 / 3, start["y"] + dy * 2 / 3, steps=5)
    page.mouse.move(start["x"] + dx, start["y"] + dy, steps=5)
    page.mouse.up()
    page.wait_for_timeout(400)


def quiet(desk_file, still=1.6, timeout=15):
    """Block until `desk.toml` has not changed for `still` seconds.

    A fixed sleep is the wrong synchroniser for "nothing was written": under
    load the shell's 250 ms flush can land AFTER the sleep, and the assertion
    then passes by reading before the wrong write. Waiting for the file to go
    QUIET proves the window really is over.
    """
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


def stored(desk_file, want=None, timeout=12):
    """The desk as the DAEMON holds it, once the flush has landed.

    `want` is a predicate on the served desk: poll until it holds rather than
    sleeping a guessed window. Without one, wait for the file to go quiet.
    """
    last = {"windows": [], "fences": []}
    deadline = time.time() + timeout
    if want is None:
        quiet(desk_file)
    while time.time() < deadline:
        try:
            last = json.loads(http("GET", "api/desk")[1])
        except Exception:
            time.sleep(0.3)
            continue
        if want is None or want(last):
            return last
        time.sleep(0.3)
    return last


def fence_of(desk, fid):
    for f in desk.get("fences", []):
        if f["id"] == fid:
            return f
    return None


def window_block(text, wid="w-fixture-a"):
    """The raw `[[windows]]` record for `wid`, verbatim out of desk.toml.

    Located by ID, not by "everything before the first `[[fences]]`": on a
    reorder that slice becomes the empty preamble and the grep below would go
    vacuously green.
    """
    for block in text.split("[[windows]]"):
        if f'id = "{wid}"' in block:
            return block.split("[[")[0]
    return None


def win_rect(desk, wid="w-fixture-a"):
    for w in desk.get("windows", []):
        if w["id"] == wid:
            return w["rect"]
    return None


def fence_block(text, fid):
    """The raw `[[fences]]` record for `fid`, verbatim out of desk.toml."""
    for block in text.split("[[fences]]"):
        if f'id = "{fid}"' in block:
            return block.split("[[")[0].strip()
    return None


def centre(r):
    return (r["left"] + r["width"] / 2, r["top"] + r["height"] / 2)


def inside(rect, point):
    return (
        rect["left"] <= point[0] < rect["left"] + rect["width"]
        and rect["top"] <= point[1] < rect["top"] + rect["height"]
    )


def draw_fence(page):
    """Draw a fence through the MERGED toolbar control (ADR-0051 §7 amendment).

    `Fence` is one button now: it opens a menu whose first row is the create
    verb and whose remaining rows are the map. Clicking the row closes the menu,
    so every call re-opens it — there is no stale-menu path to reuse.
    """
    page.locator("button[title='draw a fence, or jump to one']").click()
    page.wait_for_function(
        "() => { const m = document.querySelector('.fence-menu');"
        " return m && m.offsetParent !== null && m.clientWidth > 0; }",
        timeout=8000,
    )
    page.locator("button[title='draw a named fence on the plane']").click()


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb341_reg_")
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
            ctx = browser.new_context(viewport=dict(VIEW))
            page = desk_page(ctx)
            settle_windows(page, 1)
            page.wait_for_function(
                "() => document.querySelectorAll('.fence').length === 2", timeout=15000
            )
            unscroll(page)

            g = geometry(page)
            check(
                "the fixture restores one window and two fences at the seeded rects",
                len(g["wins"]) == 1
                and [f["rect"] for f in g["fences"]] == [FENCE_A, FENCE_B],
                f"wins={len(g['wins'])} fences={[f['rect'] for f in g['fences']]}",
            )
            check(
                "…with the window clear of both fences to start with",
                g["membership"] == {"f-alpha": [], "f-beta": []},
                f"got={g['membership']}",
            )
            win_id = g["wins"][0]["id"]

            # ===== scenario 1: drag a window IN ==============================
            # Land the window's centre inside fence alpha. The delta is computed
            # from the LIVE boxes, so an auto-pan or a chrome height that differs
            # from the fixture cannot silently aim the drag at the wrong place.
            target = (FENCE_A["left"] + 200, FENCE_A["top"] + 150)
            here = centre(g["wins"][0]["rect"])
            bar = client_point(page, ".session-window .session-titlebar")
            drag(page, bar, target[0] - here[0], target[1] - here[1])

            g = geometry(page)
            wrect = g["wins"][0]["rect"]
            check(
                "the drag put the window's CENTRE inside fence alpha",
                inside(FENCE_A, centre(wrect)),
                f"centre={centre(wrect)} fence={FENCE_A}",
            )
            check(
                "a window whose centre entered a fence IS a member — no extra gesture",
                g["membership"] == {"f-alpha": [win_id], "f-beta": []},
                f"got={g['membership']}",
            )
            in_desk = stored(desk_file, lambda d: win_rect(d) and win_rect(d)["left"] == wrect["left"])
            block = window_block(desk_file.read_text(encoding="utf-8"))
            check(
                "…and NOTHING about the fence was written into the window record",
                block is not None
                and "fence" not in block.lower()
                and "fence" not in json.dumps(in_desk["windows"]).lower(),
                f"block={block!r} record={json.dumps(in_desk['windows'])[:200]}",
            )

            # ===== scenario 2: drag it back OUT ==============================
            here = centre(wrect)
            bar = client_point(page, ".session-window .session-titlebar")
            back = (WIN["left"] + WIN["width"] / 2, WIN["top"] + WIN["height"] / 2)
            drag(page, bar, back[0] - here[0], back[1] - here[1])
            g = geometry(page)
            check(
                "dragged back out, the window stops being a member — same gesture",
                g["membership"] == {"f-alpha": [], "f-beta": []},
                f"got={g['membership']} win={g['wins'][0]['rect']}",
            )

            # ===== scenario 3: an overlapping fence move is REFUSED ==========
            settled = stored(desk_file)
            before_text = desk_file.read_text(encoding="utf-8")
            before_block = fence_block(before_text, "f-beta")
            check(
                "fence beta is on disk to compare against",
                before_block is not None and fence_of(settled, "f-beta")["rect"]["left"] == FENCE_B["left"],
                f"block={before_block}",
            )

            grab_b = client_point(page, ".fence-grab", 1)
            check(
                "the fence carries a move handle with a real box on screen",
                grab_b is not None and grab_b["w"] > 4 and grab_b["h"] > 4,
                f"got={grab_b}",
            )
            # Manual, so the class can be read MID-gesture: -320 puts beta at
            # left 280, squarely over alpha (40..440).
            page.mouse.move(grab_b["x"], grab_b["y"])
            page.mouse.down()
            page.mouse.move(grab_b["x"] - 160, grab_b["y"], steps=5)
            page.mouse.move(grab_b["x"] - 320, grab_b["y"], steps=5)
            page.wait_for_timeout(200)
            mid = geometry(page)
            check(
                "a fence dragged onto its neighbour shows the refusal MID-gesture",
                mid["fences"][1]["invalid"] is True,
                f"fences={[(f['id'], f['invalid'], f['rect']['left']) for f in mid['fences']]}",
            )
            check(
                "…and the neighbour it would cover is NOT dragged along or marked",
                mid["fences"][0]["invalid"] is False and mid["fences"][0]["rect"] == FENCE_A,
                f"alpha={mid['fences'][0]}",
            )
            page.mouse.up()
            page.wait_for_timeout(600)
            after = geometry(page)
            check(
                "…on release the fence REVERTS to where it started — no snapping",
                after["fences"][1]["rect"] == FENCE_B and after["fences"][1]["invalid"] is False,
                f"got={after['fences'][1]}",
            )
            quiet(desk_file)
            after_block = fence_block(desk_file.read_text(encoding="utf-8"), "f-beta")
            check(
                "…and its record in desk.toml is byte-identical — a refusal persists NOTHING",
                after_block == before_block,
                f"before={before_block!r} after={after_block!r}",
            )

            # ===== scenario 4: a fence moves its members with it =============
            # Put the window back inside alpha first.
            g = geometry(page)
            here = centre(g["wins"][0]["rect"])
            bar = client_point(page, ".session-window .session-titlebar")
            drag(page, bar, target[0] - here[0], target[1] - here[1])
            g = geometry(page)
            check(
                "the window is a member of alpha again, ready to be carried",
                g["membership"]["f-alpha"] == [win_id],
                f"got={g['membership']}",
            )
            member_before = stored(desk_file)
            wr0 = win_rect(member_before)

            grab_a = client_point(page, ".fence-grab", 0)
            drag(page, grab_a, MOVE["dx"], MOVE["dy"])
            g = geometry(page)
            check(
                f"the fence itself moved by exactly {MOVE['dx']},{MOVE['dy']}",
                g["fences"][0]["rect"]["left"] == FENCE_A["left"] + MOVE["dx"]
                and g["fences"][0]["rect"]["top"] == FENCE_A["top"] + MOVE["dy"],
                f"got={g['fences'][0]['rect']}",
            )
            check(
                "…and its member travelled with it, so it is still a member",
                g["membership"]["f-alpha"] == [win_id],
                f"got={g['membership']}",
            )
            moved_desk = stored(desk_file)
            wr1 = win_rect(moved_desk)
            check(
                "the member's PERSISTED rect moved by exactly the fence's delta",
                wr1["left"] == wr0["left"] + MOVE["dx"] and wr1["top"] == wr0["top"] + MOVE["dy"],
                f"before={wr0} after={wr1}",
            )
            check(
                "…preserving its relative position, and its size untouched",
                wr1["width"] == wr0["width"] and wr1["height"] == wr0["height"],
                f"before={wr0} after={wr1}",
            )
            check(
                "…and the fence's own new rect is what the daemon stores",
                fence_of(moved_desk, "f-alpha")["rect"]["left"] == FENCE_A["left"] + MOVE["dx"]
                and fence_of(moved_desk, "f-alpha")["rect"]["top"] == FENCE_A["top"] + MOVE["dy"],
                f"got={fence_of(moved_desk, 'f-alpha')['rect']}",
            )
            check(
                "…while the fence it did NOT touch is unmoved",
                fence_of(moved_desk, "f-beta")["rect"]["left"] == FENCE_B["left"],
                f"got={fence_of(moved_desk, 'f-beta')['rect']}",
            )

            # ===== scenario 4b: the clamp at the plane's pinned origin =======
            # `fenceMoveDelta` folds the MEMBER rects into the clamp, not just
            # the fence's. Park the member so its left/top are SMALLER than the
            # fence's: a clamp computed on the fence alone then parks it at a
            # negative coordinate, off the plane whose origin is pinned (#336).
            fa = geometry(page)["fences"][0]["rect"]
            # The member's TOP must sit above the fence's, so the vertical clamp
            # can only come from the member. Its LEFT stays to the right of the
            # fence's, which keeps the window's box clear of the `.fence-grab`
            # handle in the fence's NW corner — a covered handle is pressed as
            # the WINDOW and the gesture never starts.
            here = centre(geometry(page)["wins"][0]["rect"])
            bar = client_point(page, ".session-window .session-titlebar")
            drag(page, bar, fa["left"] + 240 - here[0], fa["top"] + 80 - here[1])
            g = geometry(page)
            wr_pre = g["wins"][0]["rect"]
            check(
                "the member is parked ABOVE the fence's own top edge",
                g["membership"]["f-alpha"] == [win_id]
                and wr_pre["top"] < fa["top"]
                and wr_pre["left"] > fa["left"],
                f"member={wr_pre} fence={fa}",
            )
            # NEGATIVE CONTROL for `fenceMoveDelta`'s member leg: clamping on the
            # FENCE alone allows dy = -fence.top, which parks this member at a
            # negative `top`. The clamp must answer to the member instead.
            want_dx = -min(fa["left"], wr_pre["left"])
            want_dy = -min(fa["top"], wr_pre["top"])
            check(
                "the fixture really is the clamp's negative control",
                want_dy != -fa["top"] and wr_pre["top"] > 0,
                f"fence.top={fa['top']} member.top={wr_pre['top']} dy={want_dy}",
            )
            grab_a = client_point(page, ".fence-grab", 0)
            drag(page, grab_a, -400, -400)  # far more than the plane allows
            g = geometry(page)
            check(
                "a move toward the origin is CLAMPED, not refused — the fence still moves",
                g["fences"][0]["rect"]["left"] == fa["left"] + want_dx
                and g["fences"][0]["rect"]["top"] == fa["top"] + want_dy,
                f"fence={g['fences'][0]['rect']} want={(fa['left'] + want_dx, fa['top'] + want_dy)}",
            )
            clamped = stored(
                desk_file,
                lambda d: win_rect(d) and win_rect(d)["top"] == 0.0,
            )
            wr_clamp = win_rect(clamped)
            check(
                "…and the member lands EXACTLY on the origin, never past it",
                wr_clamp["top"] == 0.0 and wr_clamp["left"] == wr_pre["left"] + want_dx,
                f"got={wr_clamp} want top=0 left={wr_pre['left'] + want_dx}",
            )
            check(
                "…with no coordinate anywhere on the plane gone negative",
                min(
                    wr_clamp["left"],
                    wr_clamp["top"],
                    fence_of(clamped, "f-alpha")["rect"]["left"],
                    fence_of(clamped, "f-alpha")["rect"]["top"],
                )
                >= 0,
                f"member={wr_clamp} fence={fence_of(clamped, 'f-alpha')['rect']}",
            )
            check(
                "…and the member is still a member: the clamp moved BOTH by one delta",
                geometry(page)["membership"]["f-alpha"] == [win_id],
                f"got={geometry(page)['membership']}",
            )

            # ===== scenario 5: a resize resizes no member ====================
            # Park the window deep in alpha, so shrinking the fence to its
            # minimum leaves the window's centre plainly OUTSIDE — not on a
            # border, where a one-pixel measurement would decide the check.
            # The window is 300x200 and draws ABOVE the fence (`z >= 60`), so its
            # box must stay CLEAR of the 14 px grip in the fence's SE corner —
            # a `page.mouse` press on a covered grip correctly hits the WINDOW
            # and the resize never starts (#340's probe-ambiguity trap).
            fa = geometry(page)["fences"][0]["rect"]
            corner = (fa["left"] + 230, fa["top"] + 180)
            here = centre(geometry(page)["wins"][0]["rect"])
            bar = client_point(page, ".session-window .session-titlebar")
            drag(page, bar, corner[0] - here[0], corner[1] - here[1])
            g = geometry(page)
            check(
                "the window sits in the fence's far corner and is still a member",
                g["membership"]["f-alpha"] == [win_id],
                f"got={g['membership']} win={g['wins'][0]['rect']}",
            )
            size_before = stored(desk_file)
            wr2 = win_rect(size_before)

            grip = client_point(page, ".fence-grip", 0)
            check(
                "the fence carries a resize grip with a real box on screen",
                grip is not None and grip["w"] > 4 and grip["h"] > 4,
                f"got={grip}",
            )
            drag(page, grip, -260, -260)
            page.wait_for_timeout(400)
            g = geometry(page)
            shrunk = g["fences"][0]["rect"]
            check(
                "the grip shrinks the fence, down to its 240x150 minimum",
                shrunk["width"] == 240 and shrunk["height"] == 150,
                f"got={shrunk}",
            )
            check(
                "…moving neither of its edges that the SE grip does not own",
                shrunk["left"] == fa["left"] and shrunk["top"] == fa["top"],
                f"before={fa} after={shrunk}",
            )
            check(
                "a member whose centre now falls OUTSIDE leaves the fence",
                g["membership"] == {"f-alpha": [], "f-beta": []},
                f"got={g['membership']} win={g['wins'][0]['rect']} fence={shrunk}",
            )
            after_size = stored(desk_file)
            wr3 = win_rect(after_size)
            check(
                "…and the resize resized NO member: its persisted size is unchanged",
                wr3["width"] == wr2["width"] and wr3["height"] == wr2["height"],
                f"before={wr2} after={wr3}",
            )
            check(
                "…nor did it move one",
                wr3["left"] == wr2["left"] and wr3["top"] == wr2["top"],
                f"before={wr2} after={wr3}",
            )
            check(
                "…and the shrunken fence is what the daemon stores",
                fence_of(after_size, "f-alpha")["rect"]["width"] == 240,
                f"got={fence_of(after_size, 'f-alpha')['rect']}",
            )

            # ===== scenario 5b: a resize INTO a neighbour is refused =========
            # The refusal branch of `startFenceResize` — the shrink above never
            # reaches it, so without this leg the `fenceFits` check could be
            # deleted from the resize and every gate would stay green.
            # Park the member well clear first: the fence just shrank, so its
            # grip has moved INTO the window's box — and a covered grip is
            # pressed as the window, silently skipping the whole gesture.
            drag(page, client_point(page, ".session-window .session-titlebar"), 0, 400)
            small = geometry(page)["fences"][0]["rect"]
            grip = client_point(page, ".fence-grip", 0)
            page.mouse.move(grip["x"], grip["y"])
            page.mouse.down()
            page.mouse.move(grip["x"] + 250, grip["y"], steps=5)
            page.mouse.move(grip["x"] + 500, grip["y"], steps=5)
            page.wait_for_timeout(200)
            mid = geometry(page)
            check(
                "a fence GROWN into its neighbour shows the refusal mid-gesture",
                mid["fences"][0]["invalid"] is True
                and mid["fences"][0]["rect"]["width"] > small["width"],
                f"alpha={mid['fences'][0]} beta={mid['fences'][1]['rect']}",
            )
            page.mouse.up()
            page.wait_for_timeout(600)
            back = geometry(page)["fences"][0]["rect"]
            check(
                "…and on release it reverts to the rect it started the resize at",
                back == small,
                f"before={small} after={back}",
            )
            quiet(desk_file)
            check(
                "…having persisted nothing: the store still holds the shrunken rect",
                fence_of(json.loads(http("GET", "api/desk")[1]), "f-alpha")["rect"]["width"]
                == small["width"],
                f"got={fence_of(json.loads(http('GET', 'api/desk')[1]), 'f-alpha')['rect']}",
            )

            # ===== scenario 5c: EVERY edge is a handle ======================
            # ADR-0051 §7a: the borders resize, not just the SE grip. The WEST
            # edge is the discriminating one — it moves `left` with the east edge
            # ANCHORED, which is the leg a `se`-only fold cannot fake, and it is
            # still a resize: it carries no member, exactly like the shrink above.
            page.evaluate("() => { document.getElementById('workspace').scrollLeft = 0; }")
            page.wait_for_timeout(200)
            # Alpine is parked at its 240x150 MINIMUM by scenario 5, and beta
            # sits at left 600: a west drag rightwards would only meet the width
            # floor, and a leftwards one the pinned origin. Move it clear of both
            # first — down and right — so the edge has somewhere to go. The move
            # carries the member (§6), which is why `wr_before` is read after it.
            drag(page, client_point(page, ".fence-grab", 0), 400, 400)
            page.wait_for_timeout(300)
            edge_before = geometry(page)["fences"][0]["rect"]
            wr_before = win_rect(stored(desk_file))
            west = client_point(page, ".fence-edge[data-dir='w']", 0)
            check(
                "the fence carries a WEST edge handle with a real box on screen",
                west is not None and west["w"] > 2 and west["h"] > 4,
                f"got={west}",
            )
            drag(page, west, -60, 0)
            page.wait_for_timeout(400)
            edge_after = geometry(page)["fences"][0]["rect"]
            check(
                "dragging the west edge moves the fence's LEFT by the pointer delta",
                edge_after["left"] == edge_before["left"] - 60,
                f"before={edge_before} after={edge_after}",
            )
            check(
                "…with the EAST edge anchored: the fence narrows, it does not slide",
                edge_after["left"] + edge_after["width"]
                == edge_before["left"] + edge_before["width"],
                f"before={edge_before} after={edge_after}",
            )
            check(
                "…and no member moved with it — a west drag is a resize, not a §6 move",
                win_rect(stored(desk_file)) == wr_before,
                f"before={wr_before} after={win_rect(stored(desk_file))}",
            )
            check(
                "…and the daemon stores the edge-resized rect",
                fence_of(stored(desk_file), "f-alpha")["rect"]["left"] == edge_after["left"],
                f"got={fence_of(stored(desk_file), 'f-alpha')['rect']}",
            )

            page.screenshot(path=SHOT)
            check("the evidence screenshot is on disk", os.path.exists(SHOT), SHOT)
            ctx.close()
            time.sleep(1.2)

            # ===== scenario 6: CREATING into an overlap is refused too =======
            # One fence blanketing the whole spawn grid, so every slot
            # `nextFenceSlot` can offer is taken. Drop the guard from
            # `createFence` and this fence count goes to 2 — the criterion says
            # creating an overlap is refused, and nothing else exercises it.
            http(
                "PUT",
                "api/desk",
                {
                    "windows": [],
                    "fences": [
                        {
                            "id": "f-blanket",
                            "name": "blanket",
                            "rect": {"left": 0.0, "top": 0.0, "width": 4000.0, "height": 4000.0},
                            "ts": 200,
                        }
                    ],
                },
            )
            full_ctx = browser.new_context(viewport=dict(VIEW))
            full = desk_page(full_ctx)
            full.wait_for_function(
                "() => document.querySelectorAll('.fence').length === 1", timeout=15000
            )
            unscroll(full)
            draw_fence(full)
            full.wait_for_timeout(300)
            flashed = full.evaluate(
                "() => [...document.querySelectorAll('.fence')]"
                "  .some((f) => f.classList.contains('fence-invalid'))"
            )
            drawn = full.evaluate("() => document.querySelectorAll('.fence').length")
            check(
                "a fence that could only be born overlapping is REFUSED, not nudged into a gap",
                drawn == 1,
                f"fence count={drawn}",
            )
            check(
                "…with the offending fence flashed, so the refusal is visible",
                flashed,
                "no .fence carried fence-invalid after the refused create",
            )
            full.wait_for_timeout(900)
            check(
                "…and the flash clears itself, leaving no stuck red border",
                not full.evaluate(
                    "() => [...document.querySelectorAll('.fence')]"
                    "  .some((f) => f.classList.contains('fence-invalid'))"
                ),
            )
            quiet(desk_file)
            check(
                "…and nothing reached the store: still exactly one fence",
                len(json.loads(http("GET", "api/desk")[1])["fences"]) == 1,
                f"got={json.loads(http('GET', 'api/desk')[1])['fences']}",
            )
            full_ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The floor is the REAL count, not a loose lower bound: set under the total,
    # a whole scenario could stop running while the suite still exits 0.
    ok = all(results) and len(results) == 47
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("A FENCE IS A GROUP")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
