"""#342 browser acceptance: ARRANGE MOVES INTO THE FENCE.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7437, so this can run
beside #340's and #341's suites without any daemon stealing another's port.

The fixture `desk.toml` is written BEFORE the daemon starts: two DISJOINT
fences and one placeholder window inside neither. The three MEMBERS are real
plain consoles (`WBConsole.open({ repo, plain: true })`) dragged into fence
alpha — real terminals, because the refit criterion has nothing to measure on a
placeholder. Every member is dropped clear of the fence's own head band AND of
its 14 px SE grip: both sit at `z-index: 1`, BELOW every window, so a member
parked on one makes the fence's controls unhittable (#341's covered-handle
trap).

Scenario 1  the fence's chrome reads its name, its window COUNT and the REPOS
            its members belong to — deduped and sorted; an empty fence reads
            `0 consoles` and no repos
Scenario 2  the global Arrange control is gone, and so is `WBConsole.arrange`
Scenario 3  a REAL click on the fence's own arrange button tiles exactly its
            members, entirely inside its rect; the window outside is untouched
Scenario 4  the fence's chrome survives its own arrange — the arrange button
            and the SE grip are both still the top element at their own centres
Scenario 7  the tiled terminals are REFITTED: `proposeDimensions()` agrees with
            the live `cols`/`rows`, and at least one console's cols changed
Scenario 6  the tiled rects PERSIST: the daemon serves them and a reload
            reproduces the tiled layout box for box
Scenario 5  arranging an EMPTY fence is a no-op: no rect moves, nothing is
            written, and no page error is raised
Scenario 3b the grid is in STAGE coordinates: the same rects come back at a
            non-zero scroll (re-homed from `wb_stage_336.py` scenario 6)
Scenario 8  a MAXIMIZED member is left alone (#338's rule, re-homed here) while
            the rest re-tile into the smaller grid — this is where the evidence
            screenshot is taken
Scenario 9  a cell BELOW `.session-window`'s CSS floor still renders inside the
            fence: CSS outranks an inline width, and every other fixture here
            sits above the floor

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/342-arrange-into-the-fence-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_fence_342.py   (exit 0 = all pass)
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

PORT = 7437
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "342-arrange-into-the-fence-2026-07-27.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

VIEW = {"width": 1400, "height": 900}

# The fixture geometry. Alpha is roomy enough for a 2x2 grid of real consoles;
# beta is disjoint from it (640 < 680) and stays EMPTY, which is the no-op leg.
FENCE_A = {"left": 40, "top": 40, "width": 600, "height": 460}
FENCE_B = {"left": 680, "top": 40, "width": 320, "height": 300}
# Centre (190, 680): inside neither fence, so an arrange must not touch it.
OUTSIDE = {"left": 60, "top": 600, "width": 260, "height": 160}
# Three member drop targets, as CENTRES in stage coordinates. All three lie in
# alpha; all three boxes (260x160) clear the head band at the top and the SE
# grip at (626..640, 486..500).
MEMBER_BOX = {"width": 260, "height": 160}
MEMBERS = [(190, 180), (470, 180), (190, 370)]

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
    empty = tempfile.mkdtemp(prefix="wb342_empty_")
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


def make_fixture_repo(tag):
    d = tempfile.mkdtemp(prefix=f"wb342_{tag}_")
    p = Path(d)
    (p / "README.md").write_text(f"# fixture {tag}\n\nThe #342 arrange fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb342@example.com"],
        ["git", "config", "user.name", "wb342"],
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
    """Two disjoint fences and ONE window inside neither. `kind = "agent"`
    restores as a PLACEHOLDER: full chrome, deterministic geometry, no PTY."""
    Path(daemon_dir, "desk.toml").write_text(
        "[[windows]]\n"
        'id = "w-outside"\n'
        f'repo = "{slug}"\n'
        'agent = "claude"\n'
        'kind = "agent"\n'
        "max = false\n"
        "ts = 100\n"
        f"{rect_toml(OUTSIDE)}\n"
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
        timeout=25000,
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
    workspace's own origin."""
    page.evaluate(
        "() => { const ws = document.getElementById('workspace');"
        " ws.scrollLeft = 0; ws.scrollTop = 0; }"
    )
    page.wait_for_timeout(200)


def quiet(desk_file, still=1.6, timeout=15):
    """Block until `desk.toml` has not changed for `still` seconds.

    A fixed sleep is the wrong synchroniser for "nothing was written": under
    load the shell's 250 ms flush can land AFTER the sleep (#341).
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


def stored(desk_file, want=None, timeout=15):
    """The desk as the DAEMON holds it, once the flush has landed."""
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


def boxes(page):
    """Every window's id, inline rect, measured box, maximized flag and terminal
    dimensions, in ONE evaluate. The INLINE rect is what `tileIntoRect` wrote;
    the measured box is what the operator sees — both are asserted, because a
    tile rect written onto a `.maximized` window is invisible in the second."""
    return page.evaluate(
        "() => [...document.querySelectorAll('.session-window')].map((w) => ({"
        "  id: w._deskId,"
        "  inline: { left: parseFloat(w.style.left), top: parseFloat(w.style.top),"
        "    width: parseFloat(w.style.width), height: parseFloat(w.style.height) },"
        "  box: { left: w.offsetLeft, top: w.offsetTop,"
        "    width: w.offsetWidth, height: w.offsetHeight },"
        "  max: w.classList.contains('maximized'),"
        "  cols: w._term ? w._term.term.cols : null,"
        "  rows: w._term ? w._term.term.rows : null }))"
    )


def by_id(rows):
    return {r["id"]: r for r in rows}


def fence_chrome(page):
    return page.evaluate(
        "() => { const out = {};"
        " for (const f of document.querySelectorAll('.fence')) {"
        "   out[f.dataset.fenceId] = {"
        "     name: f.querySelector('.fence-name').value,"
        "     count: f.querySelector('.fence-count').textContent.trim(),"
        "     repos: f.querySelector('.fence-repos').textContent.trim() }; }"
        " return out; }"
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


def open_member(page, slug, index, centre):
    """Open a real plain console and DRAG it to `centre` (stage coordinates).

    A drag, not an inline style write: membership and the fence chrome are
    re-derived on the persist that a real drop performs, so a scripted rect
    would leave the readouts asserting stale state.
    """
    before = page.locator(".session-window").count()
    page.evaluate(f"() => window.WBConsole.open({{ repo: '{slug}', plain: true }})")
    page.wait_for_function(
        f"() => document.querySelectorAll('.session-window').length === {before + 1}", timeout=10000
    )
    win = page.locator(".session-window").nth(before)
    win.locator(".xterm").wait_for(timeout=20000)
    page.wait_for_timeout(400)
    # Size it first, so every member's box is the one this file names.
    page.evaluate(
        "([i, w, h]) => { const el = document.querySelectorAll('.session-window')[i];"
        " el.style.width = w + 'px'; el.style.height = h + 'px'; }",
        [before, MEMBER_BOX["width"], MEMBER_BOX["height"]],
    )
    page.wait_for_timeout(200)
    here = page.evaluate(
        "(i) => { const el = document.querySelectorAll('.session-window')[i];"
        " return { x: el.offsetLeft + el.offsetWidth / 2, y: el.offsetTop + el.offsetHeight / 2 }; }",
        before,
    )
    bar = page.evaluate(
        "(i) => { const el = document.querySelectorAll('.session-window')[i]"
        "   .querySelector('.session-titlebar');"
        " const r = el.getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
        before,
    )
    drag(page, bar, centre[0] - here["x"], centre[1] - here["y"])
    return page.evaluate(
        "(i) => document.querySelectorAll('.session-window')[i]._deskId", before
    )


def inside_fence(r, fence=FENCE_A, slack=0.0):
    return (
        r["left"] >= fence["left"] - slack
        and r["top"] >= fence["top"] - slack
        and r["left"] + r["width"] <= fence["left"] + fence["width"] + slack
        and r["top"] + r["height"] <= fence["top"] + fence["height"] + slack
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb342_reg_")
    desk_file = Path(daemon_dir, "desk.toml")
    slug_a = register_fixture(daemon_dir, make_fixture_repo("one"))
    slug_b = register_fixture(daemon_dir, make_fixture_repo("two"))
    write_fixture_desk(daemon_dir, slug_a)
    want_repos = " · ".join(sorted([slug_a, slug_b]))

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport=dict(VIEW))
            page = ctx.new_page()
            errors = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.set_viewport_size(dict(VIEW))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
            page.wait_for_timeout(1800)
            settle_windows(page, 1)
            page.wait_for_function(
                "() => document.querySelectorAll('.fence').length === 2", timeout=15000
            )
            unscroll(page)

            member_ids = [
                open_member(page, slug_a, 0, MEMBERS[0]),
                open_member(page, slug_a, 1, MEMBERS[1]),
                open_member(page, slug_b, 2, MEMBERS[2]),
            ]
            page.wait_for_timeout(600)
            start = by_id(boxes(page))
            check(
                "three real consoles are dropped inside fence alpha, clear of both handles",
                all(
                    start[i]["box"]["left"] >= FENCE_A["left"]
                    and start[i]["box"]["top"] >= 100
                    and start[i]["box"]["left"] + start[i]["box"]["width"] <= 626
                    for i in member_ids
                ),
                f"boxes={[start[i]['box'] for i in member_ids]}",
            )

            # ===== scenario 1: the fence's own chrome =========================
            chrome = fence_chrome(page)
            check(
                "the fence reads its name and its window COUNT",
                chrome["f-alpha"]["name"] == "alpha" and chrome["f-alpha"]["count"] == "3 consoles",
                f"got={chrome['f-alpha']}",
            )
            check(
                "…and the REPOS its members belong to, deduped and sorted",
                chrome["f-alpha"]["repos"] == want_repos,
                f"got={chrome['f-alpha']['repos']!r} want={want_repos!r}",
            )
            check(
                "an EMPTY fence reads zero consoles and no repos — the window outside counts for neither",
                chrome["f-beta"]["count"] == "0 consoles" and chrome["f-beta"]["repos"] == "",
                f"got={chrome['f-beta']}",
            )

            # ===== scenario 2: the global control is retired ==================
            arrange_buttons = page.locator("button:has-text('Arrange')").count()
            arrange_global = page.evaluate("() => typeof window.WBConsole.arrange")
            fence_buttons = page.evaluate("() => document.querySelectorAll('.fence-arrange').length")
            check(
                "no Arrange control survives anywhere in the shell",
                arrange_buttons == 0,
                f"count={arrange_buttons}",
            )
            check(
                "…and the global entry point is gone from the module too",
                arrange_global == "undefined",
                f"typeof={arrange_global}",
            )
            check(
                "…while each fence carries its OWN arrange button",
                fence_buttons == 2,
                f"count={fence_buttons}",
            )

            # ===== scenario 3: arranging the fence tiles ITS members ==========
            outside_before = by_id(boxes(page))["w-outside"]
            page.locator(".fence[data-fence-id='f-alpha'] .fence-arrange").click()
            page.wait_for_timeout(900)  # past the 0.24s tiling transition
            tiled = by_id(boxes(page))
            check(
                "every member's INLINE rect — what tileIntoRect wrote — lies inside the fence",
                all(inside_fence(tiled[i]["inline"]) for i in member_ids),
                f"inline={[tiled[i]['inline'] for i in member_ids]} fence={FENCE_A}",
            )
            check(
                "…and so does every member's MEASURED box (1 px for the DOM's own rounding)",
                all(inside_fence(tiled[i]["box"], slack=1.0) for i in member_ids),
                f"boxes={[tiled[i]['box'] for i in member_ids]} fence={FENCE_A}",
            )
            check(
                "…the members really MOVED — an arrange that did nothing would pass containment vacuously",
                all(
                    tiled[i]["box"] != start[i]["box"] for i in member_ids
                ),
                f"before={[start[i]['box'] for i in member_ids]} after={[tiled[i]['box'] for i in member_ids]}",
            )
            overlaps = [
                (a, b)
                for x, a in enumerate(member_ids)
                for b in member_ids[x + 1 :]
                if tiled[a]["inline"]["left"] < tiled[b]["inline"]["left"] + tiled[b]["inline"]["width"]
                and tiled[a]["inline"]["left"] + tiled[a]["inline"]["width"] > tiled[b]["inline"]["left"]
                and tiled[a]["inline"]["top"] < tiled[b]["inline"]["top"] + tiled[b]["inline"]["height"]
                and tiled[a]["inline"]["top"] + tiled[a]["inline"]["height"] > tiled[b]["inline"]["top"]
            ]
            check(
                "…and no two members OVERLAP — a grid, not three rects that merely differ",
                overlaps == [],
                f"overlapping pairs={overlaps} rects={[tiled[i]['inline'] for i in member_ids]}",
            )
            check(
                "the window OUTSIDE the fence is byte-identical — arrange tiles members, not the plane",
                tiled["w-outside"]["box"] == outside_before["box"],
                f"before={outside_before['box']} after={tiled['w-outside']['box']}",
            )
            check(
                "…and the fence's count survives its own arrange",
                fence_chrome(page)["f-alpha"]["count"] == "3 consoles",
                f"got={fence_chrome(page)['f-alpha']}",
            )

            # ===== scenario 4: the fence's chrome stays hittable ==============
            # The head and the grip are at `z-index: 1`, BELOW every window: a
            # tile parked on either makes the fence usable exactly once. The
            # inset is what keeps them clear, and this is the only gate on it.
            hit = page.evaluate(
                "() => { const el = document.querySelector(\"[data-fence-id='f-alpha']\");"
                " const at = (r) => { const e = document.elementFromPoint("
                "     r.left + r.width / 2, r.top + r.height / 2);"
                # The hit is reported as the nearest NAMED owner, not as the raw
                # node: a point over a tile lands on the xterm canvas inside the
                # window, which is still "the window took this press".
                "   if (!e) return null;"
                "   const own = e.closest('.session-window, .fence-arrange, .fence-grip');"
                "   return own ? own.className.toString() : e.className.toString(); };"
                " const win = document.querySelectorAll('.session-window')[1];"
                " return { arrange: at(el.querySelector('.fence-arrange').getBoundingClientRect()),"
                "   grip: at(el.querySelector('.fence-grip').getBoundingClientRect()),"
                "   overWindow: at(win.getBoundingClientRect()) }; }"
            )
            check(
                "the fence's arrange button is still the top element at its own centre",
                hit["arrange"] is not None and "fence-arrange" in hit["arrange"],
                f"got={hit['arrange']!r}",
            )
            check(
                "…and so is its SE resize grip",
                hit["grip"] is not None and "fence-grip" in hit["grip"],
                f"got={hit['grip']!r}",
            )
            check(
                "…while a point over a TILE answers the window — the control that this probe discriminates",
                hit["overWindow"] is not None and "session-window" in hit["overWindow"],
                f"got={hit['overWindow']!r}",
            )

            # ===== scenario 7: the tiled terminals are refitted ===============
            refit = page.evaluate(
                "(ids) => ids.map((id) => { const w = [...document.querySelectorAll('.session-window')]"
                "   .find((x) => x._deskId === id);"
                " const p = w._term ? w._term.fit.proposeDimensions() : null;"
                " return { id, cols: w._term.term.cols, rows: w._term.term.rows,"
                "   proposed: p ? { cols: p.cols, rows: p.rows } : null }; })",
                member_ids,
            )
            check(
                "every tiled terminal agrees with the box it now occupies",
                all(
                    r["proposed"] is not None
                    and r["proposed"]["cols"] == r["cols"]
                    and r["proposed"]["rows"] == r["rows"]
                    for r in refit
                ),
                f"got={refit}",
            )
            check(
                "…and at least one console's cols really CHANGED — a refit ran, two stale numbers did not agree",
                any(r["cols"] != start[r["id"]]["cols"] for r in refit),
                f"before={[start[i]['cols'] for i in member_ids]} after={[r['cols'] for r in refit]}",
            )

            # ===== scenario 6: the tiled rects persist, and a reload replays ==
            def all_tiled(desk):
                got = {w["id"]: w["rect"] for w in desk.get("windows", [])}
                return all(
                    i in got and abs(got[i]["left"] - tiled[i]["inline"]["left"]) < 0.5 for i in member_ids
                )

            served = stored(desk_file, all_tiled)
            served_rects = {w["id"]: w["rect"] for w in served.get("windows", [])}
            check(
                "the daemon stores the TILED rects — tiling is a layout act, like a drag",
                all(
                    served_rects[i] == {k: float(v) for k, v in tiled[i]["box"].items()}
                    for i in member_ids
                ),
                f"served={[served_rects.get(i) for i in member_ids]}"
                f" boxes={[tiled[i]['box'] for i in member_ids]}",
            )
            check(
                "…which are the TILED rects and not the pre-arrange ones — the defect this criterion names",
                all(served_rects[i] != {k: float(v) for k, v in start[i]["box"].items()} for i in member_ids),
                f"served={[served_rects.get(i) for i in member_ids]}"
                f" pre-arrange={[start[i]['box'] for i in member_ids]}",
            )
            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
            page.wait_for_timeout(1800)
            settle_windows(page, 4)
            unscroll(page)
            back = by_id(boxes(page))
            check(
                "a reload reproduces the tiled layout, box for box",
                all(back[i]["box"] == tiled[i]["box"] for i in member_ids),
                f"before={[tiled[i]['box'] for i in member_ids]} after={[back[i]['box'] for i in member_ids]}",
            )
            check(
                "…and the restored fence re-derives its chrome from the restored members",
                fence_chrome(page)["f-alpha"]["count"] == "3 consoles"
                and fence_chrome(page)["f-alpha"]["repos"] == want_repos,
                f"got={fence_chrome(page)['f-alpha']}",
            )

            # ===== scenario 3b: the grid is in STAGE coordinates ==============
            # Re-homed from `wb_stage_336.py` scenario 6, whose premise (tiling
            # follows the viewport's scroll offset) this issue retires: tiling is
            # now fence-scoped, so the SAME rects must come back at any scroll.
            # Without this every arrange in this file runs at scroll 0, and an
            # `arrangeFence` reading client coordinates passes the whole suite.
            page.evaluate("() => { document.getElementById('workspace').scrollLeft = 200; }")
            page.wait_for_timeout(200)
            page.locator(".fence[data-fence-id='f-alpha'] .fence-arrange").click()
            page.wait_for_timeout(900)
            scrolled = by_id(boxes(page))
            check(
                "arranging at scrollLeft 200 lands on the SAME stage rects — the fence is the target, not the frame",
                all(scrolled[i]["inline"] == tiled[i]["inline"] for i in member_ids),
                f"unscrolled={[tiled[i]['inline'] for i in member_ids]}"
                f" scrolled={[scrolled[i]['inline'] for i in member_ids]}",
            )
            unscroll(page)

            # ===== scenario 5: an empty fence is a no-op ======================
            before_empty = by_id(boxes(page))
            errors_before_empty = len(errors)
            page.locator(".fence[data-fence-id='f-beta'] .fence-arrange").click()
            page.wait_for_timeout(900)
            quiet(desk_file)
            after_empty = by_id(boxes(page))
            check(
                "arranging an EMPTY fence moves nothing on the stage",
                after_empty == before_empty,
                f"before={[before_empty[k]['box'] for k in sorted(before_empty)]}"
                f" after={[after_empty[k]['box'] for k in sorted(after_empty)]}",
            )
            check(
                "…and raises no error — a no-op, not a failure",
                len(errors) == errors_before_empty,
                f"pageerrors={errors[errors_before_empty:]}",
            )

            # ===== scenario 8: a maximized member is left alone (#338) ========
            # Re-homed from `wb_frame_338.py`: the rule survives the retirement,
            # but its only driver is now per-fence and #338's fixture has no
            # fence to arrange.
            keep = member_ids[0]
            others = member_ids[1:]
            page.evaluate(
                "(id) => [...document.querySelectorAll('.session-window')]"
                "  .find((w) => w._deskId === id).querySelector('.session-max').click()",
                keep,
            )
            page.wait_for_timeout(700)
            maxed = by_id(boxes(page))
            check(
                "one member is maximized, and its INLINE rect still holds its pre-maximize box",
                maxed[keep]["max"] and maxed[keep]["inline"] == before_empty[keep]["inline"],
                f"got max={maxed[keep]['max']} inline={maxed[keep]['inline']}",
            )
            # Driven through the module: the full bleed covers the in-fence
            # button, and the CLICK path is already proven by scenario 3.
            page.evaluate("() => window.WBConsole.arrangeFence('f-alpha')")
            page.wait_for_timeout(900)
            regrid = by_id(boxes(page))
            check(
                "a maximized member is NOT tiled: its stored rect is untouched",
                regrid[keep]["inline"] == maxed[keep]["inline"],
                f"before={maxed[keep]['inline']} after={regrid[keep]['inline']}",
            )
            check(
                "…while the other two re-tile into the SMALLER grid, taller than the 3-member one",
                all(
                    regrid[i]["inline"]["height"] > tiled[i]["inline"]["height"] + 1 for i in others
                ),
                f"three={[tiled[i]['inline']['height'] for i in others]}"
                f" two={[regrid[i]['inline']['height'] for i in others]}",
            )
            check(
                "…still entirely inside the fence",
                all(inside_fence(regrid[i]["inline"]) for i in others),
                f"got={[regrid[i]['inline'] for i in others]}",
            )
            page.evaluate(
                "(id) => [...document.querySelectorAll('.session-window')]"
                "  .find((w) => w._deskId === id).querySelector('.session-max').click()",
                keep,
            )
            page.wait_for_timeout(700)
            restored = by_id(boxes(page))
            check(
                "…and it restores to exactly the rect the arrange refused to overwrite",
                not restored[keep]["max"]
                and restored[keep]["box"]["width"] == maxed[keep]["inline"]["width"],
                f"got={restored[keep]['box']} want width={maxed[keep]['inline']['width']}",
            )

            page.screenshot(path=SHOT)
            check("the evidence screenshot is on disk", os.path.exists(SHOT), SHOT)

            # ===== scenario 9: a cell BELOW the CSS window floor =============
            # `.session-window` carries `min-width: 240px; min-height: 150px`,
            # and CSS outranks the inline width a tile writes — so a fence small
            # enough to tile a 176x116 cell renders it 240x150 and the member
            # escapes by 52 px. Every other scenario here uses a fixture whose
            # cells sit ABOVE the floor, which is exactly why none of them can
            # see this. Shrunk to 200x180, only the first member's centre stays
            # inside, so this is the one-member grid: cell = the whole inset.
            small = {"left": 40, "top": 40, "width": 200, "height": 180}
            page.evaluate(
                "(r) => { const el = document.querySelector(\"[data-fence-id='f-alpha']\");"
                " el.style.width = r.width + 'px'; el.style.height = r.height + 'px'; }",
                small,
            )
            page.wait_for_timeout(200)
            page.locator(".fence[data-fence-id='f-alpha'] .fence-arrange").click()
            page.wait_for_timeout(900)
            tight = by_id(boxes(page))
            inside_small = [i for i in member_ids if inside_fence(tight[i]["box"], small, slack=1.0)]
            check(
                "a tile smaller than the CSS window floor still renders INSIDE the fence",
                all(
                    tight[i]["box"]["width"] < 240 and tight[i]["box"]["height"] < 150
                    for i in inside_small
                )
                and inside_small,
                f"tiled boxes={[tight[i]['box'] for i in member_ids]} fence={small}",
            )
            check(
                "…and the member that no longer belongs to the shrunken fence was not tiled into it",
                len(inside_small) == 1,
                f"inside={inside_small} boxes={[tight[i]['box'] for i in member_ids]}",
            )

            check("no page error was raised by the whole pass", errors == [], f"pageerrors={errors}")
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The floor is the REAL count, not a loose lower bound: set under the total,
    # a whole scenario could stop running while the suite still exits 0.
    ok = all(results) and len(results) == 35
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("ARRANGE MOVES INTO THE FENCE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
