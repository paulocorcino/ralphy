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


def stored(desk_file, timeout=8):
    """The desk as the DAEMON holds it, once the 250 ms flush has landed."""
    time.sleep(0.9)
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        try:
            last = json.loads(http("GET", "api/desk")[1])
            return last
        except Exception:
            time.sleep(0.3)
    return last or {"windows": [], "fences": []}


def fence_of(desk, fid):
    for f in desk.get("fences", []):
        if f["id"] == fid:
            return f
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
            in_desk = stored(desk_file)
            text = desk_file.read_text(encoding="utf-8")
            window_block = text.split("[[fences]]")[0]
            check(
                "…and NOTHING about the fence was written into the window record",
                "fence" not in window_block.lower() and "fence" not in json.dumps(in_desk["windows"]).lower(),
                f"window record={json.dumps(in_desk['windows'])[:220]}",
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
            time.sleep(1.2)
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

            page.screenshot(path=SHOT)
            check("the evidence screenshot is on disk", os.path.exists(SHOT), SHOT)

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The floor is the REAL count, not a loose lower bound: set under the total,
    # a whole scenario could stop running while the suite still exits 0.
    ok = all(results) and len(results) >= 24
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("A FENCE IS A GROUP")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
