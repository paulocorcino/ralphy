"""Console screen measure harness (#689): writes the facts as JSON.

This is a measure, not a pass/fail check. It drives a REAL daemon on a scratch
`RALPHY_DAEMON_DIR` (the operator's own desk and login policy are untouched)
with `RALPHY_DAEMON_AGENT_OVERRIDE` set to `session_test_child`, and records
what a console screen looks like in six situations. PORT 7530. Three plain
consoles float at 680 px each in a 2400 px viewport.

A  maximize swap: console 1 is maximized, then the maximize moves between
   console 1 and console 2 five times. A move is one task, as a swap in the
   columns is: the maximized console is restored, then the other one is
   maximized. `floating_sizes` counts the PTY sizes
   (the child prints `SIZE <cols>x<rows>`) that are smaller than the maximized
   size, printed after the first maximize.
B  slice columns: the same with columns. `columns_floating_sizes` counts the
   sizes smaller than the width of a column.
F  uncover: console 1 is maximized and console 2, under it, gets a larger
   font and asks for a fit, as the font setting does. `covered_sizes` counts the sizes console 2 printed while it
   was covered; `uncover_sizes` counts the sizes it printed when console 1
   was restored and the cover went.
C  reconnect of the same terminal after `flood 600` (the page reattaches a
   socket and keeps its xterm): `reconnect`. Then a reconnect while the child
   writes `flood 300`: `reconnect_busy`. The wake (D) compares with the screen
   after it.
D  wake: console 1 is covered until it sleeps, then uncovered, so it gets a
   NEW terminal: `wake` (and `screen_equal_after_wake`).
E  reload: the page loads again and reattaches: `reattach`.

Each of `reconnect`, `wake` and `reattach` holds `replay_bytes`, `first_frame_ms`
(from the creation of the socket to its first terminal frame), `screen_equal`
(the visible text after is the visible text before), `open` (the `session-open`
payload of that socket), `since` (the byte count the socket sent),
`away_bytes` (`open.offset` minus `since`: the bytes the terminal did not
have) and `traffic` (every field of the daemon's `console socket traffic`
line for that socket).

`wake_gzip_bytes` is the gzip replay frame of the wake: the compressed
snapshot (null when the wake's replay was not gzip).

Arguments: `--label <name>` names the output
`docs/plans/logs/689-measure-<label>.json` (default `latest`); `--assert <name>`
(repeatable) names a fact that must hold. `size` always holds: no console
sent its floating size while another one covered it (`floating_sizes == 0`,
`columns_floating_sizes == 0`, `covered_sizes == 0`, `uncover_sizes <= 1`).
`snapshot` always holds too: the screen after the replay is the screen before
it (`screen_equal`) for `reconnect`, `wake` and `reattach`. `resume` always
holds: both reconnects resume (`open.resume`) with a replay of exactly the
bytes the terminal did not have, and `wake` and `reattach`, which start a
new terminal, do not resume.
Exit 0: everything was measured. Exit 1: an asserted fact failed. Exit 2: a
scenario could not run.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/console/wb_console_screen_689.py --label baseline
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7530
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
LOGS = os.path.join(REPO_ROOT, "docs", "plans", "logs")
VIEW = {"width": 2400, "height": 1000}
# `dormantAfterMs()` in wb-console-gpu.ts: 15 s on a fast link, as loopback is.
DORMANT_AFTER_S = 15
MOVES = 5
FLOOD_KIB = 600
BUSY_KIB = 300
# A terminal frame is a tag and an 8-byte session id before the bytes.
FRAME_HEADER = 9
IDS = ["w-1", "w-2", "w-3"]
# Names `--assert` accepts. Later stages add their own.
ASSERTS: dict = {
    "size": lambda f: f.get("floating_sizes") == 0
    and f.get("columns_floating_sizes") == 0
    and f.get("covered_sizes") == 0
    and (f.get("uncover_sizes") or 0) <= 1,
    "snapshot": lambda f: all(
        (f.get(name) or {}).get("screen_equal") is True for name in ("reconnect", "wake", "reattach")
    ),
    "resume": lambda f: all(resumed(f.get(name)) for name in ("reconnect", "reconnect_busy"))
    and all(((f.get(name) or {}).get("open") or {}).get("resume") is not True for name in ("wake", "reattach")),
}
# The asserts every run checks.
DEFAULT_ASSERTS = ["size", "snapshot", "resume"]


def resumed(fact):
    """The socket resumed, and its replay was exactly the bytes it did not have."""
    if not fact or (fact.get("open") or {}).get("resume") is not True or fact.get("away_bytes") is None:
        return False
    raw = (fact.get("traffic") or {}).get("replay_raw_bytes")
    if raw is None:
        return False
    tail = raw - FRAME_HEADER if raw > 0 else 0
    return tail == fact["away_bytes"]

HELPERS = """() => {
  window.__W = (id) => [...document.querySelectorAll('#stage .session-window')].find((w) => w._deskId === id) || null;
}"""
BUFFER = """(id) => { const b = __W(id)?._term?.term?.buffer.active; if (!b) return null;
  let out = ''; for (let y = 0; y < b.length; y++) out += (b.getLine(y)?.translateToString(true) ?? '') + '\\n';
  return out; }"""
VISIBLE = """(id) => { const t = __W(id)?._term?.term; if (!t) return null;
  const b = t.buffer.active; const lines = [];
  for (let y = b.viewportY; y < b.viewportY + t.rows; y++)
    lines.push((b.getLine(y)?.translateToString(true) ?? '').trimEnd());
  return lines.join('\\n'); }"""
STATE = """(id) => { const w = __W(id); return !w ? null : {
  maximized: w.classList.contains('maximized'), column: w.classList.contains('column'),
  dormant: w.classList.contains('dormant'), term: !!w._term, cols: w._term?.term?.cols ?? 0,
  session: w._term?.sessionId ?? w._dormantSession ?? null }; }"""

facts: dict = {}
errors: list = []
sockets: list = []  # every /ws/session socket the page opened, in order


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


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    # The UI assets are `include_dir!`-embedded: rebuild or the browser loads an old bundle.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True)


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb689_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb689@example.com"],
        ["git", "config", "user.name", "wb689"],
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


def write_desk(daemon_dir, slug):
    out = ""
    for ts, (wid, left) in enumerate(zip(IDS, (40, 760, 1480)), start=100):
        out += (
            "[[windows]]\n"
            f'id = "{wid}"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            f'consoleName = "{wid}"\n'
            f"ts = {ts}\n"
            f"rect = {{ left = {left}, top = 40, width = 680, height = 500 }}\n\n"
        )
    Path(daemon_dir, "desk.toml").write_bytes(out.encode("utf-8"))


def launch(daemon_dir, log_path):
    empty = tempfile.mkdtemp(prefix="wb689_empty_")
    env = dict(
        os.environ,
        RUST_LOG="info",
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_DAEMON_AGENT_OVERRIDE=CHILD,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)], env=env, stdout=subprocess.DEVNULL, stderr=open(log_path, "wb")
    )


ANSI = re.compile(r"\x1b\[[0-9;]*m")
FIELD = re.compile(r'(\w+)=("[^"]*"|\S+)')


def parse_value(raw):
    raw = raw.strip('"')
    if raw == "None":
        return None
    return int(raw) if re.fullmatch(r"-?\d+", raw) else raw


def traffic_lines(log_path):
    text = ANSI.sub("", Path(log_path).read_text(encoding="utf-8", errors="replace"))
    out = []
    for line in text.splitlines():
        if "console socket traffic" in line:
            out.append({k: parse_value(v) for k, v in FIELD.findall(line)})
    return out


def on_websocket(ws):
    if "/ws/session" not in ws.url:
        return
    m = re.search(r"[?&]id=(\d+)", ws.url)
    since = re.search(r"[?&]since=(\d+)", ws.url)
    rec = {
        "n": len(sockets),
        "session": int(m.group(1)) if m else None,
        "since": int(since.group(1)) if since else None,
        "t0": time.time(),
        "first_frame_ms": None,
        "first_frame_bytes": None,
        "open": None,
    }
    sockets.append(rec)

    def frame(payload):
        data = payload if isinstance(payload, (bytes, bytearray)) else str(payload).encode("latin-1", "replace")
        if not data:
            return
        if data[0] == 0x01 and rec["first_frame_ms"] is None:
            rec["first_frame_ms"] = round((time.time() - rec["t0"]) * 1000, 1)
            rec["first_frame_bytes"] = len(data) - 9
        elif data[0] == 0x02 and rec["open"] is None:
            try:
                msg = json.loads(bytes(data[1:]).decode("utf-8"))
            except Exception:
                return
            if msg.get("verb") == "session-open":
                rec["open"] = msg.get("payload")

    ws.on("framereceived", frame)


def wait_for(page, js, timeout_s, arg=None):
    try:
        page.wait_for_function(js, arg=arg, timeout=int(timeout_s * 1000))
        return True
    except Exception:
        return False


def boot(page):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate("() => Alpine.store('tabs').activate('consoles')")
    page.wait_for_function(
        "() => { const ws = [...document.querySelectorAll('#stage .session-window')];"
        " return ws.length === 3 && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        timeout=20000,
    )
    page.evaluate(HELPERS)
    ok = wait_for(page, "() => ['w-1','w-2','w-3'].every((id) => __W(id)?._term?.term?.cols > 0)", 20)
    page.wait_for_timeout(600)
    return ok


def press_max(page, wid, wait_ms=1000):
    page.evaluate("(id) => __W(id).querySelector('.session-max').click()", wid)
    page.wait_for_timeout(wait_ms)


def open_column(page, from_id, wid):
    page.evaluate("(id) => __W(id).querySelector('.session-column').click()", from_id)
    wait_for(page, "() => document.querySelector('.column-menu')?.getClientRects().length > 0", 5)
    page.wait_for_timeout(150)
    page.locator(f".column-menu .column-item[data-id='{wid}']").click()
    page.wait_for_timeout(1000)


def restore_all(page):
    """Back to three floating consoles."""
    for _ in range(4):
        held = [w for w in IDS if (s := page.evaluate(STATE, w)) and (s["maximized"] or s["column"])]
        if not held:
            return True
        press_max(page, held[0], 700)
    return False


def size_lines(page, wid):
    text = page.evaluate(BUFFER, wid) or ""
    return [(int(c), int(r)) for c, r in re.findall(r"SIZE (\d+)x(\d+)", text)]


def sizes_now(page):
    return {w: size_lines(page, w) for w in IDS}


def floating_after(before, after, limit):
    """SIZE lines printed after `before` (a count per console) with cols under `limit`."""
    out = []
    for w in IDS:
        out += [s for s in after[w][len(before[w]) :] if s[0] < limit]
    return out


def scenario_a(page):
    before = sizes_now(page)
    press_max(page, "w-1")
    for i in range(MOVES):
        old, new = ("w-1", "w-2") if i % 2 == 0 else ("w-2", "w-1")
        page.evaluate(
            "([a, b]) => { __W(a).querySelector('.session-max').click(); __W(b).querySelector('.session-max').click(); }",
            [old, new],
        )
        page.wait_for_timeout(1000)
    after = sizes_now(page)
    maximized_cols = max((s[0] for w in IDS for s in after[w][len(before[w]) :]), default=0)
    floating = floating_after(before, after, maximized_cols - 2)
    facts["sizes"] = {w: [f"{c}x{r}" for c, r in after[w]] for w in IDS}
    facts["maximized_cols"] = maximized_cols
    facts["floating_sizes"] = len(floating)
    restore_all(page)


def scenario_b(page):
    press_max(page, "w-1")
    open_column(page, "w-1", "w-2")
    before = sizes_now(page)
    column_cols = max((s[0] for w in ("w-1", "w-2") for s in before[w][-1:]), default=0)
    # Each round leaves the columns with one console: w-2 goes back to its
    # floating rect under the full bleed of w-1, and then comes back.
    for _ in range(MOVES):
        press_max(page, "w-2")
        open_column(page, "w-1", "w-2")
    after = sizes_now(page)
    floating = floating_after(before, after, column_cols - 2)
    facts["column_cols"] = column_cols
    facts["columns_sizes"] = {w: [f"{c}x{r}" for c, r in after[w]] for w in IDS}
    facts["columns_floating_sizes"] = len(floating)
    restore_all(page)


def scenario_f(page):
    press_max(page, "w-1")
    before = sizes_now(page)
    font = page.evaluate("() => { const t = __W('w-2')._term; const f = t.term.options.fontSize;"
                         " t.term.options.fontSize = f + 4; t.fit.fit(); return f; }")
    page.wait_for_timeout(1000)
    covered = sizes_now(page)
    press_max(page, "w-1")
    after = sizes_now(page)
    facts["covered_sizes"] = len(covered["w-2"]) - len(before["w-2"])
    facts["uncover_sizes"] = len(after["w-2"]) - len(covered["w-2"])
    facts["uncover_trace"] = {w: [f"{c}x{r}" for c, r in after[w][len(before[w]) :]] for w in IDS}
    page.evaluate("(f) => { const t = __W('w-2')._term; t.term.options.fontSize = f; t.fit.fit(); }", font)
    page.wait_for_timeout(600)


def type_line(page, wid, text):
    page.evaluate("(id) => __W(id).querySelector('.xterm-helper-textarea').focus()", wid)
    page.keyboard.type(text)
    page.keyboard.press("Enter")


def session_of(page, wid):
    return (page.evaluate(STATE, wid) or {}).get("session")


def sockets_of(session):
    return [s for s in sockets if s["session"] == session]


def new_socket(page, session, known, timeout=10):
    deadline = time.time() + timeout
    while time.time() < deadline:
        now = sockets_of(session)
        if len(now) > known:
            return now[known]
        # Playwright delivers socket events only while the page is being waited on.
        page.wait_for_timeout(100)
    return None


def wait_flood(page, wid, timeout=20):
    return wait_for(
        page,
        "(id) => { const t = __W(id)?._term?.term; if (!t) return false; const b = t.buffer.active;"
        " for (let y = b.viewportY; y < b.viewportY + t.rows; y++)"
        " if ((b.getLine(y)?.translateToString(true) ?? '').includes('FLOOD-DONE')) return true; return false; }",
        timeout,
        arg=wid,
    )


def settle(page, wid):
    """The same text twice, 600 ms apart: the replay is written."""
    last = None
    for _ in range(20):
        now = page.evaluate(VISIBLE, wid)
        if now is not None and now == last:
            return now
        last = now
        page.wait_for_timeout(600)
    return last


def socket_fact(sock):
    opened = sock["open"] if sock else None
    since = sock["since"] if sock else None
    offset = (opened or {}).get("offset")
    return {
        "replay_bytes": sock["first_frame_bytes"] if sock else None,
        "first_frame_ms": sock["first_frame_ms"] if sock else None,
        "open": opened,
        "since": since,
        "away_bytes": offset - since if isinstance(offset, int) and since is not None else None,
        "traffic": None,
        "_socket": sock["n"] if sock else None,
    }


def record(page, name, session, known, before):
    sock = new_socket(page, session, known)
    wait_flood(page, "w-1")
    after = settle(page, "w-1")
    facts[name] = {**socket_fact(sock), "screen_equal": after == before}
    return after



def resume_while_busy(page, session):
    """A reconnect while the child writes: the replay is the bytes written
    while the terminal had no socket. The wake (D) then checks that the
    terminal shows the daemon's screen: no byte twice, none missing."""
    # `RESUME_DEBOUNCE_MS` in wb-resume.ts is 1.5 s since the last resume.
    page.wait_for_timeout(1600)
    # No wait: the command goes on the old socket, and the child writes while
    # the terminal reconnects.
    type_line(page, "w-1", f"flood {BUSY_KIB}")
    known = len(sockets_of(session))
    if not page.evaluate("() => __W('w-1')._term.resume(true)"):
        raise RuntimeError("the terminal did not reconnect while busy")
    sock = new_socket(page, session, known)
    done = wait_for(
        page,
        "(id) => { const t = __W(id)?._term?.term; if (!t) return false; const b = t.buffer.active;"
        " for (let y = b.viewportY; y < b.viewportY + t.rows; y++)"
        " if (/FLOOD-DONE 3\\d{5}/.test(b.getLine(y)?.translateToString(true) ?? '')) return true; return false; }",
        60,
        arg="w-1",
    )
    if not done:
        raise RuntimeError("the busy FLOOD-DONE did not appear")
    facts["reconnect_busy"] = socket_fact(sock)
    return settle(page, "w-1")


def scenario_c(page):
    type_line(page, "w-1", f"flood {FLOOD_KIB}")
    if not wait_flood(page, "w-1", 60):
        raise RuntimeError("FLOOD-DONE did not appear")
    before = settle(page, "w-1")
    session = session_of(page, "w-1")
    known = len(sockets_of(session))
    if not page.evaluate("() => __W('w-1')._term.resume(true)"):
        raise RuntimeError("the terminal did not reconnect")
    record(page, "reconnect", session, known, before)
    facts["resume"] = (facts["reconnect"]["open"] or {}).get("resume")
    # The screen the wake (D) must show is the one after the busy reconnect.
    return resume_while_busy(page, session)


def scenario_d(page, before):
    session = session_of(page, "w-1")
    press_max(page, "w-2")
    slept = wait_for(
        page, "() => __W('w-1').classList.contains('dormant') && !__W('w-1')._term", DORMANT_AFTER_S + 3
    )
    if not slept:
        raise RuntimeError("console 1 did not sleep")
    known = len(sockets_of(session))
    press_max(page, "w-2")
    wait_for(page, "() => __W('w-1')._term?.term?.cols > 0", 10)
    record(page, "wake", session, known, before)
    facts["screen_equal_after_wake"] = facts["wake"]["screen_equal"]
    restore_all(page)


def scenario_e(page):
    before = settle(page, "w-1")
    session = session_of(page, "w-1")
    known = len(sockets_of(session))
    page.reload()
    if not boot(page):
        raise RuntimeError("the desk did not come back after a reload")
    record(page, "reattach", session, known, before)


def attach_traffic(log_path, page_session):
    """Put each measured socket's daemon traffic line on its fact. A socket logs
    one line when it closes; the first socket of a session (the launch) logs the first."""
    deadline = time.time() + 8
    lines = []
    while time.time() < deadline:
        lines = [t for t in traffic_lines(log_path) if t.get("session") == page_session]
        if len(lines) >= len(sockets_of(page_session)) + 1:
            break
        time.sleep(0.5)
    mine = sockets_of(page_session)
    for name in ("reconnect", "reconnect_busy", "wake", "reattach"):
        fact = facts.get(name)
        if not fact:
            continue
        sock = fact.pop("_socket", None)
        if sock is None or len(lines) != len(mine) + 1:
            continue
        index = [s["n"] for s in mine].index(sock)
        fact["traffic"] = lines[index + 1]
        if fact["replay_bytes"] is None:
            fact["replay_bytes"] = fact["traffic"].get("replay_bytes")
    for name in ("reconnect", "reconnect_busy", "wake", "reattach"):
        if name in facts:
            facts[name].pop("_socket", None)
    facts["traffic_lines"] = lines
    # The compressed snapshot on wake decides #581 item 5 (ADR-0079 D11).
    wake = facts.get("wake") or {}
    gzipped = (wake.get("open") or {}).get("replay_encoding") == "gzip"
    facts["wake_gzip_bytes"] = wake.get("replay_bytes") if gzipped else None


def run(args):
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb689_daemon_")
    log_path = os.path.join(daemon_dir, "daemon-stderr.log")
    slug = register_fixture(daemon_dir, make_fixture_repo())
    write_desk(daemon_dir, slug)
    proc = launch(daemon_dir, log_path)
    page_session = None
    try:
        if not wait_listening(BASE):
            errors.append("the daemon did not listen")
            return
        with sync_playwright() as p:
            browser = p.chromium.launch(args=["--disable-webgl", "--disable-gpu"])
            page = browser.new_context(viewport=dict(VIEW)).new_page()
            page.on("pageerror", lambda e: errors.append(f"pageerror: {e}"))
            page.on("websocket", on_websocket)
            page.goto(BASE)
            if not boot(page):
                errors.append("the consoles did not start")
                browser.close()
                return
            page_session = session_of(page, "w-1")
            before = None
            for name, step in (
                ("A", lambda: scenario_a(page)),
                ("B", lambda: scenario_b(page)),
                ("F", lambda: scenario_f(page)),
                ("C", lambda: scenario_c(page)),
            ):
                try:
                    result = step()
                    before = result if name == "C" else before
                except Exception as exc:  # a scenario that cannot run is reported, then the rest go on
                    errors.append(f"scenario {name}: {exc}")
                restore_all(page)
            try:
                scenario_d(page, before)
            except Exception as exc:
                errors.append(f"scenario D: {exc}")
            try:
                scenario_e(page)
            except Exception as exc:
                errors.append(f"scenario E: {exc}")
            browser.close()
        if page_session is not None:
            attach_traffic(log_path, page_session)
    finally:
        stop(proc)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--label", default="latest")
    parser.add_argument("--assert", dest="asserts", action="append", default=[], choices=sorted(ASSERTS) or None)
    args = parser.parse_args()
    run(args)
    head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=REPO_ROOT, capture_output=True, encoding="utf-8")
    facts["daemon_commit"] = head.stdout.strip()
    facts["errors"] = errors
    facts.setdefault("resume", None)
    os.makedirs(LOGS, exist_ok=True)
    out = os.path.join(LOGS, f"689-measure-{args.label}.json")
    with open(out, "w", encoding="utf-8") as fh:
        json.dump(facts, fh, indent=2, ensure_ascii=False)
    print(json.dumps({k: v for k, v in facts.items() if k not in ("sizes", "columns_sizes", "traffic_lines")}, indent=2))
    print(f"wrote {out}")
    if errors:
        print("could not run: " + "; ".join(errors), file=sys.stderr)
        sys.exit(2)
    names = DEFAULT_ASSERTS + [n for n in args.asserts if n not in DEFAULT_ASSERTS]
    failed = [name for name in names if not ASSERTS[name](facts)]
    if failed:
        print("failed asserts: " + ", ".join(failed), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
