"""#347 browser acceptance: THE DETACH SURVIVES AN F5, AND DIES WITH THE TAB.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7440, so this can run
beside #340-#346's suites without any daemon stealing another's port.

The fixture `desk.toml` is written BEFORE the daemon starts: `f-alpha` holds all
three window records (two `console` records — real PTYs, so "the consoles stay
live in the popup" is not vacuous — and one `agent` placeholder), and a second,
non-overlapping fence `f-beta` holds nothing. Alpha holding EVERY member is what
makes the no-flash oracle sharp: with the fence detached, a correct plane
carries zero `.session-window` for the whole post-reload page life, so the peak
counter's expected value is exactly 0 rather than "some number".

Scenario 1  the F5: reload the origin tab — the popup stays open with its live
            consoles, the fence renders detached from the first paint, the
            registry is still in this tab's session store, and NOT ONE console
            was ever inserted on the plane (the peak counter, sampled from
            document start). The screenshot is taken here.
Scenario 2  the popup's consoles are still DRIVEABLE after the origin reloaded
Scenario 3  a second tab of the same browser sees no detach: three consoles
            inside the fence, no glyph, and no registry of its own
Scenario 4  re-attach by CLOSING the popup, after the reload — every console
            back at its original box within 2000 ms
Scenario 5  re-attach by the GLYPH, after a reload and an abrupt popup close
            (no unload): home within 2000 ms — well inside the 6 s peer expiry,
            which is what tells the two paths apart
Scenario 5b a MOVED detached fence still keeps its members off the plane across
            a reload. The registry carries the member IDS for exactly this: a
            detached fence is still movable (§7a), and a membership fold run
            after the reload compares its NEW rect with the members' ORIGINAL
            records — answering "no members", and putting every one of them back
            under the live popup. Found by the self-review, not by this suite.
Scenario 5c a SILENT origin is not a dead one: with the origin's periodic post
            dropped — what a hidden tab's throttled timer does — the popup stays
            open past two peer windows, because a probe still gets an answer
Scenario 6  force-killing the origin tab (no unload fires): the popup tells the
            operator its peer is gone, then closes itself
Scenario 7  closing the origin tab cleanly closes its popups too
Scenario 8  NO DAEMON CHANGE: `GET /api/desk` is unchanged across the detach and
            the reload, and no popup context ever issues a `PUT`

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_fence_347.py   (exit 0 = all pass)
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

PORT = 7440
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
# The deterministic echo child (#334): every console becomes a
# `session_test_child`, whose `GOT:<line>` reply is a machine-readable oracle for
# "this keystroke reached the PTY".
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "347-the-detach-survives-an-f5-2026-07-28.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

VIEW = {"width": 1400, "height": 900}

# The registry's key, and the heartbeat window the popup applies to its peer.
DETACH_KEY = "wb.detach.v1"
PEER_WINDOW_MS = 6000

# The fixture geometry, in stage coordinates. The two fences do not overlap —
# an overlapping pair would make `fenceMembership`'s tie-break, not the detach,
# decide which fence a member belongs to.
FENCE_A = {"left": 40, "top": 40, "width": 600, "height": 460}
FENCE_B = {"left": 800, "top": 40, "width": 320, "height": 300}

# Three members, ALL in alpha. Each starts below the head band (top >= 100) and
# stops well short of the SE grip at (626..640, 486..500) — a member parked on a
# fence handle makes the fence ungrabbable from a script (#341).
MEM_1 = {"left": 60, "top": 100, "width": 260, "height": 160}
MEM_2 = {"left": 340, "top": 100, "width": 260, "height": 160}
MEM_3 = {"left": 60, "top": 280, "width": 260, "height": 160}

# Sampled from DOCUMENT START, because "nothing ever appeared" cannot be read
# after the page settles — the plane would already have put back and removed the
# windows by then (KNOWLEDGE, #334/#339).
#
# It counts `addedNodes`, NOT a re-query of the document: observer callbacks are
# batched at the microtask checkpoint, so an insert-then-remove inside one
# checkpoint — exactly the shape of a naive "spawn, then re-attach" regression —
# would score 0 against a re-query and pass vacuously.
FLASH_ORACLE = (
    "window.__flash = { peak: 0, added: 0 };"
    " new MutationObserver((records) => {"
    "   for (const r of records) {"
    "     for (const n of r.addedNodes) {"
    "       if (n.nodeType !== 1) continue;"
    "       if (n.classList && n.classList.contains('session-window')) window.__flash.added++;"
    "       if (n.querySelectorAll)"
    "         window.__flash.added += n.querySelectorAll('.session-window').length;"
    "     }"
    "   }"
    "   window.__flash.peak = Math.max(window.__flash.peak,"
    "     document.querySelectorAll('.session-window').length); })"
    "  .observe(document, { childList: true, subtree: true });"
)

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
    empty = tempfile.mkdtemp(prefix="wb347_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_DAEMON_AGENT_OVERRIDE=CHILD,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )


def make_fixture_repo(tag):
    d = tempfile.mkdtemp(prefix=f"wb347_{tag}_")
    p = Path(d)
    (p / "README.md").write_text(f"# fixture {tag}\n\nThe #347 reload fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb347@example.com"],
        ["git", "config", "user.name", "wb347"],
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


def window_toml(wid, repo, rect, ts):
    return (
        "[[windows]]\n"
        f'id = "{wid}"\n'
        f'repo = "{repo}"\n'
        'agent = "console"\n'
        'kind = "console"\n'
        "max = false\n"
        f"ts = {ts}\n"
        f"{rect_toml(rect)}\n\n"
    )


def placeholder_toml(wid, repo, rect, ts):
    """An `agent` record with no live session — restores as a PLACEHOLDER."""
    return (
        "[[windows]]\n"
        f'id = "{wid}"\n'
        f'repo = "{repo}"\n'
        'agent = "claude"\n'
        'kind = "agent"\n'
        "max = false\n"
        f"ts = {ts}\n"
        f"{rect_toml(rect)}\n\n"
    )


def fence_toml(fid, name, rect, ts):
    return "[[fences]]\n" f'id = "{fid}"\n' f'name = "{name}"\n' f"ts = {ts}\n" f"{rect_toml(rect)}\n\n"


def write_fixture_desk(daemon_dir, slug):
    Path(daemon_dir, "desk.toml").write_text(
        window_toml("w-m1", slug, MEM_1, 100)
        + window_toml("w-m2", slug, MEM_2, 101)
        + placeholder_toml("w-m3", slug, MEM_3, 102)
        + fence_toml("f-alpha", "alpha", FENCE_A, 110)
        + fence_toml("f-beta", "beta", FENCE_B, 111),
        encoding="utf-8",
    )


def build():
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # the previous build's console.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"],
        cwd=REPO_ROOT,
        check=True,
    )


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


def settle_windows(page, want, timeout=30000):
    """Wait for the windows to be REAL boxes.

    KNOWLEDGE: an `x-show` flip is not visible to the next evaluate, and a
    still-hidden box measures 0x0 — which passes a geometry assertion vacuously.
    """
    page.wait_for_function(
        "(n) => { const ws = [...document.querySelectorAll('.session-window')];"
        " return ws.length === n && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        arg=want,
        timeout=timeout,
    )
    page.wait_for_timeout(500)


def activate_consoles(page, fences=2):
    """Bring the Consoles tab into view and wait for the fences to land.

    `activate` and not a raw `active =` write: only the former reaches
    `refitAll()`, the path that re-applies a stored offset after `display:none`
    threw the scroll position away (KNOWLEDGE, #339).
    """
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    page.wait_for_timeout(1800)
    page.wait_for_function(
        "(n) => document.querySelectorAll('.fence').length === n", arg=fences, timeout=15000
    )


def desk_page(ctx, windows=3, fences=2):
    page = ctx.new_page()
    page.set_viewport_size(dict(VIEW))
    page.goto(BASE)
    activate_consoles(page, fences)
    settle_windows(page, windows)
    return page


def click_sel(page, sel):
    """Click a control through the element itself.

    KNOWLEDGE (#309-#338): only the TOPMOST element is hittable, and a fence's
    chrome can sit under a console or off-viewport — `locator.click()` then
    times out or presses the wrong thing.
    """
    page.evaluate(
        "(s) => { const el = document.querySelector(s);"
        " if (!el) throw new Error('no element for ' + s); el.click(); }",
        sel,
    )


def boxes(page):
    return page.evaluate(
        "() => [...document.querySelectorAll('.session-window')].map((w) => ({"
        "  id: w._deskId,"
        "  left: w.offsetLeft, top: w.offsetTop,"
        "  width: w.offsetWidth, height: w.offsetHeight }))"
    )


def by_id(rows):
    return {r["id"]: r for r in rows}


def members_of(page, fid):
    """The windows whose box lies inside a fence, measured on the PLANE."""
    return page.evaluate(
        "(id) => { const f = document.querySelector(`[data-fence-id='${id}']`);"
        " if (!f) return -1;"
        " const fl = f.offsetLeft, ft = f.offsetTop,"
        "   fr = fl + f.offsetWidth, fb = ft + f.offsetHeight;"
        " return [...document.querySelectorAll('.session-window')].filter((w) =>"
        "   w.offsetLeft >= fl && w.offsetTop >= ft &&"
        "   w.offsetLeft + w.offsetWidth <= fr &&"
        "   w.offsetTop + w.offsetHeight <= fb).length; }",
        fid,
    )


def glyph_visible(page, fid):
    """Gated on geometry ALONE, never on presence and never on the attribute.

    KNOWLEDGE: `hidden` is styled by the UA, and any author `display` rule beats
    it — so `!g.hidden` can be true of a glyph that is painted on screen. An
    oracle that ANDs the attribute with the geometry reports what the code
    intended instead of what the operator sees.
    """
    return page.evaluate(
        "(id) => { const g = document.querySelector(`[data-fence-id='${id}'] .fence-detached`);"
        " return !!g && g.offsetParent !== null && g.clientWidth > 0"
        "   && getComputedStyle(g).display !== 'none'; }",
        fid,
    )


def registry_of(page):
    """The raw registry record this TAB holds, or None."""
    raw = page.evaluate("(k) => sessionStorage.getItem(k)", DETACH_KEY)
    return json.loads(raw) if raw else None


def detach(page, fid):
    """Detach a fence and return its popup Page."""
    with page.expect_popup(timeout=15000) as info:
        click_sel(page, f"[data-fence-id='{fid}'] .fence-detach")
    popup = info.value
    popup.wait_for_load_state()
    return popup


def popup_windows(popup, want, timeout=30000):
    popup.wait_for_function(
        "(n) => { const ws = [...document.querySelectorAll('.session-window')];"
        " return ws.length === n && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        arg=want,
        timeout=timeout,
    )
    popup.wait_for_timeout(400)


def screen(page, i=0):
    """The i-th console window's whole terminal buffer as text."""
    return page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " const b = w && w._term && w._term.term && w._term.term.buffer.active;"
        " if (!b) return '';"
        " let out = '';"
        " for (let y = 0; y < b.length; y++) {"
        "   const line = b.getLine(y);"
        "   if (line) out += line.translateToString(true) + '\\n';"
        " }"
        " return out; }",
        i,
    )


def type_line(page, i, text):
    """Feed one line through xterm's own data path, as ONE onData event."""
    page.locator(".session-window").nth(i).locator(".xterm").click()
    page.evaluate(
        "([i, t]) => document.querySelectorAll('.session-window')[i]._term.term.paste(t + '\\r')",
        [i, text],
    )


def reached_child(page, i, token, timeout=20000):
    """A `GOT:` line CONTAINING the token — never equality.

    KNOWLEDGE (#334): after a client reattaches it sends a resize and ConPTY
    repaints its cooked-mode buffer with the PREVIOUS line still in it, so the
    next line typed arrives as `<previous><token>`.
    """
    deadline = time.time() + timeout / 1000
    while time.time() < deadline:
        buf = screen(page, i)
        if any(token in l for l in buf.split("\n") if l.startswith("GOT:")):
            return True
        if token in buf.replace("\n", "") and "GOT:" in buf:
            return True
        page.wait_for_timeout(300)
    return False


def home_within(page, want, ms):
    """Poll for the consoles being back on the plane, with a DEADLINE.

    The deadline is the discrimination between the two re-attach paths: the 6 s
    peer expiry would also bring them home, just far later.
    """
    deadline = time.time() + ms / 1000
    while time.time() < deadline:
        try:
            if members_of(page, "f-alpha") == want:
                return True
        except Exception:
            pass
        page.wait_for_timeout(100)
    return False


def closed_within(popup, ms):
    """Wait on Playwright's own `close` event, never on a polled `is_closed()`.

    MEASURED: a `time.sleep` loop does not pump the sync driver, so `is_closed()`
    stays False over a page whose target is already gone — the next `evaluate`
    then raises `TargetClosedError`. The popup was closing all along.
    """
    if popup.is_closed():
        return True
    try:
        popup.wait_for_event("close", timeout=ms)
        return True
    except Exception:
        return popup.is_closed()


def lost_notice_within(popup, ms):
    """Poll for the popup's peer-lost notice BEFORE it closes itself.

    A 200 ms poll, because the notice lives for only the 1500 ms between the
    expiry and `window.close()` — reading it once, after the fact, is reading a
    closed page.
    """
    deadline = time.time() + ms / 1000
    while time.time() < deadline:
        if popup.is_closed():
            return None
        try:
            text = popup.evaluate(
                "() => { const p = document.querySelector('.detached-lost');"
                " return p ? p.textContent.trim() : null; }"
            )
        except Exception:
            return None
        if text:
            return text
        popup.wait_for_timeout(200)
    return None


def served_fence(fid):
    for f in json.loads(http("GET", "api/desk")[1]).get("fences", []):
        if f.get("id") == fid:
            return f.get("rect")
    return None


def centre_of(page, sel):
    return page.evaluate(
        "(s) => { const e = document.querySelector(s); if (!e) return null;"
        " const r = e.getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
        sel,
    )


def drag(page, start, dx, dy):
    page.mouse.move(start["x"], start["y"])
    page.mouse.down()
    page.mouse.move(start["x"] + dx / 3, start["y"] + dy / 3, steps=5)
    page.mouse.move(start["x"] + dx * 2 / 3, start["y"] + dy * 2 / 3, steps=5)
    page.mouse.move(start["x"] + dx, start["y"] + dy, steps=5)
    page.mouse.up()
    page.wait_for_timeout(600)


def desk_shape():
    """The daemon's desk, reduced to what a UI-only change must never move."""
    desk = json.loads(http("GET", "api/desk")[1])
    return {
        "windows": sorted((w["id"], json.dumps(w["rect"], sort_keys=True)) for w in desk.get("windows", [])),
        "fences": sorted((f["id"], json.dumps(f["rect"], sort_keys=True)) for f in desk.get("fences", [])),
        "keys": sorted(desk.keys()),
    }


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb347_reg_")
    desk_file = Path(daemon_dir, "desk.toml")
    slug = register_fixture(daemon_dir, make_fixture_repo("one"))
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
            # Installed on the CONTEXT, so it is armed on the reloaded document
            # and on every popup before a line of app code runs.
            ctx.add_init_script(FLASH_ORACLE)
            errors = []
            ctx.on("weberror", lambda e: errors.append(str(e.error)))

            # Every PUT /api/desk this context issues, tagged with the page that
            # issued it — scenario 8's oracle for "the popup writes nothing".
            puts = []

            def on_request(req):
                if req.method == "PUT" and "/api/desk" in req.url:
                    # The frame's URL is captured HERE, at request time: reading
                    # it later, off a page this suite deliberately closes, is
                    # reading a dead handle.
                    try:
                        who, where = req.frame.page, req.frame.url
                    except Exception:
                        who, where = None, None
                    puts.append({"page": who, "url": where})

            ctx.on("request", on_request)

            page = desk_page(ctx)
            before_boxes = by_id(boxes(page))
            quiet(desk_file)
            desk_before = desk_shape()

            check(
                "the fixture puts all three members inside f-alpha",
                members_of(page, "f-alpha") == 3,
                f"members={members_of(page, 'f-alpha')}",
            )
            check(
                "…and this tab holds no detach registry before the detach",
                registry_of(page) is None,
                f"registry={registry_of(page)!r}",
            )

            popup = detach(page, "f-alpha")
            popup_windows(popup, 3)
            reg_detached = registry_of(page)
            check(
                "detaching writes the fence into this tab's session-scoped store",
                reg_detached is not None
                and reg_detached.get("fences") == ["f-alpha"]
                and reg_detached.get("v") == 1
                and isinstance(reg_detached.get("tab"), str),
                f"registry={reg_detached!r}",
            )
            popups_before_reload = [pg for pg in ctx.pages if "detached-fence.html" in pg.url]

            # ---- scenario 1: THE F5 ------------------------------------------
            page.reload()
            activate_consoles(page)
            # The channel round trip plus the desk restore: let the whole boot
            # settle before reading the "nothing ever appeared" counter, or it
            # would be read before the plane had the chance to be wrong.
            page.wait_for_timeout(3000)

            check(
                "the popup SURVIVES the origin tab's reload",
                (not popup.is_closed())
                and [pg for pg in ctx.pages if "detached-fence.html" in pg.url] == popups_before_reload,
                f"closed={popup.is_closed()}",
            )
            popup_live = popup.evaluate(
                "() => [...document.querySelectorAll('.session-window')]"
                "  .filter((w) => w.querySelector('.xterm-screen')).length"
            )
            check(
                "…still holding this fence's consoles, LIVE",
                popup_live == 2,
                f"windows-with-a-terminal={popup_live} (of 3 members; w-m3 is a placeholder)",
            )
            check(
                "the reloaded fence renders NO member",
                members_of(page, "f-alpha") == 0,
                f"members={members_of(page, 'f-alpha')}",
            )
            check(
                "…and shows its detach glyph, so the way home survived too",
                glyph_visible(page, "f-alpha"),
                "hidden or zero-sized" if not glyph_visible(page, "f-alpha") else "",
            )
            reg_after = registry_of(page)
            check(
                "the registry SURVIVED the reload, tab identity and all",
                reg_after is not None
                and reg_after.get("fences") == ["f-alpha"]
                and reg_after.get("tab") == reg_detached.get("tab"),
                f"before={reg_detached!r} after={reg_after!r}",
            )
            flash = page.evaluate("() => window.__flash")
            check(
                "NO FLASH: not one console was ever inserted on the plane after the reload",
                flash["peak"] == 0 and flash["added"] == 0,
                f"peak={flash['peak']} inserted={flash['added']}",
            )
            fence_count = page.evaluate("() => document.querySelectorAll('.fence').length")
            check(
                "…and the fences themselves did come back — the negative control"
                " for a peak of 0 over an empty page",
                fence_count == 2,
                f"fences={fence_count}",
            )
            page.screenshot(path=SHOT, full_page=False)

            # ---- scenario 2: the popup is still driveable --------------------
            live_i = popup.evaluate(
                "() => [...document.querySelectorAll('.session-window')]"
                "  .findIndex((w) => w.querySelector('.xterm-screen'))"
            )
            type_line(popup, live_i, "probe-347-reload")
            check(
                "a popup console is still driveable after its origin tab reloaded",
                live_i >= 0 and reached_child(popup, live_i, "probe-347-reload"),
                f"i={live_i} buffer={screen(popup, max(live_i, 0))[-160:]!r}",
            )

            # ---- scenario 3: a SECOND tab sees no detach ---------------------
            page_b = desk_page(ctx)
            check(
                "a second tab of the same browser renders the fence WITH its consoles",
                members_of(page_b, "f-alpha") == 3,
                f"members={members_of(page_b, 'f-alpha')}",
            )
            check(
                "…shows no detach glyph",
                not glyph_visible(page_b, "f-alpha"),
                "",
            )
            check(
                "…and holds NO registry of its own — the store is per-tab",
                registry_of(page_b) is None,
                f"registry={registry_of(page_b)!r}",
            )
            # THE ANTI-VACUITY CONTROL for scenario 1's `inserted == 0`: the same
            # init script, on the same kind of document, over a page that really
            # does put three consoles on the plane. Without this, an observer
            # that never fired at all would satisfy the no-flash assertion.
            flash_b = page_b.evaluate("() => window.__flash")
            check(
                "…and the flash counter really counts — this tab's own reads 3, same oracle",
                flash_b["added"] == 3,
                f"inserted-here={flash_b['added']} (scenario 1 read 0 on the reloaded tab)",
            )
            check(
                "…while tab one's popup is untouched, still holding its live consoles",
                (not popup.is_closed())
                and popup.evaluate(
                    "() => [...document.querySelectorAll('.session-window')]"
                    "  .filter((w) => w.querySelector('.xterm-screen')).length"
                )
                == 2,
                f"closed={popup.is_closed()}",
            )
            check(
                "…and tab one still renders the fence detached",
                members_of(page, "f-alpha") == 0 and glyph_visible(page, "f-alpha"),
                f"members={members_of(page, 'f-alpha')}",
            )
            page_b.close()
            page.wait_for_timeout(800)

            # ---- scenario 8 (half): the daemon has not moved -----------------
            quiet(desk_file)
            desk_mid = desk_shape()
            check(
                "GET /api/desk is unchanged across the detach and the reload",
                desk_mid == desk_before,
                f"before={desk_before} after={desk_mid}",
            )
            check(
                "…and NOT ONE PUT /api/desk was issued from a popup context",
                # `len(puts) > 0` is the anti-vacuity leg: an `all()` over an
                # empty list passes with the request listener mis-wired.
                len(puts) > 0
                and all(r["page"] is not popup for r in puts)
                and all(r["url"] is not None for r in puts),
                f"puts-from-popup={sum(1 for r in puts if r['page'] is popup)}"
                f" unattributable={sum(1 for r in puts if r['url'] is None)} total={len(puts)}",
            )

            # ---- scenario 4: re-attach by CLOSING the popup, post-reload -----
            popup.close(run_before_unload=True)
            came_home = home_within(page, 3, 2000)
            settle_windows(page, 3)
            page.wait_for_timeout(400)
            back = by_id(boxes(page))
            check(
                "closing the popup after a reload brings the consoles home, within 2000 ms",
                came_home,
                f"members={members_of(page, 'f-alpha')}",
            )
            check(
                "…each at the EXACT box it was detached from",
                all(back.get(k) == before_boxes.get(k) for k in before_boxes),
                f"before={before_boxes} after={back}",
            )
            check(
                "…and the glyph is gone",
                not glyph_visible(page, "f-alpha"),
                "",
            )
            check(
                "…leaving an empty registry behind, not a stale one",
                (registry_of(page) or {}).get("fences") == [],
                f"registry={registry_of(page)!r}",
            )

            # ---- scenario 5: re-attach by the GLYPH, post-reload -------------
            popup2 = detach(page, "f-alpha")
            popup_windows(popup2, 3)
            page.reload()
            activate_consoles(page)
            page.wait_for_timeout(2500)
            check(
                "a second detach also survives a reload",
                members_of(page, "f-alpha") == 0 and glyph_visible(page, "f-alpha"),
                f"members={members_of(page, 'f-alpha')} glyph={glyph_visible(page, 'f-alpha')}",
            )
            # ABRUPT: no `beforeunload`, so nothing announces the departure, and
            # after the reload this document holds no handle either — the peer
            # expiry would eventually notice, six seconds later. The glyph is
            # what makes the wait unnecessary: it does not ASK whether the popup
            # is still there, it evicts it over the channel and brings the
            # consoles home either way.
            popup2.close()
            page.wait_for_timeout(300)
            glyph_t0 = time.time()
            click_sel(page, "[data-fence-id='f-alpha'] .fence-detached")
            came_home2 = home_within(page, 3, 2000)
            check(
                "the GLYPH brings the consoles home after a reload and an abrupt popup close,"
                " well inside the 6 s peer expiry",
                came_home2 and (time.time() - glyph_t0) < 2.0,
                f"elapsed={time.time() - glyph_t0:.2f}s members={members_of(page, 'f-alpha')}",
            )
            settle_windows(page, 3)
            page.wait_for_timeout(400)
            back2 = by_id(boxes(page))
            check(
                "…returning every console to its original box",
                all(back2.get(k) == before_boxes.get(k) for k in before_boxes),
                f"before={before_boxes} after={back2}",
            )
            check(
                "…and clearing the glyph",
                not glyph_visible(page, "f-alpha"),
                "",
            )

            # ---- scenario 5b: a MOVED detached fence still skips its members --
            # The registry must carry the member IDS, not a geometry the drag
            # invalidates: a detached fence is still movable (#346 §7a), and a
            # membership fold run after the reload compares the fence's NEW rect
            # with the members' ORIGINAL records — answering "no members", and
            # putting every one of them back under the live popup.
            popup2b = detach(page, "f-alpha")
            popup_windows(popup2b, 3)
            rect_before = served_fence("f-alpha")
            # A REAL titlebar drag, through `.fence-grab`: writing inline styles
            # bypasses the persist path and `/api/desk` would keep the old rect,
            # making the whole scenario vacuous (KNOWLEDGE, #336/#337).
            # +120/+320 keeps alpha (40,40 600x460) clear of beta (800,40 320x300)
            # on the X axis: a drop that overlaps a sibling is REFUSED and
            # silently reverted, which reads exactly like a dead handle (#346).
            drag(page, centre_of(page, "[data-fence-id='f-alpha'] .fence-grab"), 120, 320)
            quiet(desk_file)
            rect_moved = served_fence("f-alpha")
            check(
                "a detached fence really MOVED on the plane, and the daemon stored the new rect",
                rect_moved is not None
                and rect_before is not None
                and (
                    round(rect_moved["left"] - rect_before["left"]),
                    round(rect_moved["top"] - rect_before["top"]),
                )
                == (120, 320),
                f"before={rect_before} moved={rect_moved}",
            )
            page.reload()
            activate_consoles(page)
            page.wait_for_timeout(3000)
            moved_flash = page.evaluate("() => window.__flash")
            on_plane = page.evaluate("() => document.querySelectorAll('.session-window').length")
            check(
                "…and after a reload its members STAY off the plane —"
                " the registry carries their ids, not a geometry the drag invalidated",
                moved_flash["added"] == 0 and on_plane == 0,
                f"inserted={moved_flash['added']} on-plane={on_plane}",
            )
            check(
                "…and its popup is untouched, still driving the same live consoles",
                (not popup2b.is_closed())
                and popup2b.evaluate(
                    "() => [...document.querySelectorAll('.session-window')]"
                    "  .filter((w) => w.querySelector('.xterm-screen')).length"
                )
                == 2,
                f"closed={popup2b.is_closed()}",
            )
            # Put the fence back WHILE IT IS STILL DETACHED. Once the members are
            # home they sit at their ORIGINAL rects — MEM_3 covers the moved
            # fence's 14 px NW `.fence-grab`, and a member parked on a handle
            # makes the fence ungrabbable from a script AND for an operator
            # (KNOWLEDGE, #341). Detached, the plane is clear.
            drag(page, centre_of(page, "[data-fence-id='f-alpha'] .fence-grab"), -120, -320)
            quiet(desk_file)
            rect_home = served_fence("f-alpha")
            check(
                "…and the fence drags back to where it started, so the plane is at baseline again",
                rect_home == rect_before,
                f"start={rect_before} back={rect_home}",
            )
            popup2b.close(run_before_unload=True)
            home_within(page, 3, 3000)
            settle_windows(page, 3)
            page.wait_for_timeout(400)
            check(
                "…and the members come home INSIDE the restored fence",
                members_of(page, "f-alpha") == 3,
                f"members={members_of(page, 'f-alpha')}",
            )

            # ---- scenario 5c: a SILENT origin is not a dead one ---------------
            # The defect this answers: the popup closed itself mid-work and the
            # consoles came home from under the operator. Chrome throttles a
            # hidden tab's timers to one tick per MINUTE after about five
            # minutes, so a workbench sitting behind another tab stops beating
            # while it is perfectly alive — and six seconds of silence read a
            # working popup as a dead one.
            #
            # Simulated where throttling actually bites: the origin's periodic
            # POST is dropped, and nothing else is. Message DELIVERY is not
            # timer-throttled, so the origin still answers a probe from its
            # message handler — which is the whole reason silence must not be
            # the verdict.
            popup2c = detach(page, "f-alpha")
            popup_windows(popup2c, 3)
            page.evaluate(
                "() => { const post = BroadcastChannel.prototype.postMessage;"
                " window.__unthrottle = () => { BroadcastChannel.prototype.postMessage = post; };"
                " BroadcastChannel.prototype.postMessage = function (m) {"
                "   if (m && m.type === 'origin-beat') return;"
                "   return post.call(this, m); }; }"
            )
            quiet_t0 = time.time()
            # Two and a half peer windows: one expiry would be survivable by
            # accident, the second is what the probe has to carry.
            page.wait_for_timeout(int(PEER_WINDOW_MS * 2.5))
            check(
                "an origin whose timers are throttled does NOT close its popup",
                not popup2c.is_closed(),
                f"silent for {time.time() - quiet_t0:.1f}s"
                f" ({PEER_WINDOW_MS / 1000}s window)",
            )
            in_popup = (
                0
                if popup2c.is_closed()
                else popup2c.evaluate("() => document.querySelectorAll('.session-window').length")
            )
            on_plane = page.evaluate("() => document.querySelectorAll('.session-window').length")
            check(
                "…and the consoles are still in it, not yanked home",
                in_popup == 3 and on_plane == 0,
                f"popup={in_popup} plane={on_plane}",
            )
            page.evaluate("() => window.__unthrottle()")
            popup2c.close(run_before_unload=True)
            home_within(page, 3, 4000)
            settle_windows(page, 3)
            page.wait_for_timeout(400)

            # ---- scenario 6: FORCE-KILLING the origin tab --------------------
            popup3 = detach(page, "f-alpha")
            popup_windows(popup3, 3)
            # Playwright's default runs NO `beforeunload`: the origin document
            # simply stops existing, which is the crash/force-kill case. Only
            # silence can be the signal.
            kill_t0 = time.time()
            page.close()
            notice = lost_notice_within(popup3, 12000)
            check(
                "a force-killed origin tab leaves its popup saying WHY it is going",
                notice is not None
                and "the workbench window that opened this one is gone" in notice,
                f"notice={notice!r} after={time.time() - kill_t0:.1f}s",
            )
            check(
                "…and the popup then closes ITSELF, within the heartbeat window plus its notice",
                closed_within(popup3, 15000),
                f"closed={popup3.is_closed()} after={time.time() - kill_t0:.1f}s",
            )
            check(
                "…which is longer than a reload ever takes — the F5 rule is intact",
                (time.time() - kill_t0) > PEER_WINDOW_MS / 1000,
                f"elapsed={time.time() - kill_t0:.1f}s window={PEER_WINDOW_MS / 1000}s",
            )

            # ---- scenario 7: CLOSING the origin tab cleanly ------------------
            page_c = desk_page(ctx)
            check(
                "a fresh tab renders the fence normally once every popup is gone",
                members_of(page_c, "f-alpha") == 3 and not glyph_visible(page_c, "f-alpha"),
                f"members={members_of(page_c, 'f-alpha')}",
            )
            popup4 = detach(page_c, "f-alpha")
            popup_windows(popup4, 3)
            close_t0 = time.time()
            page_c.close(run_before_unload=True)
            check(
                "closing the origin tab closes its popup too",
                closed_within(popup4, 15000),
                f"closed={popup4.is_closed()} after={time.time() - close_t0:.1f}s",
            )

            # ---- scenario 8 (rest): NO DAEMON CHANGE -------------------------
            quiet(desk_file)
            desk_end = desk_shape()
            check(
                "the daemon's desk carries the same windows, fences and rects it started with",
                desk_end == desk_before,
                f"before={desk_before} end={desk_end}",
            )
            check(
                "…and no new top-level key appeared on GET /api/desk",
                desk_end["keys"] == desk_before["keys"],
                f"keys={desk_end['keys']}",
            )
            check(
                "no popup context EVER issued a PUT /api/desk across the whole pass",
                len(puts) > 0
                and all(r["url"] is not None for r in puts)
                and not any("detached-fence.html" in r["url"] for r in puts),
                f"puts={len(puts)} urls={sorted({r['url'] for r in puts})}",
            )

            check("no page error was raised by the whole pass", errors == [], f"weberrors={errors}")
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The floor is the REAL count, not a loose lower bound: set under the total,
    # a whole scenario could stop running while the suite still exits 0.
    ok = all(results) and len(results) == 44
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("THE DETACH SURVIVES AN F5, AND DIES WITH THE TAB")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
