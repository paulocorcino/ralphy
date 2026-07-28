"""#301 browser acceptance: the board goes live — refresh policy, running cards, navigation.

One Playwright pass over a REAL daemon. The `board.list` / `label.set` verbs are
intercepted in the page (the board fold spawns a CLI that needs a real GitHub
tracker, which a throwaway fixture repo has not); EVERY other verb — including
the whole run channel (`runs.list` + `runs.watch`) — stays real, so the running
card pill is driven by the same path a real run drives.

Scenario 1   the refresh predicate, in isolation: a 15-row table over
             `WBKanban.shouldRefresh`, touching no board state
Scenario 2   a project switch with the board CLOSED spawns no fold; opening it does
Scenario 3   the explicit refresh control reloads the board
Scenario 4   a label mutation reply refreshes the board; a REFUSED one does not
Scenario 5   a run snapshot change refreshes the board, attributed to `runs`;
             an ERRORING fold still throttles; an in-flight fold COALESCES the
             next trigger and replays it once on settle (never drops it)
Scenario 6   a hidden tab never refreshes (6a) and refreshes on becoming visible
             (6b), attributed to `visible`
Scenario 7   the backstop tick fires once past the backstop window, then coalesces
Scenario 8   the actively-worked issue's card is marked with its phase + agent
Scenario 9   clicking the card run pill opens the Runs panel on that run, focused
Scenario 10  clicking a trail node opens that issue's detail — from a CLOSED
             board, the leg that exercises `focusIssue`'s toggle+select ordering

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
    # `focused` is consulted by `backstop` ONLY: an implementation that gated the
    # operator's own control on it would kill refresh on an unfocused window, and
    # every row above would still pass.
    ("manual ignores focus — the operator asked", dict(trigger="manual", sinceMs=0, boardOpen=True, docVisible=True, focused=False), True),
    ("a run push ignores focus", dict(trigger="runs", sinceMs=6000, boardOpen=True, docVisible=True, focused=False), True),
    ("focused defaults to true when omitted", dict(trigger="backstop", sinceMs=130000, boardOpen=True, docVisible=True), True),
]

# The spy: record every `board.list` call and answer it from a fixed fold, answer
# `label.set` OK, and delegate everything else (the whole run channel) to the real
# daemon. Installed in the page so the client's own call path is exercised.
SPY_JS = """
() => {
  window.__boardCalls = [];
  window.__triggers = [];          // which trigger caused each accepted load
  window.__boardMode = "ok";       // "ok" | "error" | "defer"
  window.__labelMode = "ok";       // "ok" | "error"
  window.__deferred = null;        // the held resolver, for the in-flight leg
  const real = window.WBDaemon.observe.bind(window.WBDaemon);
  const row = (n, title) => ({
    number: n, title, state: "open", labels: ["ready-for-agent"],
    assignees: [], blocked_by: [], created: "2026-07-20T10:00:00Z", updated: "2026-07-24T10:00:00Z",
  });
  const okBoard = () => ({
    status: "ok",
    board: {
      issues: [row(71, "the done one"), row(72, "the active one"), row(73, "the pending one")],
      labels: [{ name: "ready-for-agent", color: "0E8A16" }, { name: "AFK", color: "34A985" }],
    },
  });
  window.__okBoard = okBoard;
  window.WBDaemon.observe = (verb, payload) => {
    if (verb === "board.list") {
      window.__boardCalls.push(payload);
      if (window.__boardMode === "error") return Promise.resolve({ status: "error", message: "fold refused" });
      if (window.__boardMode === "defer") {
        return new Promise((res) => { window.__deferred = () => res(okBoard()); });
      }
      return Promise.resolve(okBoard());
    }
    if (verb === "label.set") {
      return Promise.resolve(
        window.__labelMode === "error" ? { status: "error", message: "label refused" } : { status: "ok" },
      );
    }
    return real(verb, payload);
  };
  // Attribute each ACCEPTED load to the trigger that asked for it: the recorded
  // `board.list` payload is `{repo}` for all five, so a count delta alone cannot
  // tell a `visible` refresh from a stray `runs` push.
  const d = Alpine.$data(document.querySelector('[x-data]'));
  const realMaybe = d.maybeRefreshBoard.bind(d);
  d.maybeRefreshBoard = (trigger) => {
    const before = window.__boardCalls.length;
    const out = realMaybe(trigger);
    if (window.__boardCalls.length > before) window.__triggers.push(trigger);
    return out;
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

            # A REFUSED label write must not re-fold (the error branch reverts and
            # flashes; nothing changed server-side worth re-reading).
            page.evaluate("() => { window.__labelMode = 'error'; }")
            before = board_calls(page)
            page.evaluate(
                f"async () => {{ const d = {SH};"
                f" d.toggleLabel(d.projectIssues().find(i => i.number === 71), 'AFK');"
                f" await new Promise(r => setTimeout(r, 300)); }}"
            )
            page.wait_for_timeout(400)
            n = board_calls(page)
            check("a REFUSED label write does not refresh the board", n == before, f"{before} -> {n}")
            page.evaluate("() => { window.__labelMode = 'ok'; }")

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
            trig = page.evaluate("() => window.__triggers")
            check(
                "…and the load is attributed to the `runs` trigger",
                trig[-1] == "runs",
                f"triggers={trig}",
            )

            # --- an ERRORING fold throttles like a healthy one -----------------
            # `_boardLoadedAt` is stamped before the await, so a refused fold must
            # not leave the board un-throttled and re-folding on every push.
            page.evaluate("() => { window.__boardMode = 'error'; }")
            page.evaluate(f"() => {{ {SH}._boardLoadedAt = Date.now() - 10000; }}")
            before = board_calls(page)
            page.evaluate(f"() => {SH}.refreshBoard()")
            page.wait_for_timeout(400)
            mid = board_calls(page)
            check("an erroring fold still counts as a load", mid == before + 1, f"{before} -> {mid}")
            check(
                "…and raises the board error state",
                page.evaluate(f"() => !!{SH}.boardError[{SH}.openSlug]") is True,
                f"boardError={page.evaluate(f'() => {SH}.boardError[{SH}.openSlug]')!r}",
            )
            page.evaluate(f"() => {SH}.maybeRefreshBoard('runs')")
            page.wait_for_timeout(400)
            n = board_calls(page)
            check("…so the next push inside the min gap is throttled away", n == mid, f"{mid} -> {n}")
            page.evaluate("() => { window.__boardMode = 'ok'; }")
            page.evaluate(f"() => {SH}.refreshBoard()")
            page.wait_for_timeout(400)
            check(
                "the board recovers from the error state on the next good fold",
                page.evaluate(f"() => {SH}.boardError[{SH}.openSlug]") in (None, ""),
                "",
            )

            # --- a fold in flight COALESCES the next trigger, never drops it ---
            page.evaluate("() => { window.__boardMode = 'defer'; window.__deferred = null; }")
            page.evaluate(f"() => {{ {SH}._boardLoadedAt = Date.now() - 10000; }}")
            before = board_calls(page)
            page.evaluate(f"() => {SH}.refreshBoard()")
            page.wait_for_timeout(300)
            check("a deferred fold is in flight", board_calls(page) == before + 1, "")
            check(
                "…and the refresh control is disabled while it runs",
                page.locator(".kanban-refresh").is_disabled(),
                "",
            )
            # A second trigger while it is in flight must NOT spawn a second fold…
            page.evaluate(f"() => {SH}.refreshBoard()")
            page.wait_for_timeout(300)
            check("…a concurrent trigger spawns no second fold", board_calls(page) == before + 1, "")
            # …but must not be lost either: releasing the first replays it once.
            page.evaluate("() => { window.__boardMode = 'ok'; window.__deferred(); }")
            page.wait_for_timeout(600)
            n = board_calls(page)
            check("…it is REPLAYED when the in-flight fold settles", n == before + 2, f"{before} -> {n}")
            check(
                "…and the guard clears (the control is live again)",
                page.locator(".kanban-refresh").is_disabled() is False,
                "",
            )

            # --- scenario 6a: a HIDDEN tab never refreshes ---------------------
            # A second page in the same context takes the foreground, so page 1's
            # real `document.visibilityState` flips to `hidden` (no stubbing).
            other = ctx.new_page()
            other.goto("about:blank")
            other.bring_to_front()
            page.wait_for_timeout(400)
            vis = page.evaluate("() => document.visibilityState")
            if vis != "hidden":
                # Headless chromium does not background a page just because a
                # sibling took the foreground. Stub the STATE (the predicate's
                # own input) rather than lose the scenario; the handler, the
                # predicate and the wiring under test all stay real.
                page.evaluate(
                    "() => { Object.defineProperty(document, 'visibilityState',"
                    " { get: () => 'hidden', configurable: true }); }"
                )
            # Not "is it hidden" (a tautology after the stub) but "does the
            # PREDICATE see it hidden" — the input the policy actually reads.
            check(
                "the predicate sees a hidden document and refuses",
                page.evaluate(
                    "() => window.WBKanban.shouldRefresh({ trigger: 'runs', sinceMs: 99999,"
                    " boardOpen: true, docVisible: document.visibilityState === 'visible' })"
                )
                is False,
                f"natural={vis!r} (stubbed={vis != 'hidden'})",
            )

            # Back-date the clock so the ONLY thing blocking a refresh is hiddenness.
            page.evaluate(f"() => {{ {SH}._boardLoadedAt = Date.now() - 10000; }}")
            before = board_calls(page)
            doc_a.write_text(json.dumps(snapshot(RUN_A, "planning", "planning")), encoding="utf-8")
            page.wait_for_function(f"() => {SH}.projectRuns()[0]?.phase === 'planning'", timeout=15000)
            page.wait_for_timeout(600)
            n = board_calls(page)
            check("a hidden tab never refreshes the board", n == before, f"{before} -> {n}")
            check(
                "…while its run channel keeps advancing",
                page.evaluate(f"() => {SH}.projectRuns()[0].phase") == "planning",
                "",
            )

            # --- scenario 6b: becoming visible again refreshes ------------------
            doc_a.write_text(json.dumps(snapshot(RUN_A, "executing", "executing")), encoding="utf-8")
            page.wait_for_function(f"() => {SH}.projectRuns()[0]?.phase === 'executing'", timeout=15000)
            page.evaluate(f"() => {{ {SH}._boardLoadedAt = Date.now() - 10000; }}")
            before = board_calls(page)
            if vis != "hidden":
                page.evaluate("() => { delete document.visibilityState; }")
            other.close()
            page.bring_to_front()
            if vis != "hidden":
                page.evaluate("() => document.dispatchEvent(new Event('visibilitychange'))")
            page.wait_for_timeout(800)
            n = board_calls(page)
            check("the board reloads when the document becomes visible again", n == before + 1, f"{before} -> {n}")
            trig = page.evaluate("() => window.__triggers")
            check(
                "…caused by the `visible` trigger, not a stray run push",
                trig[-1] == "visible",
                f"triggers={trig[-3:]}",
            )

            # --- scenario 7: the slow backstop --------------------------------
            page.evaluate(f"() => {{ {SH}._boardLoadedAt = Date.now() - 130000; }}")
            before = board_calls(page)
            page.evaluate(f"() => {SH}.boardBackstopTick()")
            page.wait_for_timeout(400)
            n = board_calls(page)
            check("a backstop tick past its window reloads the board", n == before + 1, f"{before} -> {n}")

            page.evaluate(f"() => {SH}.boardBackstopTick()")
            page.wait_for_timeout(400)
            n2 = board_calls(page)
            check("the very next backstop tick coalesces away", n2 == n, f"{n} -> {n2}")

            # --- scenario 8: the live card is marked --------------------------
            # `offsetParent === null` is the x-show oracle: an Alpine-hidden pill
            # is still in the DOM, so a textContent read alone would pass on a
            # card that shows nothing.
            cards = page.evaluate(
                """() => Object.fromEntries(
                     Array.from(document.querySelectorAll('.kanban-card')).map((c) => {
                       const pill = c.querySelector('.kc-run');
                       return [c.querySelector('.kc-num').textContent.trim(), {
                         running: c.classList.contains('running'),
                         pillShown: !!pill && pill.offsetParent !== null,
                         txt: (c.querySelector('.kc-run .kc-run-txt')?.textContent || '').trim(),
                       }];
                     }))"""
            )
            c72 = cards.get("#72", {})
            check("the worked issue's card is marked running", c72.get("running") is True, f"got={c72}")
            check("its run pill is visible", c72.get("pillShown") is True, f"got={c72}")
            check(
                "the pill names the phase and the agent",
                c72.get("txt") == "executing · opencode",
                f"got={c72.get('txt')!r}",
            )
            idle = {k: v for k, v in cards.items() if k != "#72"}
            check(
                "no other card carries a run pill",
                len(idle) == 2 and not any(v["pillShown"] or v["running"] for v in idle.values()),
                f"got={idle}",
            )

            page.screenshot(path=os.path.join(SHOT_DIR, "301-board-live-2026-07-25.png"))

            # --- scenario 9: card pill -> the run -----------------------------
            check("the Runs panel starts closed", page.evaluate(f"() => {SH}.runsOpen") is False)
            page.click(".kanban-card.running .kc-run")
            page.wait_for_timeout(600)
            check("clicking the run pill opens the Runs panel", page.evaluate(f"() => {SH}.runsOpen") is True)
            rid = page.evaluate(f"() => {SH}.currentRunId")
            check("…on that run", rid == RUN_A, f"got={rid!r}")
            focused = page.evaluate(
                "() => Array.from(document.querySelectorAll('.trail-node.focus'))"
                ".map(e => e.getAttribute('data-issue'))"
            )
            check(
                "…with exactly that issue marked in the trail",
                focused == ["72"],
                f"got={focused} trailFocus={page.evaluate(f'() => {SH}.trailFocus')!r}",
            )
            # The pill must NOT also open the detail drawer (`@click.stop`).
            check(
                "the pill click did not fall through to the card",
                page.evaluate(f"() => {SH}.kanbanSel") is None,
                f"kanbanSel={page.evaluate(f'() => {SH}.kanbanSel')!r}",
            )

            # --- scenario 10: trail node -> the issue detail -------------------
            # From a CLOSED board — the leg that exercises `focusIssue`'s
            # `if (!this.kanbanOpen) this.toggleKanban()` and its ordering against
            # `toggleKanban`'s `kanbanSel = null` reset. With the board already
            # open that branch never runs and the ordering hazard is untested.
            page.evaluate(f"() => {{ if ({SH}.kanbanOpen) {SH}.toggleKanban(); }}")
            page.wait_for_timeout(300)
            check("the board starts CLOSED for the run->board leg", page.evaluate(f"() => {SH}.kanbanOpen") is False)
            page.click('.trail-node[data-issue="73"]')
            page.wait_for_timeout(600)
            check("clicking a trail node keeps the board open", page.evaluate(f"() => {SH}.kanbanOpen") is True)
            check("…and closes the Runs panel", page.evaluate(f"() => {SH}.runsOpen") is False)
            sel = page.evaluate(f"() => {SH}.kanbanSel")
            check("…selecting that issue", sel == 73, f"got={sel!r}")
            drawer = page.locator(".kanban-detail.open")
            check(
                "…with its detail drawer open on it",
                drawer.is_visible() and "#73" in drawer.inner_text(),
                f"text={drawer.inner_text()[:60]!r}" if drawer.count() else "no drawer",
            )

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The count floor is load-bearing: an early `sys.exit` or a scenario that
    # never ran must not report success on a handful of passing checks.
    ok = all(results) and len(results) >= 53
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("BOARD LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
