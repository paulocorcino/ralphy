"""A fence opened as columns (ADR-0051 §5, 2026-09-30) browser acceptance: one
button in the fence title bar opens every console of the fence as columns, in
a grid that follows their places on the stage.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7473.

The fixture `desk.toml` is written BEFORE the daemon starts:
  f-grid   a fence holding four placeholders:
             w-a above w-b (one column, two rows), w-c to the right, and w-d
             overlapping w-c by a quarter of its width (its own column)
  f-empty  a fence with no console
  w-x      loose, live shell, maximized
  w-y      loose, live shell

Scenario 1  the button sits between tile and lock; on the empty fence it is
            disabled
Scenario 2  w-x opens w-y as a column; the fence button REPLACES those columns
            with the fence's grid [[w-a, w-b], [w-c], [w-d]]
Scenario 3  the painted grid: w-a is the maximized console, w-a and w-b share a
            column, three columns of equal width; the desk moves `max` to w-a
Scenario 4  each console is restored on its own, and every console is back at
            its stage rect; no desk rect changed; then a real mouse click on
            the button opens the grid again
Scenario 5  a phone-width viewport hides the button, and a wide one shows it
Scenario 6  a detached fence hides the button, and a re-attach shows it

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/fence/wb_fence_columns.py   (exit 0 = all pass)
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
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "tmp")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
VIEW = {"width": 2400, "height": 1100}
FLOOR = 17  # every check above the floor check; pinned after the first green run

F_GRID = {"left": 40, "top": 40, "width": 1300, "height": 800}
F_EMPTY = {"left": 40, "top": 1000, "width": 400, "height": 300}
WINDOWS = [
    # id, kind, rect, max
    ("w-a", "agent", {"left": 80, "top": 100, "width": 400, "height": 300}, False),
    ("w-b", "agent", {"left": 100, "top": 440, "width": 400, "height": 300}, False),
    ("w-c", "agent", {"left": 540, "top": 100, "width": 400, "height": 300}, False),
    ("w-d", "agent", {"left": 840, "top": 440, "width": 400, "height": 300}, False),
    ("w-x", "console", {"left": 1500, "top": 100, "width": 500, "height": 300}, True),
    ("w-y", "console", {"left": 1500, "top": 500, "width": 500, "height": 300}, False),
]
GRID = [["w-a", "w-b"], ["w-c"], ["w-d"]]

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
    empty = tempfile.mkdtemp(prefix="wbfcol_empty_")
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
    d = tempfile.mkdtemp(prefix="wbfcol_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe fence columns fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbfcol@example.com"],
        ["git", "config", "user.name", "wbfcol"],
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
    for ts, (fid, name, rect) in enumerate([("f-grid", "grid", F_GRID), ("f-empty", "empty", F_EMPTY)], start=200):
        out += "[[fences]]\n" f'id = "{fid}"\n' f'name = "{name}"\n' f"ts = {ts}\n" f"{rect_toml(rect)}\n\n"
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


HELPERS = """() => {
  window.__W = (id) => [...document.querySelectorAll('.session-window')].find((w) => w._deskId === id) || null;
  window.__F = (id) => document.querySelector(`[data-fence-id='${id}']`);
  window.__btn = (id) => __F(id)?.querySelector('.fence-columns') || null;
  window.__visible = (el) => !!el && !el.hidden && getComputedStyle(el).display !== 'none' && el.offsetParent !== null;
  window.__painted = () => {
    const ws = document.getElementById('workspace').getBoundingClientRect();
    return Object.fromEntries([...document.querySelectorAll('.session-window')].map((w) => {
      const r = w.getBoundingClientRect();
      return [w._deskId, { column: w.classList.contains('column'), max: w.classList.contains('maximized'),
        col: w.style.getPropertyValue('--col-index'), row: w.style.getPropertyValue('--row-index'),
        left: Math.round(r.left - ws.left), top: Math.round(r.top - ws.top), width: Math.round(r.width) }];
    }));
  };
}"""


def restore_rects(page):
    return page.evaluate(
        "() => Object.fromEntries([...document.querySelectorAll('.session-window')]"
        "  .map((w) => [w._deskId, WBConsole.restoreRect(w)]))"
    )


def main():
    build()
    os.makedirs(SHOT_DIR, exist_ok=True)
    daemon_dir = tempfile.mkdtemp(prefix="wbfcol_daemon_")
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
                f" return ws.length === {len(WINDOWS)} && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }}",
                timeout=20000,
            )
            page.evaluate(HELPERS)
            page.wait_for_timeout(500)
            before_rects = restore_rects(page)
            before_desk = desk_windows()

            # 1 --------------------------------------------------------------
            geom = page.evaluate(
                "() => { const f = __F('f-grid'); const q = (c) => f.querySelector(c).getBoundingClientRect();"
                " const b = q('.fence-columns');"
                " return { tileR: q('.fence-arrange').right, lockL: q('.fence-lock').left,"
                "   left: b.left, right: b.right, w: b.width,"
                "   title: __btn('f-grid').title,"
                "   disabled: __btn('f-grid').disabled, emptyDisabled: __btn('f-empty').disabled }; }"
            )
            check("1 the button sits between tile and lock",
                  geom["w"] > 0 and geom["tileR"] <= geom["left"] + 1 and geom["right"] <= geom["lockL"] + 1,
                  str(geom))
            check("1 it is enabled on a fence with consoles, disabled on an empty one",
                  not geom["disabled"] and geom["emptyDisabled"], str(geom))

            # 2 --------------------------------------------------------------
            page.evaluate(f"() => {{ {SH}.columnFrom = 'w-x'; {SH}.openColumn('w-y'); }}")
            page.wait_for_timeout(500)
            s2a = page.evaluate(f"() => {SH}.columns")
            check("2 the loose consoles are open as columns first", s2a == [["w-x"], ["w-y"]], str(s2a))
            page.screenshot(path=os.path.join(SHOT_DIR, "fence-columns-before.png"))
            page.evaluate("() => { WBConsole.jumpToFence?.('f-grid'); }")
            page.wait_for_timeout(300)
            # The columns cover the fence; the operator restores them first in
            # real use, but the button must still act when it is reached.
            page.evaluate("() => __btn('f-grid').click()")
            page.wait_for_timeout(700)
            s2 = page.evaluate(f"() => {SH}.columns")
            check("2 the fence button replaces them with the fence's grid", s2 == GRID, str(s2))
            page.screenshot(path=os.path.join(SHOT_DIR, "fence-columns-after.png"))

            # 3 --------------------------------------------------------------
            painted = page.evaluate("() => __painted()")
            want = {"w-a": ("0", "0"), "w-b": ("0", "1"), "w-c": ("1", "0"), "w-d": ("2", "0")}
            got = {k: (painted[k]["col"], painted[k]["row"]) for k in want}
            check("3 each console is in its column and row", got == want and all(painted[k]["column"] for k in want),
                  str(got))
            check("3 w-a is the maximized console, and only it",
                  [k for k, v in painted.items() if v["max"]] == ["w-a"],
                  str({k: v["max"] for k, v in painted.items()}))
            check("3 the loose consoles left the columns",
                  not painted["w-x"]["column"] and not painted["w-y"]["column"], str(painted["w-x"]))
            widths = {painted[k]["width"] for k in ("w-a", "w-c", "w-d")}
            check("3 w-a and w-b share a column; three columns of equal width",
                  painted["w-a"]["left"] == painted["w-b"]["left"] and max(widths) - min(widths) <= 1
                  and painted["w-a"]["left"] < painted["w-c"]["left"] < painted["w-d"]["left"],
                  str({k: (painted[k]["left"], painted[k]["width"]) for k in want}))
            desk = poll_desk(lambda d: d["w-a"]["max"] is True and d["w-x"]["max"] is False)
            check("3 the desk moves the maximize to w-a",
                  desk and desk["w-a"]["max"] is True and desk["w-x"]["max"] is False,
                  str({k: v["max"] for k, v in (desk or {}).items()}))

            # 4 --------------------------------------------------------------
            left = []
            for wid in ("w-d", "w-b", "w-a", "w-c"):
                page.evaluate("(id) => __W(id).querySelector('.session-max').click()", wid)
                page.wait_for_timeout(400)
                left.append(page.evaluate(f"() => {SH}.columns.flat().length"))
            s4 = page.evaluate(
                "() => [...document.querySelectorAll('.session-window')]"
                "  .filter((w) => w.classList.contains('column') || w.classList.contains('maximized')).map((w) => w._deskId)"
            )
            check("4 one restore per console, until none is left", s4 == [] and left == [3, 2, 0, 0],
                  f"left={left} still={s4}")
            after_rects = restore_rects(page)
            check("4 every console is back at its stage rect", after_rects == before_rects,
                  "" if after_rects == before_rects else f"{before_rects} vs {after_rects}")
            after_desk = poll_desk(lambda d: all(d[k]["rect"] == before_desk[k]["rect"] for k in before_desk))
            same = after_desk and all(after_desk[k]["rect"] == before_desk[k]["rect"] for k in before_desk)
            check("4 no desk rect changed", same)

            # 4b: with nothing maximized the fence is in view, and a real mouse
            # click on the button opens the grid again.
            page.evaluate("() => __F('f-grid').scrollIntoView({ block: 'nearest', inline: 'nearest' })")
            page.wait_for_timeout(300)
            box = page.evaluate(
                "() => { const b = __btn('f-grid').getBoundingClientRect();"
                " const x = b.left + b.width / 2, y = b.top + b.height / 2;"
                " return { x, y, hit: !!document.elementFromPoint(x, y)?.closest('.fence-columns') }; }"
            )
            check("4b a click reaches the button (elementFromPoint)", box["hit"], str(box))
            page.mouse.click(box["x"], box["y"])
            page.wait_for_timeout(700)
            s4b = page.evaluate(f"() => {SH}.columns")
            check("4b a real click opens the fence's grid", s4b == GRID, str(s4b))

            # 5 --------------------------------------------------------------
            page.set_viewport_size({"width": 390, "height": 844})
            page.wait_for_timeout(500)
            narrow = page.evaluate("() => __btn('f-grid').hidden")
            page.set_viewport_size(dict(VIEW))
            page.wait_for_timeout(500)
            wide = page.evaluate("() => __btn('f-grid').hidden")
            check("5 a phone width hides the button, a wide one shows it", narrow and not wide,
                  f"narrow={narrow} wide={wide}")

            # 6 --------------------------------------------------------------
            with ctx.expect_page() as popup_info:
                page.evaluate("() => __F('f-grid').querySelector('.fence-detach').click()")
            popup = popup_info.value
            popup.wait_for_load_state()
            page.wait_for_timeout(500)
            away = page.evaluate("() => __btn('f-grid').hidden")
            page.evaluate("() => WBConsole.reattachFence('f-grid', { force: true })")
            page.wait_for_timeout(800)
            home = page.evaluate("() => __btn('f-grid').hidden")
            check("6 a detached fence hides the button, and a re-attach shows it", away and not home,
                  f"detached={away} home={home}")

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
