"""Columns (#473, ADR-0051 §5 and §8) browser acceptance: the columns stay
correct across a reload, a narrower viewport, the keys, and changes made by
another client.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7473.

The fixture `desk.toml` is the one `wb_columns_472.py` writes:
  w-a  loose, live shell (`kind = "console"`), maximized
  w-b  loose, live shell
  w-c  loose, placeholder (`kind = "agent"`: full chrome, no PTY)
  w-f  placeholder inside fence f-one
  w-l  placeholder inside fence f-lock (`locked = true`)

R1  after a reload the same columns come back, in the same order
R2  a new tab of the same browser shows the same columns
R3  another browser profile shows only the desk's maximized console
R4  a stored id that is no longer on the desk is dropped
R5  a stored list whose first id is not the desk's maximized console is ignored
R6  only window ids are stored, and nothing about columns reaches the desk
N1  a narrower viewport keeps every column; a phone width paints only the
    leftmost and keeps the list; wider brings them back
N2  a font change keeps every column
S1  the Settings text size shows the current size, and a new size changes
    every console and is stored
N3  the focused column stops being painted: the focus moves to the rightmost
    painted column
K1  a click gives a column the focus, and the mark shows between touching
    columns
K2  Alt+Shift+→/← walks the columns from inside a terminal, and wraps
X1  a session that ends keeps its column; restart starts it again in place
X2  a console closed by another client leaves the columns
X3  detaching a fence takes its consoles out of the columns before the popup
    loads; the popup offers no column button
X4  a console maximized by another client stays behind the columns
X5  a rect or fence change from another client leaves the columns alone

The column cap measures `#workspace`, not the window: at or below 560 px of
workspace only the leftmost is painted.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_columns_473.py   (exit 0 = all pass)
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

PORT = 7473
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT = os.path.join(REPO_ROOT, ".ralphy", "screenshots", "473-columns-2026-09-26.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
VIEW = {"width": 2400, "height": 1000}
PHONE = {"width": 480, "height": 1000}
NARROW = {"width": 1000, "height": 1000}
FLOOR = 63  # every check above the floor check; pinned after the first green run

F_ONE = {"left": 40, "top": 40, "width": 600, "height": 500}
F_LOCK = {"left": 700, "top": 40, "width": 600, "height": 500}
F_EMPTY = {"left": 40, "top": 700, "width": 400, "height": 300}
WINDOWS = [
    # id, kind, rect, max
    ("w-a", "console", {"left": 1400, "top": 700, "width": 500, "height": 300}, True),
    ("w-b", "console", {"left": 1400, "top": 1100, "width": 500, "height": 300}, False),
    ("w-c", "agent", {"left": 2000, "top": 700, "width": 400, "height": 300}, False),
    ("w-f", "agent", {"left": 80, "top": 100, "width": 400, "height": 300}, False),
    ("w-l", "agent", {"left": 740, "top": 100, "width": 400, "height": 300}, False),
]
LIVE = {"w-a", "w-b"}

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
    empty = tempfile.mkdtemp(prefix="wbcol_empty_")
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
    d = tempfile.mkdtemp(prefix="wbcol_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe columns fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbcol@example.com"],
        ["git", "config", "user.name", "wbcol"],
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
    out = ""
    for ts, (wid, kind, rect, mx) in enumerate(WINDOWS, start=100):
        agent = "console" if kind == "console" else "claude"
        out += (
            "[[windows]]\n"
            f'id = "{wid}"\n'
            f'repo = "{slug}"\n'
            f'agent = "{agent}"\n'
            f'kind = "{kind}"\n'
            f"max = {'true' if mx else 'false'}\n"
            f"ts = {ts}\n"
            f"{rect_toml(rect)}\n\n"
        )
    for ts, (fid, name, rect, locked) in enumerate(
        [
            ("f-one", "one", F_ONE, False),
            ("f-lock", "held", F_LOCK, True),
            ("f-empty", "empty", F_EMPTY, False),
        ],
        start=200,
    ):
        out += (
            "[[fences]]\n"
            f'id = "{fid}"\n'
            f'name = "{name}"\n'
            + ("locked = true\n" if locked else "")
            + f"ts = {ts}\n"
            f"{rect_toml(rect)}\n\n"
        )
    Path(daemon_dir, "desk.toml").write_bytes(out.encode("utf-8"))


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


def desk_raw():
    return json.loads(http("GET", "api/desk")[1])


def desk_windows():
    return {r["id"]: r for r in desk_raw()["windows"]}


def poll_desk(predicate, timeout=8):
    """A desk write is debounced-then-HTTP: poll, never one fixed sleep."""
    deadline = time.time() + timeout
    got = None
    while time.time() < deadline:
        try:
            got = desk_windows()
        except Exception:
            got = None
        if got and predicate(got):
            return got
        time.sleep(0.3)
    return got


# In-page helpers, installed once per page.
HELPERS = """() => {
  window.__W = (id) => [...document.querySelectorAll('#stage .session-window')].find((w) => w._deskId === id) || null;
  window.__colBtn = (id) => __W(id)?.querySelector('.session-column') || null;
  window.__columns = () => {
    const ws = document.getElementById('workspace').getBoundingClientRect();
    return [...document.querySelectorAll('.session-window.column')]
      .map((w) => { const r = w.getBoundingClientRect();
        return { id: w._deskId, left: r.left - ws.left, right: r.right - ws.left, width: r.width,
                 max: w.classList.contains('maximized') }; })
      .sort((a, b) => a.left - b.left);
  };
  window.__focused = () => [...document.querySelectorAll('.session-window.focused')].map((w) => w._deskId);
  window.__active = () => document.activeElement?.closest?.('.session-window')?._deskId ?? null;
  window.__inXterm = () => !!document.activeElement?.closest?.('.xterm');
}"""


def ids(page):
    return [c["id"] for c in page.evaluate("() => __columns()")]


def shell_cols(page):
    return page.evaluate(f"() => {SH}.columns.flat()")


def stored(page):
    return page.evaluate("() => WBView.read()?.columns?.flat() ?? null")


def boot(page, want=5):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    page.wait_for_function(
        "(n) => { const ws = [...document.querySelectorAll('#stage .session-window')];"
        " return ws.length === n && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        arg=want,
        timeout=20000,
    )
    page.evaluate(HELPERS)
    try:
        page.wait_for_function(
            "() => ['w-a','w-b'].every((id) => !__W(id) || __W(id)?._term?.term?.cols > 0)", timeout=15000
        )
    except Exception:
        pass
    page.wait_for_timeout(600)


def reload(page, want=5):
    page.reload()
    boot(page, want)


def open_menu(page, from_id):
    page.evaluate("(id) => __colBtn(id).click()", from_id)
    page.wait_for_function(
        f"() => {SH}.columnMenu === true && document.querySelector('.column-menu').getClientRects().length > 0",
        timeout=5000,
    )
    page.wait_for_timeout(150)


def open_column(page, from_id, id):
    open_menu(page, from_id)
    page.locator(f".column-menu .column-item[data-id='{id}']").click()
    page.wait_for_timeout(500)


def press_max(page, id):
    page.evaluate("(id) => __W(id).querySelector('.session-max').click()", id)
    page.wait_for_timeout(500)


def equal_fill(cols, ws_width):
    n = len(cols)
    if n < 2:
        return False
    want = ws_width / n
    widths = all(abs(c["width"] - want) <= 1 for c in cols)
    edges = all(abs(c["left"] - i * want) <= 1 for i, c in enumerate(cols))
    total = abs(sum(c["width"] for c in cols) - ws_width) <= n
    return widths and edges and total


def click_centre(page, id):
    """A click inside the window body, near its bottom-left corner. MEASURED: the
    centre of a placeholder is its Relaunch control, which starts a real
    vendor CLI."""
    box = page.evaluate(
        "(id) => { const r = __W(id).querySelector('.session-body').getBoundingClientRect();"
        " return { x: r.left + 24, y: r.bottom - 24 }; }",
        id,
    )
    page.mouse.click(box["x"], box["y"])
    page.wait_for_timeout(250)


def confirm(page):
    try:
        page.wait_for_selector(".wb-confirm", timeout=3000)
    except Exception:
        return False
    page.locator(".wb-confirm .modal-foot .btn.danger, .wb-confirm .modal-foot .btn.accent").first.click()
    page.wait_for_timeout(300)
    return True


def main():
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbcol_daemon_")
    fixture = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture)
    write_fixture_desk(daemon_dir, slug)
    proc = launch(daemon_dir)
    errors = []
    try:
        if not wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch()
            ctx = browser.new_context(viewport=dict(VIEW))
            # X3: MEASURED (Playwright 1.60, Chromium): the init script runs once
            # in a popup, on its initial `about:blank`, created INSIDE
            # `window.open` — before `detachFence` returns. The Window object is
            # kept when `detached-fence.html` replaces the document, so a
            # zero-delay timer reads the opener's columns after `detachFence`
            # has returned (measured: it can fire after the document commits).
            ctx.add_init_script(
                "if (window.opener && location.href === 'about:blank') setTimeout(() => {"
                " try { const o = window.opener;"
                "   window.__openerCols = o.Alpine.$data(o.document.querySelector('[x-data]')).columns.flat();"
                "   window.__openerAt = location.href;"
                " } catch (e) { window.__openerCols = 'error: ' + e; } }, 0);"
            )
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            boot(page)

            probe = page.evaluate(
                "() => Object.fromEntries([...document.querySelectorAll('.session-window')].map((w) =>"
                "  [w._deskId, { term: !!w._term, placeholder: w.classList.contains('placeholder') }]))"
            )
            ok = all(probe[w]["term"] for w in LIVE) and all(
                probe[w]["placeholder"] for w in probe if w not in LIVE
            )
            check("probe: shells are live, agent records are placeholders", ok, str(probe))
            if not ok:
                return
            cap = page.evaluate(f"() => {SH}.columnCap()")
            check("wider than a phone the columns have no limit", cap == float("inf"), f"cap={cap}")
            check("boot writes no column list", stored(page) is None, str(stored(page)))

            open_column(page, "w-a", "w-b")
            open_column(page, "w-b", "w-c")
            check("setup: three columns", ids(page) == ["w-a", "w-b", "w-c"], str(ids(page)))

            # R1 -------------------------------------------------------------
            reload(page)
            try:
                page.wait_for_function(f"() => {SH}.columns.flat().length === 3", timeout=8000)
            except Exception:
                pass
            check("R1 after a reload the list comes back", shell_cols(page) == ["w-a", "w-b", "w-c"],
                  str(shell_cols(page)))
            check("R1 …painted in the same order", ids(page) == ["w-a", "w-b", "w-c"], str(ids(page)))

            # R2 -------------------------------------------------------------
            tab = ctx.new_page()
            tab.on("pageerror", lambda e: errors.append("tab: " + str(e)))
            tab.goto(BASE)
            boot(tab)
            try:
                tab.wait_for_function("() => document.querySelectorAll('.session-window.column').length === 3",
                                      timeout=8000)
            except Exception:
                pass
            check("R2 a new tab of the same browser shows the same columns",
                  ids(tab) == ["w-a", "w-b", "w-c"], str(ids(tab)))
            tab.close()

            # R3 -------------------------------------------------------------
            other = browser.new_context(viewport=dict(VIEW))
            other_page = other.new_page()
            other_page.goto(BASE)
            boot(other_page)
            other_page.wait_for_timeout(800)
            r3 = other_page.evaluate(
                "() => ({ columns: document.querySelectorAll('.session-window.column').length,"
                " max: [...document.querySelectorAll('.session-window.maximized')].map((w) => w._deskId) })"
            )
            check("R3 another browser profile shows no column", r3["columns"] == 0, str(r3))
            check("R3 …and only the desk's maximized console", r3["max"] == ["w-a"], str(r3))
            other.close()

            # R4 -------------------------------------------------------------
            page.evaluate("() => WBView.patch({ columns: ['w-a', 'w-gone', 'w-b'] })")
            reload(page)
            try:
                page.wait_for_function(f"() => {SH}.columns.flat().length === 2", timeout=8000)
            except Exception:
                pass
            check("R4 a stored id no longer on the desk is dropped", shell_cols(page) == ["w-a", "w-b"],
                  str(shell_cols(page)))
            check("R4 …and the store follows", stored(page) == ["w-a", "w-b"], str(stored(page)))

            # R5 -------------------------------------------------------------
            page.evaluate("() => WBView.patch({ columns: ['w-b', 'w-a'] })")
            reload(page)
            page.wait_for_timeout(1000)
            check("R5 a list whose first id is not maximized in the desk is ignored",
                  shell_cols(page) == [] and ids(page) == [], f"{shell_cols(page)} {ids(page)}")
            check("R5 …and cleared from the store", stored(page) is None, str(stored(page)))
            check("R5 w-a is still the maximized console",
                  page.evaluate("() => __W('w-a').classList.contains('maximized')"))

            # R6 -------------------------------------------------------------
            open_column(page, "w-a", "w-b")
            open_column(page, "w-b", "w-c")
            s = stored(page)
            check("R6 the store holds window ids only, equal to the list",
                  isinstance(s, list) and all(isinstance(x, str) for x in s) and s == shell_cols(page), str(s))
            page.wait_for_timeout(1500)
            raw = desk_raw()
            check("R6 nothing about columns reaches the desk", "column" not in json.dumps(raw),
                  json.dumps(raw)[:200])
            maxed = [w["id"] for w in raw["windows"] if w.get("max")]
            check("R6 only the leftmost is maximized in the desk", maxed == ["w-a"], str(maxed))

            # N1 -------------------------------------------------------------
            ws_w = page.evaluate("() => document.getElementById('workspace').clientWidth")
            page.set_viewport_size(dict(NARROW))
            page.wait_for_timeout(700)
            check("N1 a narrower viewport keeps every column", ids(page) == ["w-a", "w-b", "w-c"], str(ids(page)))
            page.set_viewport_size(dict(PHONE))
            page.wait_for_timeout(700)
            n1 = page.evaluate(
                "() => ({ columns: document.querySelectorAll('.session-window.column').length,"
                " max: __W('w-a').classList.contains('maximized') })"
            )
            check("N1 a phone width paints only the leftmost, as a maximize",
                  n1["columns"] == 0 and n1["max"], str(n1))
            check("N1 …and the list keeps all three", shell_cols(page) == ["w-a", "w-b", "w-c"],
                  str(shell_cols(page)))
            hid = page.evaluate(
                "() => { const r = __W('w-c').getBoundingClientRect();"
                " const at = document.elementFromPoint(r.left + r.width / 2, r.top + 10);"
                " return !at || !__W('w-c').contains(at); }"
            )
            check("N1 the hidden column is not painted over the maximize", hid)
            page.set_viewport_size(dict(VIEW))
            page.wait_for_timeout(700)
            check("N1 wider again: all three come back in order", ids(page) == ["w-a", "w-b", "w-c"],
                  str(ids(page)))
            check("N1 …and fill the width", equal_fill(page.evaluate("() => __columns()"), ws_w))

            # N2 -------------------------------------------------------------
            font = page.evaluate("() => WBConsole.fontSize()")
            page.evaluate("() => WBConsole.setFont(28)")
            page.wait_for_timeout(700)
            check("N2 a larger font keeps every column", ids(page) == ["w-a", "w-b", "w-c"], str(ids(page)))
            page.evaluate("(f) => WBConsole.setFont(f)", font)
            page.wait_for_timeout(700)

            # S1 -------------------------------------------------------------
            page.evaluate(f"() => {{ {SH}.openSettings(); {SH}.settingsSection = 'consoles'; }}")
            page.wait_for_timeout(300)
            field = page.locator(".set-row", has_text="Console text size").locator("input.set-num")
            check("S1 the field shows the size the consoles use", field.input_value() == str(font),
                  f"{field.input_value()} != {font}")
            field.fill("12")
            field.dispatch_event("change")
            page.wait_for_timeout(500)
            s1 = page.evaluate(
                "() => ({ store: WBView.read()?.font, sizes: ['w-a', 'w-b'].map((id) => __W(id)._term.term.options.fontSize) })"
            )
            check("S1 the Settings text size changes every console and is stored",
                  s1["store"] == 12 and s1["sizes"] == [12, 12], str(s1))
            field.fill(str(font))
            field.dispatch_event("change")
            page.evaluate(f"() => {SH}.closeSettings()")
            page.wait_for_timeout(300)

            # N3 -------------------------------------------------------------
            click_centre(page, "w-c")
            check("N3 setup: the rightmost column has the focus", page.evaluate("() => __focused()") == ["w-c"],
                  str(page.evaluate("() => __focused()")))
            page.set_viewport_size(dict(PHONE))
            page.wait_for_timeout(700)
            n3 = page.evaluate("() => ({ focused: __focused(), active: __active(), xterm: __inXterm() })")
            check("N3 the focus moves to the one painted console", n3["focused"] == ["w-a"], str(n3))
            check("N3 …and the keys go to its terminal", n3["active"] == "w-a" and n3["xterm"], str(n3))
            page.set_viewport_size(dict(VIEW))
            page.wait_for_timeout(700)

            # K1 -------------------------------------------------------------
            click_centre(page, "w-a")
            click_centre(page, "w-b")
            k1 = page.evaluate(
                "() => { const s = (id) => getComputedStyle(__W(id));"
                " return { focused: __focused(), style: s('w-b').outlineStyle, width: s('w-b').outlineWidth,"
                "   left: s('w-a').outlineStyle, right: s('w-c').outlineStyle, cols: __columns() }; }"
            )
            check("K1 a click gives the column the focus", k1["focused"] == ["w-b"], str(k1["focused"]))
            check("K1 the focused column carries a visible mark",
                  k1["style"] == "solid" and k1["width"] != "0px", f"{k1['style']} {k1['width']}")
            check("K1 its neighbours do not", k1["left"] == "none" and k1["right"] == "none",
                  f"{k1['left']} {k1['right']}")
            cols = k1["cols"]
            touch = abs(cols[0]["right"] - cols[1]["left"]) < 1 and abs(cols[1]["right"] - cols[2]["left"]) < 1
            check("K1 the columns touch", touch, str(cols))
            os.makedirs(os.path.dirname(SHOT), exist_ok=True)
            page.screenshot(path=SHOT)

            # K2 -------------------------------------------------------------
            # A click, as the operator does it: it moves the focus mark AND the
            # keyboard focus. `term.focus()` alone moves only the keyboard focus.
            click_centre(page, "w-a")
            check("K2 setup: the terminal of the first column is focused",
                  page.evaluate("() => __active() === 'w-a' && __inXterm()"))
            walk = []
            for key in ["ArrowRight", "ArrowRight", "ArrowRight", "ArrowLeft"]:
                page.keyboard.press(f"Alt+Shift+{key}")
                page.wait_for_timeout(250)
                walk.append(page.evaluate("() => ({ focused: __focused(), active: __active(), xterm: __inXterm() })"))
            got = [w["focused"] for w in walk]
            check("K2 → walks 1, 2, then wraps to 0; ← from 0 wraps to 2",
                  got == [["w-b"], ["w-c"], ["w-a"], ["w-c"]], str(walk))
            live_ok = all(w["active"] == w["focused"][0] and w["xterm"] for w in walk if w["focused"][0] in LIVE)
            check("K2 a live column's terminal takes the keys", live_ok, str(walk))
            screen = page.evaluate(
                "() => { const b = __W('w-a')._term.term.buffer.active; let t = '';"
                " for (let i = 0; i < b.length; i++) t += b.getLine(i)?.translateToString(true) + '\\n'; return t; }"
            )
            check("K2 the chord never reached the shell", "[1;4" not in screen and "\x1b" not in screen)

            # X1 -------------------------------------------------------------
            # The tab of R2 attached to the same sessions and may hold the writer
            # slot: this page is then parked, read-only, until it takes over.
            if page.evaluate("() => !!__W('w-b').querySelector('.session-parked')"):
                page.evaluate("() => __W('w-b').querySelector('[data-act=take-over]').click()")
                try:
                    page.wait_for_function("() => !__W('w-b').querySelector('.session-parked')", timeout=8000)
                except Exception:
                    pass
            check("X1 setup: this page drives w-b",
                  page.evaluate("() => !__W('w-b').querySelector('.session-parked')"))
            # MEASURED: keys typed at once after the click lost their first
            # characters (`exit` ran as `it`). Settle, clear the line, type slowly.
            click_centre(page, "w-b")
            page.wait_for_timeout(800)
            page.keyboard.press("Escape")
            page.keyboard.type("exit", delay=80)
            page.keyboard.press("Enter")
            try:
                page.wait_for_function("() => __W('w-b')?.classList.contains('ended')", timeout=15000)
                ended = True
            except Exception:
                ended = False
            tail = page.evaluate(
                "() => { const w = __W('w-b'); const b = w._term.term.buffer.active; const t = [];"
                " for (let i = 0; i < b.length; i++) { const l = b.getLine(i)?.translateToString(true);"
                " if (l) t.push(l); } return [w.className, __active(), t.slice(-4)]; }"
            )
            check("X1 the session in the column ends", ended, "" if ended else str(tail))
            check("X1 …and the window stays a column",
                  page.evaluate("() => __W('w-b')?.classList.contains('column')"))
            page.wait_for_timeout(6000)
            check("X1 six seconds later the list is unchanged", shell_cols(page) == ["w-a", "w-b", "w-c"],
                  str(shell_cols(page)))
            page.evaluate("() => __W('w-b').querySelector('.session-restart').click()")
            asked = confirm(page)
            check("X1 restart asks first", asked)
            try:
                page.wait_for_function(
                    "() => { const w = __W('w-b'); return w && !w.classList.contains('ended')"
                    " && w.classList.contains('column') && w._term?.term?.cols > 0; }",
                    timeout=10000,
                )
            except Exception:
                pass
            x1 = page.evaluate(
                "() => ({ ended: __W('w-b')?.classList.contains('ended'),"
                " column: __W('w-b')?.classList.contains('column'), ids: __columns().map((c) => c.id) })"
            )
            check("X1 restart starts it again in the same column",
                  x1["column"] and not x1["ended"] and x1["ids"] == ["w-a", "w-b", "w-c"], str(x1))

            # X2 -------------------------------------------------------------
            ctx_b = browser.new_context(viewport=dict(VIEW))
            page_b = ctx_b.new_page()
            page_b.goto(BASE)
            boot(page_b)
            page_b.evaluate("() => __W('w-c').querySelector('.session-close').click()")
            confirm(page_b)
            gone_b = poll_desk(lambda d: "w-c" not in d)
            check("X2 setup: the other client closed w-c on the desk", gone_b is not None and "w-c" not in gone_b)
            try:
                page.wait_for_function(f"() => !{SH}.columns.flat().includes('w-c')", timeout=8000)
            except Exception:
                pass
            page.wait_for_timeout(500)
            x2 = page.evaluate(f"() => ({{ list: {SH}.columns.flat(), win: !!__W('w-c'), cols: __columns() }})")
            check("X2 the closed console leaves the columns", x2["list"] == ["w-a", "w-b"], str(x2["list"]))
            check("X2 …and this stage", not x2["win"])
            check("X2 the others widen", equal_fill(x2["cols"], ws_w), str(x2["cols"]))
            ctx_b.close()
            page.wait_for_timeout(2500)
            check("X2 it does not come back", page.evaluate("() => !__W('w-c')") and "w-c" not in desk_windows())

            # X3 -------------------------------------------------------------
            open_column(page, "w-b", "w-f")
            check("X3 setup: w-f in a column", ids(page) == ["w-a", "w-b", "w-f"], str(ids(page)))
            # What the opener holds right after `detachFence` returns: a
            # microtask queued inside `window.open` runs after the synchronous
            # rest of `detachFence` and before the popup document can load.
            page.evaluate(
                "() => { const open = window.open; window.open = (...a) => { const h = open.apply(window, a);"
                " queueMicrotask(() => { window.__afterOpen = { cols: " + SH + ".columns.flat(),"
                "   onStage: !!__W('w-f'), popup: h ? h.location.href : null }; });"
                " window.open = open; return h; }; }"
            )
            with page.expect_popup(timeout=15000) as info:
                page.evaluate("() => WBConsole.detachFence('f-one')")
            popup = info.value
            popup.on("pageerror", lambda e: errors.append("popup: " + str(e)))
            popup.wait_for_load_state()
            try:
                popup.wait_for_function(
                    "() => [...document.querySelectorAll('.session-window')].some((w) => w._deskId === 'w-f')",
                    timeout=15000,
                )
            except Exception:
                pass
            popup.wait_for_timeout(500)
            after_open = page.evaluate("() => window.__afterOpen")
            check("X3 the columns let go before the popup loaded",
                  after_open and "w-f" not in after_open["cols"] and not after_open["onStage"]
                  and after_open["popup"] == "about:blank", str(after_open))
            opener_cols = popup.evaluate("() => window.__openerCols")
            check("X3 the popup never saw w-f in the opener's columns",
                  isinstance(opener_cols, list) and "w-f" not in opener_cols, str(opener_cols))
            check("X3 the fence's console is gone from this stage", page.evaluate("() => !__W('w-f')"))
            check("X3 the list follows", shell_cols(page) == ["w-a", "w-b"], str(shell_cols(page)))
            x3 = popup.evaluate(
                "() => ({ ids: [...document.querySelectorAll('.session-window')].map((w) => w._deskId),"
                " buttons: [...document.querySelectorAll('.session-column')].map((b) => b.hidden) })"
            )
            check("X3 the popup holds w-f", "w-f" in x3["ids"], str(x3))
            check("X3 the popup shows no column button", all(x3["buttons"]), str(x3))
            desk = poll_desk(lambda d: sum(1 for v in d.values() if v.get("max")) == 1)
            check("X3 the desk has exactly one maximized console",
                  desk and [k for k, v in desk.items() if v.get("max")] == ["w-a"],
                  str({k: v.get("max") for k, v in (desk or {}).items()}))
            popup.close()
            try:
                page.wait_for_function("() => !!__W('w-f')", timeout=10000)
            except Exception:
                pass
            page.wait_for_timeout(500)
            check("X3 re-attach brings w-f home, not as a column",
                  page.evaluate("() => !!__W('w-f') && !__W('w-f').classList.contains('column')"))
            check("X3 the columns are untouched by the re-attach", ids(page) == ["w-a", "w-b"], str(ids(page)))

            # X4 -------------------------------------------------------------
            raw = desk_raw()
            now = int(time.time() * 1000) + 60000
            for w in raw["windows"]:
                if w["id"] == "w-l":
                    w["max"] = True
                    w["ts"] = now
            http("PUT", "api/desk", {"windows": raw["windows"], "fences": raw.get("fences", [])})
            page.wait_for_timeout(4000)
            check("X4 a remote maximize leaves the list", shell_cols(page) == ["w-a", "w-b"], str(shell_cols(page)))
            on_top = page.evaluate(
                "() => __columns().every((c) => { const w = __W(c.id); const r = w.getBoundingClientRect();"
                " const at = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);"
                " return !!at && w.contains(at); })"
            )
            check("X4 the remote maximize stays behind the columns", on_top)

            # X5 -------------------------------------------------------------
            before = page.evaluate("() => __columns()")
            raw = desk_raw()
            for w in raw["windows"]:
                if w["id"] == "w-b":
                    w["rect"] = {"left": F_ONE["left"] + 50, "top": F_ONE["top"] + 60, "width": 300, "height": 200}
                    w["ts"] = now + 1000
            http("PUT", "api/desk", {"windows": raw["windows"], "fences": raw.get("fences", [])})
            page.wait_for_timeout(6000)
            check("X5 a remote rect or fence change leaves the list", shell_cols(page) == ["w-a", "w-b"],
                  str(shell_cols(page)))
            after = page.evaluate("() => __columns()")
            check("X5 …and the painted columns", [c["id"] for c in after] == [c["id"] for c in before]
                  and equal_fill(after, ws_w), f"{before} vs {after}")

            check("no page errors", not errors, str(errors[:3]))
            browser.close()
    finally:
        stop(proc)


if __name__ == "__main__":
    main()
    passed = sum(results)
    print(f"{passed}/{len(results)} passed", flush=True)
    if FLOOR is not None:
        check("check floor", len(results) == FLOOR, f"{len(results)} != {FLOOR}")
    sys.exit(0 if results and all(results) else 1)
