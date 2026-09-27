"""Columns (#472, ADR-0051 §5) browser acceptance: a maximized console opens
other consoles beside it as columns, lists them, and restores them.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7472.

The fixture `desk.toml` is written BEFORE the daemon starts:
  w-a  loose, live shell (`kind = "console"`), maximized
  w-b  loose, live shell
  w-c  loose, placeholder (`kind = "agent"`: full chrome, no PTY)
  w-f  placeholder inside fence f-one
  w-l  placeholder inside fence f-lock (`locked = true`)
  f-empty  a fence with no console

Scenario 1   the column button: hidden on a floating console, the FIRST title
             bar control of the maximized one
Scenario 2   the list: loose consoles, then fences in the Fence menu order; no
             empty fence, no maximized console; a placeholder says not running
Scenario 3   open w-b: two equal columns fill the viewport, the live terminal
             narrows
Scenario 4   from w-a open w-l (locked fence, not running): it lands right of
             w-a, w-b keeps its place; w-b's row now says it is already open
Scenario 5   at the cap the button stays enabled; a row that is not open cannot
             open a column but can be swapped in
Scenario 5s  swap: into a middle column, two columns change places (the desk
             follows the new leftmost), and back
Scenario 6   a column hides lock, full screen, close and the handles; restart
             stays and asks first
Scenario 7   restore the middle column: the others widen
Scenario 8   restore the leftmost: the next is the maximized console, and the
             desk says so
Scenario 9   restore the last column: an ordinary maximize; one more restore
             returns it to its rect
Scenario 10  no desk rect changed, and every console's restore rect is the one
             it had before the first column opened
Scenario 11  a phone-width viewport offers no column
Scenario 12  a larger font gives a smaller cap: on a 2000 px window, font 28
             hides the button and font 15 shows it
Scenario 13  at a cap of 1, Restore on the leftmost still restores it and the
             next column in the list takes the maximize
Scenario 14  a lone maximized console swaps for another

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_columns_472.py   (exit 0 = all pass)
"""

import json
import re
import os
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7472
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT = os.path.join(REPO_ROOT, "docs", "screenshots", "472-columns-2026-09-26.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
VIEW = {"width": 2400, "height": 1000}
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


def http(method, path):
    req = urllib.request.Request(BASE + path, method=method)
    with urllib.request.urlopen(req, timeout=5) as r:
        return r.status, r.read().decode()


def desk_windows():
    return {r["id"]: r for r in json.loads(http("GET", "api/desk")[1])["windows"]}


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
  window.__W = (id) => [...document.querySelectorAll('.session-window')].find((w) => w._deskId === id) || null;
  window.__colBtn = (id) => __W(id)?.querySelector('.session-column') || null;
  window.__visible = (el) => !!el && !el.hidden && getComputedStyle(el).display !== 'none' && el.offsetParent !== null;
  window.__columns = () => {
    const ws = document.getElementById('workspace').getBoundingClientRect();
    return [...document.querySelectorAll('.session-window.column')]
      .map((w) => { const r = w.getBoundingClientRect();
        return { id: w._deskId, left: r.left - ws.left, width: r.width,
                 max: w.classList.contains('maximized') }; })
      .sort((a, b) => a.left - b.left);
  };
}"""


def open_menu(page, from_id):
    page.evaluate("(id) => __colBtn(id).click()", from_id)
    page.wait_for_function(
        f"() => {SH}.columnMenu === true && document.querySelector('.column-menu').getClientRects().length > 0",
        timeout=5000,
    )
    page.wait_for_timeout(150)


def menu_state(page):
    return page.evaluate(
        "() => [...document.querySelectorAll('.column-menu .column-group')].map((g) => ({"
        "  head: __visible(g.querySelector('.column-group-name')) ? g.querySelector('.column-group-name').textContent.trim() : '',"
        "  repo: __visible(g.querySelector('.column-group-repo')) ? g.querySelector('.column-group-repo').textContent.trim() : '',"
        "  rows: [...g.querySelectorAll('.column-item')].map((b) => ({"
        "    id: b.dataset.id, text: b.querySelector('.row-name').textContent,"
        "    disabled: b.disabled, title: b.title,"
        "    off: __visible(b.querySelector('.row-off')),"
        "    hasState: !!b.querySelector('.session-state') })) }))"
    )


def open_column(page, from_id, id):
    open_menu(page, from_id)
    page.locator(f".column-menu .column-item[data-id='{id}']").click()
    page.wait_for_timeout(500)


def swap_in(page, from_id, id):
    open_menu(page, from_id)
    page.locator(f".column-menu .column-swap[data-id='{id}']").click()
    page.wait_for_timeout(500)


def column_ids(page):
    return [c["id"] for c in page.evaluate("() => __columns()")]


def close_menu(page):
    page.evaluate(f"() => {{ {SH}.columnMenu = false; }}")
    page.wait_for_timeout(100)


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


def restore_rects(page):
    return page.evaluate(
        "() => Object.fromEntries([...document.querySelectorAll('.session-window')]"
        "  .map((w) => [w._deskId, WBConsole.restoreRect(w)]))"
    )


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
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
            page.wait_for_function(
                "() => { const ws = [...document.querySelectorAll('.session-window')];"
                " return ws.length === 5 && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
                timeout=20000,
            )
            page.evaluate(HELPERS)

            # Probe: the two shells are live terminals and the rest placeholders.
            try:
                page.wait_for_function(
                    "() => ['w-a','w-b'].every((id) => __W(id)?._term?.term?.cols > 0)", timeout=15000
                )
            except Exception:
                pass
            probe = page.evaluate(
                "() => Object.fromEntries([...document.querySelectorAll('.session-window')].map((w) =>"
                "  [w._deskId, { term: !!w._term, placeholder: w.classList.contains('placeholder') }]))"
            )
            print("probe", probe, flush=True)
            ok = all(probe[w]["term"] for w in LIVE) and all(
                probe[w]["placeholder"] for w in probe if w not in LIVE
            )
            check("probe: shells are live, agent records are placeholders", ok, str(probe))
            if not ok:
                return
            page.wait_for_timeout(600)
            cap = page.evaluate(f"() => {SH}.columnCap('w-a')")
            print("cap at", VIEW, "=", cap, flush=True)
            check("the 2400 px fixture fits at least three columns", cap >= 3, f"cap={cap}")

            before_desk = desk_windows()
            before_rects = restore_rects(page)

            # 1 --------------------------------------------------------------
            s1 = page.evaluate(
                "() => ({ floating: __visible(__colBtn('w-b')), maxed: __visible(__colBtn('w-a')),"
                "  first: __W('w-a').querySelector('.session-actions').firstElementChild.className,"
                "  title: __colBtn('w-a').title, isMax: __W('w-a').classList.contains('maximized') })"
            )
            check("1 a floating console has no column button", not s1["floating"], str(s1))
            check("1 the maximized console shows it", s1["isMax"] and s1["maxed"], str(s1))
            check("1 it is the first title bar control", s1["first"] == "session-column", s1["first"])
            check("1 its title", s1["title"] == "Open in a column", s1["title"])

            # 2 --------------------------------------------------------------
            open_menu(page, "w-a")
            menu = menu_state(page)
            fence_order = page.evaluate("() => WBConsole.fenceList().map((f) => f.name)")
            heads = [g["head"] for g in menu]
            want_heads = [""] + [n for n in fence_order if n != "empty"]
            check("2 groups: no-fence rows without a name, then fences in the Fence menu order", heads == want_heads,
                  f"heads={heads} fences={fence_order}")
            check("2 the empty fence is left out", "empty" not in heads, str(heads))
            ids = [r["id"] for g in menu for r in g["rows"]]
            check("2 the maximized console is left out", "w-a" not in ids, str(ids))
            rows = {r["id"]: r for g in menu for r in g["rows"]}
            check("2 a placeholder row says not running", rows.get("w-c", {}).get("off") is True, str(rows.get("w-c")))
            check("2 a live row does not", rows.get("w-b", {}).get("off") is False, str(rows.get("w-b")))
            # #479: no group head prints a repo; each row is `<name> (<label>)`.
            check("2 no repo head; each row prints its console name and label",
                  not any(g["repo"] for g in menu) and
                  all(re.fullmatch(r".+ \((claude|console)\)", r["text"]) and r["hasState"] for r in rows.values()),
                  str([(g["head"], g["repo"], [r["text"] for r in g["rows"]]) for g in menu]))
            s2 = page.evaluate(
                "() => { const m = document.querySelector('.column-menu');"
                " return { lock: !!m.querySelector('.bi-lock-fill'), text: m.textContent,"
                " filter: __visible(m.querySelector('.column-filter')) }; }"
            )
            check("2 no lock, no list title, no 'Loose consoles'",
                  not s2["lock"] and "Open in a column" not in s2["text"] and "Loose" not in s2["text"], str(s2))
            check("2 a short list has no filter box", not s2["filter"], str(s2))
            close_menu(page)

            # 2b: a long list opens with a filter box, focused
            open_menu(page, "w-a")
            page.evaluate(
                f"() => {{ const sh = {SH}; const g = sh.columnGroups;"
                " sh.columnGroups = [...g, ...g.map((x) => ({ ...x, fence: x.fence ? { ...x.fence, id: x.fence.id + '-2' } : { id: 'copy', name: 'copy' },"
                "   rows: x.rows.map((r) => ({ ...r, id: r.id + '-2' })) }))]; }"
            )
            page.wait_for_timeout(100)
            check("2b a list of 8 or more rows shows the filter box",
                  page.evaluate("() => __visible(document.querySelector('.column-filter'))"))
            # Long rows (each with its own repo) and a short window: the list
            # must stay inside the window on the right and at the bottom.
            page.set_viewport_size({"width": VIEW["width"], "height": 360})
            close_menu(page)
            open_menu(page, "w-a")
            page.evaluate(
                f"() => {{ const sh = {SH}; const long = 'owner/' + 'a-very-long-repository-name-'.repeat(3);"
                " const rows = Array.from({ length: 12 }, (_, i) => ({ id: 'long-' + i, agent: 'claude', repo: long + i,"
                "   kind: 'agent', state: null, running: true, enabled: true, reason: null, swappable: true }));"
                " sh.columnGroups = [{ fence: null, rows, shared: false, repo: null }]; }"
            )
            page.wait_for_timeout(100)
            fit = page.evaluate("() => { const r = document.querySelector('.column-menu').getBoundingClientRect();"
                                " return { left: r.left, right: r.right, bottom: r.bottom, w: innerWidth, h: innerHeight }; }")
            check("2b long rows on a short window: the list stays inside the window",
                  fit["left"] >= 0 and fit["right"] <= fit["w"] and fit["bottom"] <= fit["h"], str(fit))
            close_menu(page)
            page.set_viewport_size(dict(VIEW))
            page.wait_for_timeout(300)
            open_menu(page, "w-a")
            page.evaluate(
                f"() => {{ const sh = {SH}; const g = sh.columnGroups;"
                " sh.columnGroups = [...g, ...g.map((x) => ({ ...x, fence: x.fence ? { ...x.fence, id: x.fence.id + '-2' } : { id: 'copy', name: 'copy' },"
                "   rows: x.rows.map((r) => ({ ...r, id: r.id + '-2' })) }))]; }"
            )
            page.wait_for_timeout(100)
            page.locator(".column-filter").fill("held")
            page.wait_for_timeout(100)
            kept = [g["head"] for g in menu_state(page)]
            check("2b the filter keeps the matching fence", kept == ["held", "held"], str(kept))
            page.locator(".column-filter").fill("zzz")
            page.wait_for_timeout(100)
            check("2b no match says so",
                  page.evaluate("() => __visible(document.querySelector('.column-empty'))"))
            page.locator(".column-filter").press("Escape")
            page.wait_for_timeout(100)
            check("2b Escape closes the list", page.evaluate(f"() => {SH}.columnMenu === false"))

            # 3 --------------------------------------------------------------
            cols_before = page.evaluate("() => __W('w-a')._term.term.cols")
            open_column(page, "w-a", "w-b")
            page.wait_for_timeout(400)
            ws_w = page.evaluate("() => document.getElementById('workspace').clientWidth")
            cols = page.evaluate("() => __columns()")
            check("3 two columns", [c["id"] for c in cols] == ["w-a", "w-b"], str(cols))
            check("3 equal widths that fill the viewport", equal_fill(cols, ws_w), f"ws={ws_w} {cols}")
            cols_after = page.evaluate("() => __W('w-a')._term.term.cols")
            check("3 the live terminal refits narrower", cols_after < cols_before, f"{cols_before} → {cols_after}")

            # 4 --------------------------------------------------------------
            # Checked below the cap: at the cap the button does not open the list.
            open_menu(page, "w-a")
            rows = {r["id"]: r for g in menu_state(page) for r in g["rows"]}
            b = rows.get("w-b", {})
            check("4 a console already in a column is disabled with a reason",
                  b.get("disabled") is True and b.get("title") == "Already in a column", str(b))
            page.locator(".column-menu .column-item[data-id='w-l']").click()
            page.wait_for_timeout(500)
            cols = page.evaluate("() => __columns()")
            check("4 opens right of the caller; the others keep order",
                  [c["id"] for c in cols] == ["w-a", "w-l", "w-b"], str([c["id"] for c in cols]))
            check("4 a placeholder became a column", page.evaluate(
                "() => __W('w-l').classList.contains('placeholder') && __W('w-l').classList.contains('column')"))
            check("4 still three equal widths", equal_fill(cols, ws_w), str(cols))

            # 5 --------------------------------------------------------------
            extra = ["w-c", "w-f"]
            while len(page.evaluate("() => __columns()")) < cap and extra:
                open_column(page, "w-b", extra.pop(0))
            n = len(page.evaluate("() => __columns()"))
            btn = page.evaluate("() => ({ disabled: __colBtn('w-a').disabled, disabledB: __colBtn('w-b').disabled })")
            check("5 open until the cap", n == cap, f"n={n} cap={cap}")
            check("5 at the cap the button stays enabled", not btn["disabled"] and not btn["disabledB"], str(btn))
            open_menu(page, "w-a")
            full = page.evaluate(
                "() => [...document.querySelectorAll('.column-menu .column-row')].map((r) => {"
                "  const item = r.querySelector('.column-item'), swap = r.querySelector('.column-swap');"
                "  return { id: item.dataset.id, disabled: item.disabled, title: item.title, swap: !swap.disabled }; })"
            )
            closed = [r for r in full if r["title"] != "Already in a column"]
            check("5 at the cap a row that is not open cannot open a column, and can be swapped in",
                  closed and all(r["disabled"] and r["title"] == "No room for another column" and r["swap"]
                                 for r in closed), str(full))
            close_menu(page)
            page.evaluate("(id) => { __W(id)._term?.term.focus(); }", "w-b")
            os.makedirs(os.path.dirname(SHOT), exist_ok=True)
            page.screenshot(path=SHOT)
            # Back to three columns for the scenarios below.
            while len(page.evaluate("() => __columns()")) > 3:
                last = page.evaluate("() => __columns().at(-1).id")
                press_max(page, last)

            # 5s: swap -------------------------------------------------------
            check("5s setup: three columns", column_ids(page) == ["w-a", "w-l", "w-b"], str(column_ids(page)))
            swap_in(page, "w-l", "w-c")
            check("5s swap puts the console in the column that asked",
                  column_ids(page) == ["w-a", "w-c", "w-b"], str(column_ids(page)))
            s5 = page.evaluate(
                "() => { const w = __W('w-l'); return { column: w.classList.contains('column'),"
                " max: w.classList.contains('maximized'), focus: WBConsole.focusedId() }; }"
            )
            check("5s the console swapped out is back on the plane", not s5["column"] and not s5["max"], str(s5))
            check("5s the focus is on the console that came in", s5["focus"] == "w-c", str(s5))
            swap_in(page, "w-b", "w-a")
            cols = page.evaluate("() => __columns()")
            check("5s two columns change places; the new leftmost is the maximized one",
                  [c["id"] for c in cols] == ["w-b", "w-c", "w-a"] and cols[0]["max"]
                  and not any(c["max"] for c in cols[1:]), str(cols))
            desk = poll_desk(lambda d: d["w-b"]["max"] is True and d["w-a"]["max"] is False)
            check("5s the desk records the new leftmost, and only it",
                  desk and desk["w-b"]["max"] is True and sum(1 for v in desk.values() if v["max"]) == 1,
                  str({k: v["max"] for k, v in (desk or {}).items()}))
            # Back to the order the scenarios below expect.
            swap_in(page, "w-b", "w-a")
            swap_in(page, "w-c", "w-l")
            check("5s swaps back", column_ids(page) == ["w-a", "w-l", "w-b"], str(column_ids(page)))
            poll_desk(lambda d: d["w-a"]["max"] is True)

            # 6 --------------------------------------------------------------
            chrome = page.evaluate(
                "() => { const w = __W('w-b'); const d = (s) => getComputedStyle(w.querySelector(s)).display;"
                "  return { lock: d('.session-lock'), full: d('.session-full'), close: d('.session-close'),"
                "    handle: d('.session-handle'), resize: d('.session-resize'),"
                "    restart: __visible(w.querySelector('.session-restart')),"
                "    title: __visible(w.querySelector('.session-title')) }; }"
            )
            check("6 lock, full screen, close and the handles are hidden in a column",
                  all(chrome[k] == "none" for k in ("lock", "full", "close", "handle", "resize")), str(chrome))
            check("6 restart and the title stay", chrome["restart"] and chrome["title"], str(chrome))
            page.evaluate("() => __W('w-b').querySelector('.session-restart').click()")
            try:
                page.wait_for_selector(".wb-confirm", timeout=3000)
                asked = True
            except Exception:
                asked = False
            check("6 restart asks first", asked)
            if asked:
                page.locator(".wb-confirm .modal-foot .btn:not(.danger):not(.accent)").click()
                page.wait_for_timeout(300)
            check("6 cancel leaves the columns", [c["id"] for c in page.evaluate("() => __columns()")] ==
                  ["w-a", "w-l", "w-b"])

            # 7 --------------------------------------------------------------
            press_max(page, "w-l")
            cols = page.evaluate("() => __columns()")
            check("7 restore the middle removes only it", [c["id"] for c in cols] == ["w-a", "w-b"], str(cols))
            check("7 the others widen", equal_fill(cols, ws_w), str(cols))
            check("7 the restored console floats again", page.evaluate(
                "() => !__W('w-l').classList.contains('column') && !__W('w-l').classList.contains('maximized')"))

            # 8 --------------------------------------------------------------
            open_column(page, "w-b", "w-c")
            check("8 setup: three columns", [c["id"] for c in page.evaluate("() => __columns()")] ==
                  ["w-a", "w-b", "w-c"])
            press_max(page, "w-a")
            cols = page.evaluate("() => __columns()")
            check("8 restore the leftmost: the next is the maximized console",
                  [c["id"] for c in cols] == ["w-b", "w-c"] and cols[0]["max"] and not cols[1]["max"], str(cols))
            desk = poll_desk(lambda d: d["w-b"]["max"] is True and d["w-a"]["max"] is False)
            check("8 the desk records the promoted console as maximized, the old leftmost not",
                  desk and desk["w-b"]["max"] is True and desk["w-a"]["max"] is False,
                  str({k: v["max"] for k, v in (desk or {}).items()}))
            check("8 only one maximized record",
                  desk and sum(1 for v in desk.values() if v["max"]) == 1)

            # 9 --------------------------------------------------------------
            press_max(page, "w-c")
            s9 = page.evaluate("() => ({ columns: document.querySelectorAll('.session-window.column').length,"
                               " bMax: __W('w-b').classList.contains('maximized'),"
                               " shell: " + SH + ".columns.length })")
            check("9 restore the last column: an ordinary maximized console",
                  s9["columns"] == 0 and s9["bMax"] and s9["shell"] == 0, str(s9))
            check("9 the survivor keeps the column button", page.evaluate("() => __visible(__colBtn('w-b'))"))
            on_top = page.evaluate(
                "() => { const r = __W('w-c').getBoundingClientRect();"
                " const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);"
                " return !!hit && hit.closest('.session-window') === __W('w-b'); }"
            )
            check("9 the maximized survivor is on top of the restored console", on_top)
            press_max(page, "w-b")
            back = page.evaluate(
                "() => { const w = __W('w-b'); return { max: w.classList.contains('maximized'),"
                " rect: { left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth, height: w.offsetHeight } }; }"
            )
            want_b = before_desk["w-b"]["rect"]
            check("9 one more restore returns it to its rect on the plane",
                  not back["max"] and back["rect"] == want_b, f"{back} want={want_b}")

            # 10 -------------------------------------------------------------
            after_desk = poll_desk(lambda d: all(d[k]["rect"] == before_desk[k]["rect"] for k in before_desk))
            same_desk = after_desk and all(after_desk[k]["rect"] == before_desk[k]["rect"] for k in before_desk)
            check("10 no column changed a desk rect", same_desk,
                  "" if same_desk else str({k: (before_desk[k]["rect"], (after_desk or {}).get(k, {}).get("rect"))
                                            for k in before_desk}))
            after_rects = restore_rects(page)
            check("10 every console's restore rect is the one it had", after_rects == before_rects,
                  "" if after_rects == before_rects else f"{before_rects} vs {after_rects}")
            on_disk = {w["id"]: w for w in tomllib.loads(
                Path(daemon_dir, "desk.toml").read_text(encoding="utf-8"))["windows"]}
            want_l = WINDOWS[4][2]
            check("10 the locked fence member's rect is on disk unchanged",
                  on_disk["w-l"]["rect"] == want_l, f"{on_disk['w-l']['rect']} want={want_l}")

            # 11 -------------------------------------------------------------
            page.set_viewport_size({"width": 390, "height": 844})
            page.wait_for_timeout(400)
            press_max(page, "w-a")
            page.wait_for_timeout(300)
            s11 = page.evaluate("() => ({ max: __W('w-a').classList.contains('maximized'),"
                                " shown: __visible(__colBtn('w-a')), hidden: __colBtn('w-a').hidden })")
            check("11 a phone-width viewport offers no column", s11["max"] and not s11["shown"] and s11["hidden"],
                  str(s11))
            press_max(page, "w-a")

            # 12 -------------------------------------------------------------
            # 2000 px of window leaves ~1650 px of viewport beside the sidebar.
            page.set_viewport_size({"width": 2000, "height": 1000})
            page.wait_for_timeout(400)
            press_max(page, "w-a")
            page.evaluate("() => WBConsole.setFont(28)")
            page.wait_for_timeout(500)
            s12a = page.evaluate("() => ({ shown: __visible(__colBtn('w-a')), cap: " + SH + ".columnCap('w-a'),"
                                 " m: WBConsole.columnMeasure('w-a') })")
            check("12 font 28: no column", not s12a["shown"], str(s12a))
            page.evaluate("() => WBConsole.setFont(15)")
            page.wait_for_timeout(500)
            s12b = page.evaluate("() => ({ shown: __visible(__colBtn('w-a')), cap: " + SH + ".columnCap('w-a'),"
                                 " m: WBConsole.columnMeasure('w-a') })")
            check("12 font 15 on the same viewport: the button is back", s12b["shown"], str(s12b))

            # 13 -------------------------------------------------------------
            open_column(page, "w-a", "w-b")
            page.evaluate("() => WBConsole.setFont(28)")
            page.wait_for_timeout(500)
            s13a = page.evaluate("() => ({ columns: document.querySelectorAll('.session-window.column').length,"
                                 " list: " + SH + ".columns.slice() })")
            check("13 at a cap of 1 only the leftmost is painted; the list is kept",
                  s13a["columns"] == 0 and s13a["list"] == ["w-a", "w-b"], str(s13a))
            press_max(page, "w-a")
            s13b = page.evaluate("() => ({ a: __W('w-a').classList.contains('maximized'),"
                                 " b: __W('w-b').classList.contains('maximized'), list: " + SH + ".columns.length })")
            check("13 Restore on that leftmost restores it, and the next takes the maximize",
                  not s13b["a"] and s13b["b"] and s13b["list"] == 0, str(s13b))
            desk = poll_desk(lambda d: d["w-a"]["max"] is False and d["w-b"]["max"] is True)
            check("13 the desk agrees", desk and desk["w-a"]["max"] is False and desk["w-b"]["max"] is True,
                  str({k: v["max"] for k, v in (desk or {}).items()}))
            page.evaluate("() => WBConsole.setFont(15)")
            page.wait_for_timeout(500)

            # 14: a lone maximized console swaps too ------------------------
            swap_in(page, "w-b", "w-c")
            s14 = page.evaluate("() => ({ b: __W('w-b').classList.contains('maximized'),"
                                " c: __W('w-c').classList.contains('maximized'),"
                                " columns: document.querySelectorAll('.session-window.column').length })")
            check("14 a lone maximize swaps: the new console is maximized, the old one is back",
                  s14["c"] and not s14["b"] and s14["columns"] == 0, str(s14))
            desk = poll_desk(lambda d: d["w-c"]["max"] is True and d["w-b"]["max"] is False)
            check("14 the desk agrees", desk and desk["w-c"]["max"] is True and desk["w-b"]["max"] is False,
                  str({k: v["max"] for k, v in (desk or {}).items()}))

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
