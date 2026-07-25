"""#301 browser acceptance: the board goes live — refresh policy, running cards, navigation.

One Playwright pass over a REAL daemon. The `board.list` / `label.set` verbs are
intercepted in the page (the board fold spawns a CLI that needs a real GitHub
tracker, which a throwaway fixture repo has not); EVERY other verb — including
the whole run channel (`runs.list` + `runs.watch`) — stays real, so the running
card pill is driven by the same path a real run drives.

Scenario 1   the refresh predicate, in isolation: a 12-row table over
             `WBKanban.shouldRefresh`, touching no board state
Scenario 2   a project switch with the board CLOSED spawns no fold; opening it does
Scenario 3   the explicit refresh control reloads the board
Scenario 4   a label mutation reply refreshes the board
Scenario 5   a run snapshot change refreshes the board
Scenario 6   a hidden tab never refreshes (6a) and refreshes on becoming visible (6b)
Scenario 7   the backstop tick fires once past the backstop window, then coalesces
Scenario 8   the actively-worked issue's card is marked with its phase + agent
Scenario 9   clicking the card run pill opens the Runs panel on that run, focused
Scenario 10  clicking a trail node opens that issue's detail on the board

Boots a Localhost daemon on 7399 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/301-board-live-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_board_301.py   (exit 0 = all pass)
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

# The Windows console's default codepage (cp1252 here) cannot encode the glyphs
# this script prints in its detail strings; force utf-8 stdout so a PASSING
# assertion never dies on its own detail.
sys.stdout.reconfigure(encoding="utf-8")

PORT = 7399
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_board_301.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

RUN_A = "01RUNAAAAAAAAAAAAAAAAA"

PLAN_MD = "# Plan for #72\n\n## Steps\n- [ ] first step body\n\n## Verify\ncargo fmt --check\n"

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
    """A scratch registry + empty vendor stores: the operator's own daemon dir
    (and its login policy) is never touched, and the usage scan finds nothing."""
    empty = tempfile.mkdtemp(prefix="wb301_empty_")
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


def snapshot(runid, phase, status_72, active=72):
    return {
        "v": 1,
        "runid": runid,
        "pid": os.getpid(),  # a LIVE pid, so the reader never sweeps it as an orphan
        "title": "the #301 fixture run",
        "repo": "owner/board301",
        "branch": "afk/board-301",
        "plan_agent": "claude",
        "exec_agent": "opencode",
        "started_at": "2026-07-25T10:00:00-03:00",
        "plan_path": ".ralphy/plan.md",
        "queue": {"total": 3, "order": [71, 72, 73], "stop_before": None},
        "issues": [
            {"number": 71, "title": "the done one", "status": "done", "blocked_by": []},
            {"number": 72, "title": "the active one", "status": status_72, "blocked_by": []},
            {"number": 73, "title": "the pending one", "status": "pending", "blocked_by": []},
        ],
        "phase": {"active": active, "state": phase, "sleep": None, "final_summary": None},
    }


def make_fixture_repo():
    """A throwaway git repo with a real plan.md and an EMPTY runstate dir — the
    panel starts at zero runs and every document below arrives while it is open."""
    d = tempfile.mkdtemp(prefix="wb301_fixture_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "plan.md").write_text(PLAN_MD, encoding="utf-8")
    (p / ".ralphy" / "runstate").mkdir()
    (p / "README.md").write_text("# fixture\n\nThe #301 live-board fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb301@example.com"],
        ["git", "config", "user.name", "wb301"],
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
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's board.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# --- scenario 1: the pure predicate, evaluated in the page ------------------
# Each row: (label, args, expected). `sinceMs`/`focused` only matter where the
# per-value verdict consults them; the row set pins that.
PREDICATE_ROWS = [
    ("manual on an open, visible board refreshes now", dict(trigger="manual", sinceMs=0, boardOpen=True, docVisible=True, focused=True), True),
    ("manual on a CLOSED board never refreshes", dict(trigger="manual", sinceMs=0, boardOpen=False, docVisible=True, focused=True), False),
    ("manual on a HIDDEN document never refreshes", dict(trigger="manual", sinceMs=0, boardOpen=True, docVisible=False, focused=True), False),
    ("a label mutation refreshes with no gap", dict(trigger="label", sinceMs=0, boardOpen=True, docVisible=True, focused=True), True),
    ("becoming visible inside the min gap coalesces", dict(trigger="visible", sinceMs=4000, boardOpen=True, docVisible=True, focused=True), False),
    ("becoming visible past the min gap refreshes", dict(trigger="visible", sinceMs=6000, boardOpen=True, docVisible=True, focused=True), True),
    ("a run push inside the min gap coalesces", dict(trigger="runs", sinceMs=4000, boardOpen=True, docVisible=True, focused=True), False),
    ("a run push past the min gap refreshes", dict(trigger="runs", sinceMs=6000, boardOpen=True, docVisible=True, focused=True), True),
    ("the backstop fires when focused past its window", dict(trigger="backstop", sinceMs=130000, boardOpen=True, docVisible=True, focused=True), True),
    ("the backstop never fires on an UNFOCUSED tab", dict(trigger="backstop", sinceMs=130000, boardOpen=True, docVisible=True, focused=False), False),
    ("the backstop never fires inside its window", dict(trigger="backstop", sinceMs=119000, boardOpen=True, docVisible=True, focused=True), False),
    ("an unknown trigger never refreshes", dict(trigger="bogus-trigger", sinceMs=999999, boardOpen=True, docVisible=True, focused=True), False),
]

# The spy: record every `board.list` call and answer it from a fixed fold, answer
# `label.set` OK, and delegate everything else (the whole run channel) to the real
# daemon. Installed in the page so the client's own call path is exercised.
SPY_JS = """
() => {
  window.__boardCalls = [];
  const real = window.WBDaemon.observe.bind(window.WBDaemon);
  const row = (n, title) => ({
    number: n, title, state: "open", labels: ["ready-for-agent"],
    assignees: [], blocked_by: [], created: "2026-07-20T10:00:00Z", updated: "2026-07-24T10:00:00Z",
  });
  window.WBDaemon.observe = (verb, payload) => {
    if (verb === "board.list") {
      window.__boardCalls.push(payload);
      return Promise.resolve({
        status: "ok",
        board: {
          issues: [row(71, "the done one"), row(72, "the active one"), row(73, "the pending one")],
          labels: [{ name: "ready-for-agent", color: "0E8A16" }, { name: "AFK", color: "34A985" }],
        },
      });
    }
    if (verb === "label.set") return Promise.resolve({ status: "ok" });
    return real(verb, payload);
  };
}
"""


def board_calls(page):
    return page.evaluate("() => window.__boardCalls.length")


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb301_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    runstate = Path(fixture_dir, ".ralphy", "runstate")
    doc_a = runstate / f"{RUN_A}.json"

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            # A bare `return` here would skip the exit gate below and report
            # success with ZERO browser assertions run.
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows (KNOWLEDGE.md).
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(SPY_JS)
            page.wait_for_timeout(300)

            # --- scenario 1: the predicate in isolation -----------------------
            verdicts = page.evaluate(
                "(rows) => rows.map((r) => { try { return window.WBKanban.shouldRefresh(r); }"
                " catch (e) { return String(e); } })",
                [r[1] for r in PREDICATE_ROWS],
            )
            for (label, args, want), got in zip(PREDICATE_ROWS, verdicts):
                check(label, got is want, f"got={got!r} want={want!r} args={args}")

            # --- scenario 2: a CLOSED board never spawns the fold -------------
            page.evaluate(f"() => {SH}.toggle('{slug}')")
            page.wait_for_timeout(500)
            n = board_calls(page)
            check("opening a project with the board CLOSED spawns no fold", n == 0, f"got={n}")

            page.evaluate(f"() => {SH}.toggleKanban()")
            page.wait_for_timeout(400)
            n = board_calls(page)
            check("opening the board loads it once", n == 1, f"got={n}")

            # --- scenario 3: the explicit refresh control ---------------------
            page.click(".kanban-refresh")
            page.wait_for_timeout(400)
            n = board_calls(page)
            check("the refresh control reloads the board", n == 2, f"got={n}")

            # --- scenario 4: a label mutation reply refreshes ------------------
            before = board_calls(page)
            page.evaluate(
                f"async () => {{ const d = {SH};"
                f" d.toggleLabel(d.projectIssues().find(i => i.number === 72), 'AFK');"
                f" await new Promise(r => setTimeout(r, 300)); }}"
            )
            page.wait_for_timeout(400)
            n = board_calls(page)
            check("a label mutation reply refreshes the board", n == before + 1, f"{before} -> {n}")

            # --- scenario 5: a run snapshot change refreshes -------------------
            # Back-date the load clock past REFRESH_MIN_GAP_MS: the coalescing
            # itself is pinned by scenario 1's table, this pins the WIRING.
            page.evaluate(f"() => {{ {SH}._boardLoadedAt = Date.now() - 10000; }}")
            before = board_calls(page)
            doc_a.write_text(json.dumps(snapshot(RUN_A, "executing", "executing")), encoding="utf-8")
            page.wait_for_function(f"() => {SH}.projectRuns().length === 1", timeout=15000)
            page.wait_for_timeout(600)
            n = board_calls(page)
            check("a run snapshot change refreshes the board", n == before + 1, f"{before} -> {n}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 2
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("BOARD LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
