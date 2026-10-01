"""Claude's --name from the console name (#480, ADR-0066 §6) browser acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7480.

`RALPHY_DAEMON_AGENT_OVERRIDE` makes every agent console a `session_test_child`,
so no vendor CLI is needed: the daemon still builds Claude's argv and announces
the chosen name in `session-open`, and the shell shows it as the last line of
the title tooltip.

The fixture repo has an `origin` of `owner/fincal` and opts in with
`.ralphy/settings.json` = `{"claude":{"console_name":true}}`.

Scenario 1  a new Claude console is `fincal #1`: its tooltip names `wb-fincal-1`
Scenario 2  a rename to `a&b %PATH% "x"` does not restart the session: the same
            session id, the old name in the tooltip
Scenario 3  the title-bar restart launches with the new, folded name
            `wb-a-b-path-x`
Scenario 4  with the opt-in off, the next restart has no `wb-` name

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_console_names_480.py   (exit 0 = all pass)
"""

import datetime
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

PORT = 7480
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
VIEW = {"width": 1600, "height": 900}
SLUG = "owner/fincal"
RENAMED = 'a&b %PATH% "x"'
FLOOR = 13  # every check above the floor check; pinned after the first green run

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
    empty = tempfile.mkdtemp(prefix="wbname480_empty_")
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


def write_opt_in(fixture_dir, on):
    body = b'{"claude":{"console_name":true}}' if on else b'{"claude":{"console_name":false}}'
    Path(fixture_dir, ".ralphy", "settings.json").write_bytes(body)


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wbname480_fixture_")
    p = Path(d)
    p.joinpath("README.md").write_bytes(b"# fixture\n")
    p.joinpath(".gitignore").write_bytes(b".ralphy/\n")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbname@example.com"],
        ["git", "config", "user.name", "wbname"],
        ["git", "remote", "add", "origin", f"https://github.com/{SLUG}.git"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    p.joinpath(".ralphy").mkdir()
    write_opt_in(d, True)
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # the previous build's console.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True
    )


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def session_ids():
    with urllib.request.urlopen(BASE + "api/sessions?local=1", timeout=5) as r:
        return [row["id"] for row in json.loads(r.read().decode())]


def poll_ids(predicate, timeout=15):
    deadline = time.time() + timeout
    got = None
    while time.time() < deadline:
        try:
            got = session_ids()
        except Exception:
            got = None
        if got is not None and predicate(got):
            return got
        time.sleep(0.3)
    return got


HELPERS = """() => {
  window.__W = (id) => [...document.querySelectorAll('.session-window')].find((w) => w._deskId === id) || null;
  window.__name = (id) => __W(id)?.querySelector('.session-name')?.textContent ?? null;
  window.__tip = (id) => __W(id)?.querySelector('.session-title')?.title ?? null;
  window.__input = (id) => __W(id)?.querySelector('.session-name-input') || null;
  window.__screen = (id) => { const b = __W(id)?._term?.term?.buffer.active; if (!b) return '';
    let out = ''; for (let y = 0; y < b.length; y++) { const l = b.getLine(y); if (l) out += l.translateToString(true) + '\\n'; }
    return out; };
  window.__center = (el) => { const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };
}"""


def tip_lines(page, id):
    return (page.evaluate("(id) => __tip(id)", id) or "").split("\n")


def wait_tip(page, id, line, timeout=15000):
    page.wait_for_function(
        "([id, line]) => (__tip(id) || '').split('\\n').includes(line)", arg=[id, line], timeout=timeout
    )


def rename(page, id, text):
    c = page.evaluate(
        "(id) => { const w = __W(id); w.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));"
        " return __center(w.querySelector('.session-name')); }",
        id,
    )
    page.mouse.dblclick(c["x"], c["y"])
    page.wait_for_function("(id) => !!__input(id)", arg=id, timeout=3000)
    page.keyboard.press("Control+A")
    page.keyboard.type(text)
    page.keyboard.press("Enter")
    page.wait_for_function("(id) => !__input(id)", arg=id, timeout=3000)
    return page.evaluate("(id) => __name(id)", id)


def restart(page, id, before):
    page.evaluate("(id) => { window.__old = __W(id); __W(id).querySelector('.session-restart').click(); }", id)
    page.wait_for_selector(".wb-confirm .btn.danger, .wb-confirm .btn.accent", timeout=5000)
    page.locator(".wb-confirm .btn.danger, .wb-confirm .btn.accent").click()
    page.wait_for_function(
        "(id) => __W(id) && __W(id) !== window.__old && !__W(id).classList.contains('placeholder')",
        arg=id,
        timeout=20000,
    )
    return poll_ids(lambda ids: len(ids) == 1 and ids[0] not in before)


def main():
    build()
    os.makedirs(SHOT_DIR, exist_ok=True)
    daemon_dir = tempfile.mkdtemp(prefix="wbname480_daemon_")
    fixture = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture)
    check("the fixture registers as owner/fincal", slug == SLUG, slug)
    proc = launch(daemon_dir)
    errors = []
    try:
        if not wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_context(viewport=dict(VIEW)).new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
            page.wait_for_timeout(800)
            page.evaluate(HELPERS)

            # 1: a new Claude console -------------------------------------
            page.evaluate(f"() => window.WBConsole.open({{ repo: '{SLUG}', agent: 'claude' }})")
            page.wait_for_function("() => document.querySelectorAll('.session-window').length === 1", timeout=15000)
            wid = page.evaluate("() => document.querySelector('.session-window')._deskId")
            check("1 the new console is `fincal #1`", page.evaluate("(id) => __name(id)", wid) == "fincal #1",
                  str(page.evaluate("(id) => __name(id)", wid)))
            try:
                wait_tip(page, wid, "wb-fincal-1")
                ok = True
            except Exception:
                ok = False
            check("1 Claude starts as wb-fincal-1", ok, str(tip_lines(page, wid)))
            first = poll_ids(lambda ids: len(ids) == 1)
            check("1 one session runs", first is not None and len(first) == 1, str(first))

            # 2: a rename does not restart ---------------------------------
            got = rename(page, wid, RENAMED)
            check("2 the rename is shown", got == RENAMED, repr(got))
            page.wait_for_timeout(1000)
            check("2 the tooltip keeps the running name", "wb-fincal-1" in tip_lines(page, wid),
                  str(tip_lines(page, wid)))
            now = session_ids()
            check("2 the same session still runs", now == first, f"{now} vs {first}")

            # 3: the next restart takes the new name -----------------------
            second = restart(page, wid, first or [])
            check("3 the restart is a new session", second is not None and len(second) == 1
                  and second != first, f"{second} vs {first}")
            try:
                wait_tip(page, wid, "wb-a-b-path-x")
                ok = True
            except Exception:
                ok = False
            check("3 Claude restarts as wb-a-b-path-x", ok, str(tip_lines(page, wid)))
            # The child echoes its own argv: the command line it was given, not
            # the spec the daemon built.
            page.evaluate("(id) => __W(id)._term.term.focus()", wid)
            page.wait_for_timeout(800)
            page.keyboard.type("argv", delay=80)
            page.keyboard.press("Enter")
            try:
                page.wait_for_function(
                    "(id) => __screen(id).includes('ARGV:--name wb-a-b-path-x ')",
                    arg=wid,
                    timeout=8000,
                )
                ok = True
            except Exception:
                ok = False
            check("3 the child's argv holds only the folded name", ok,
                  page.evaluate("(id) => __screen(id).trim().slice(0, 300)", wid))
            page.screenshot(
                path=os.path.join(SHOT_DIR, f"480-claude-name-{datetime.date.today().isoformat()}.png")
            )

            # 4: opt-in off -------------------------------------------------
            write_opt_in(fixture, False)
            third = restart(page, wid, second or [])
            check("4 the restart is a new session", third is not None and len(third) == 1
                  and third != second, f"{third} vs {second}")
            # `session-open` carries the name; wait for its environment line so
            # the check reads the new announcement, not an empty tooltip.
            page.wait_for_function("(id) => (__tip(id) || '').split('\\n').length >= 2", arg=wid, timeout=15000)
            page.wait_for_timeout(1000)
            lines = tip_lines(page, wid)
            check("4 with the opt-in off Claude gets no wb- name",
                  not any(line.startswith("wb-") for line in lines), str(lines))

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
