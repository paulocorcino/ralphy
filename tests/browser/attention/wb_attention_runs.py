"""A run that waits, in a real browser: a run snapshot whose agent waits is
one request of the attention list, "Open the run" shows that run in the Runs
panel, and the run card shows the agent's state and what it asks. A finished
run's issues are requests with "Dismiss", and a dismissed one stays hidden
after a reload and on a second device.

One Playwright pass over a REAL daemon. The script stands in for the run: it
writes a hand-made snapshot to `<repo>/.ralphy/runstate/<runid>.json` with
its own pid, so the daemon's reader keeps it as live. The daemon's attention
tick reads it, pushes `attention.dirty`, and the page reads the list.

Scenario 1  no run: the badge is hidden
Scenario 2  a snapshot whose `phase.agent` waits -> the badge says `1`, the
            panel has one row with the agent's detail and "Open the run"
Scenario 3  "Open the run" opens the run's project and the Runs panel; the run
            card shows the `waiting` dot and the detail; screenshot HERE
Scenario 4  the snapshot is removed -> the badge is hidden
Scenario 5  a final snapshot in a run folder with issues `blocked` and `hitl`
            -> two rows, each with "Dismiss" only
Scenario 6  "Dismiss" on the `blocked` row -> after a reload one row is left
            (`dismiss holds`)
Scenario 7  a second browser context (the second device) sees the same one
            row (`second device`)
Scenario 8  no page errors

Boots a Localhost daemon on 7472 over a SCRATCH `RALPHY_DAEMON_DIR`. The
daemon is stopped by its own subprocess handle, NEVER by name. Built on the
helpers of `agents/wb_agent_state.py`; `RALPHY_TEST_SKIP_BUILD=1` and
`RALPHY_TEST_TARGET_DIR` work as there.

Prints `PASS <name>` or `FAIL <name>` per check. Writes
.ralphy/screenshots/attention-run-2026-10-28.png (`-linux` off Windows).
Run: python tests/browser/attention/wb_attention_runs.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "agents"))

from playwright.sync_api import sync_playwright  # noqa: E402

import wb_agent_state as agents  # noqa: E402

PORT = 7472
BASE = f"http://127.0.0.1:{PORT}/"
SHOT = "attention-run-2026-10-28" + ("" if os.name == "nt" else "-linux") + ".png"
RUNID = "01RUNWAITSAAAAAAAAAAAAAA"
DETAIL = "permission: Bash"

BADGE = (
    "() => { const b = document.querySelector('.rail-attention .attention-badge');"
    "  return b && b.offsetParent !== null ? b.textContent.trim() : null; }"
)
ROWS = (
    "() => [...document.querySelectorAll('.attention-panel .attention-row')]"
    "  .filter(r => r.offsetParent !== null)"
    "  .map(r => ({ detail: (r.querySelector('.attention-detail') || {}).textContent || '',"
    "    kind: (r.querySelector('.attention-kind') || {}).textContent || '',"
    "    buttons: [...r.querySelectorAll('button')].filter(b => b.offsetParent !== null)"
    "      .map(b => b.textContent.trim()) }))"
)
PANEL_OPEN = "() => { const p = document.querySelector('.attention-panel'); return !!p && p.offsetParent !== null; }"
CARD = (
    "() => { const c = document.querySelector('aside.runs .run-select-btn');"
    "  if (!c || c.offsetParent === null) return null;"
    "  const dot = c.querySelector('.run-agent-state');"
    "  const detail = c.querySelector('.run-agent-detail');"
    "  return { dot: dot && dot.offsetParent !== null ? dot.className : '',"
    "    detail: detail && detail.offsetParent !== null ? detail.textContent.trim() : '' }; }"
)

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"{'PASS' if ok else 'FAIL'} {name} {detail}".rstrip(), flush=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [agents.EXE, "daemon", "--port", str(PORT)],
        env=agents.empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def write_snapshot(repo):
    """A live run of version 1, on issue 71, whose executor waits."""
    doc = {
        "v": 1,
        "runid": RUNID,
        "pid": os.getpid(),
        "title": "attention run",
        "repo": "owner/attention",
        "branch": "afk/run-1",
        "plan_agent": "claude",
        "exec_agent": "claude",
        "started_at": "2026-10-28T10:00:00-03:00",
        "plan_path": "",
        "queue": {"total": 1, "order": [71], "stop_before": None},
        "issues": [{"number": 71, "title": "the issue", "status": "executing"}],
        "phase": {
            "active": 71,
            "state": "executing",
            "since": "2026-10-28T10:00:00-03:00",
            "agent": {"state": "waiting", "since": "2026-10-28T10:05:00-03:00", "detail": DETAIL},
        },
        "plan": {"issue": None, "steps": []},
    }
    path = os.path.join(repo, ".ralphy", "runstate", f"{RUNID}.json")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(doc, f)
    return path


STAMP = "20261028-100000"


def write_final(repo):
    """The final snapshot a finished run left in its run folder: issue 71
    ended `blocked`, issue 72 `hitl`."""
    doc = {
        "v": 1,
        "runid": "01RUNFINISHEDAAAAAAAAAAAA",
        "pid": 1,
        "title": "attention run",
        "repo": "owner/attention",
        "branch": "afk/run-1",
        "plan_agent": "claude",
        "exec_agent": "claude",
        "started_at": "2026-10-28T10:00:00-03:00",
        "plan_path": "",
        "queue": {"total": 2, "order": [71, 72], "stop_before": None},
        "issues": [
            {"number": 71, "title": "one", "status": "blocked"},
            {"number": 72, "title": "two", "status": "hitl"},
        ],
        "phase": {"active": None, "state": "finished"},
        "plan": {"issue": None, "steps": []},
    }
    path = os.path.join(repo, ".ralphy", "runs", STAMP, "snapshot.json")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(doc, f)


def details(page):
    return [r["detail"].strip() for r in page.evaluate(ROWS)]


def open_panel(page):
    if not page.evaluate(PANEL_OPEN):
        page.locator(".rail-attention > button").click()
    return wait_true(page, PANEL_OPEN, timeout=5000)


def wait_true(page, js, timeout=15000):
    try:
        page.wait_for_function(js, timeout=timeout)
        return True
    except Exception:
        return False


def main():
    os.makedirs(agents.SHOT_DIR, exist_ok=True)
    agents.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbattruns_reg_")
    fixture = agents.seed("wbattruns_", "plain")
    slug = agents.register_fixture(daemon_dir, str(fixture))

    proc = launch(daemon_dir)
    try:
        if not agents.wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            agents.wait_shell(page)

            # --- scenario 1: no run ---------------------------------------------
            page.wait_for_timeout(1500)
            check("no badge before a run", page.evaluate(BADGE) is None, f"got={page.evaluate(BADGE)!r}")

            # --- scenario 2: a run whose agent waits ----------------------------
            snapshot = write_snapshot(str(fixture))
            ok = wait_true(page, f"() => ({BADGE})() === '1'")
            check("badge", ok, f"got={page.evaluate(BADGE)!r}")
            page.locator(".rail-attention > button").click()
            wait_true(page, PANEL_OPEN, timeout=5000)
            rows = page.evaluate(ROWS)
            ok = (
                len(rows) == 1
                and rows[0]["detail"].strip() == DETAIL
                and rows[0]["kind"].strip() == "Waits for you"
                and rows[0]["buttons"] == ["Open the run"]
            )
            check("run row", ok, f"got={rows!r}")

            # --- scenario 3: open the run ---------------------------------------
            page.locator(".attention-panel .attention-go", has_text="Open the run").click()
            check("open the run closes the panel", wait_true(page, f"() => !({PANEL_OPEN})()", timeout=5000))
            ok = wait_true(page, f"() => {{ const c = ({CARD})(); return !!c && c.detail === {json.dumps(DETAIL)}; }}")
            card = page.evaluate(CARD)
            ok = ok and card is not None and "waiting" in card["dot"].split()
            check("run card", ok, f"got={card!r} slug={slug}")
            page.screenshot(path=os.path.join(agents.SHOT_DIR, SHOT))

            # --- scenario 4: the run ends ----------------------------------------
            os.remove(snapshot)
            ok = wait_true(page, f"() => ({BADGE})() === null")
            check("cleared", ok, f"badge={page.evaluate(BADGE)!r}")

            # --- scenario 5: a finished run ------------------------------------
            write_final(str(fixture))
            ok = wait_true(page, f"() => ({BADGE})() === '2'")
            open_panel(page)
            rows = page.evaluate(ROWS)
            ok = (
                ok
                and [r["detail"].strip() for r in rows] == ["blocked", "hitl"]
                and all(r["buttons"] == ["Dismiss"] for r in rows)
            )
            check("finished rows", ok, f"got={rows!r}")

            # --- scenario 6: dismiss, then reload -------------------------------
            page.locator(".attention-panel .attention-row", has_text="blocked").locator(
                ".attention-dismiss"
            ).click()
            wait_true(page, f"() => ({BADGE})() === '1'")
            page.reload()
            agents.wait_shell(page)
            ok = wait_true(page, f"() => ({BADGE})() === '1'")
            open_panel(page)
            check("dismiss holds", ok and details(page) == ["hitl"], f"got={details(page)!r}")

            # --- scenario 7: a second device -------------------------------------
            other = browser.new_context(viewport={"width": 1440, "height": 900})
            second = other.new_page()
            second.on("pageerror", lambda e: thrown.append(str(e)))
            second.goto(BASE)
            agents.wait_shell(second)
            ok = wait_true(second, f"() => ({BADGE})() === '1'")
            open_panel(second)
            check("second device", ok and details(second) == ["hitl"], f"got={details(second)!r}")
            other.close()

            # --- scenario 8 -----------------------------------------------------
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        agents.stop(proc)

    check_floor = 11
    if len(results) != check_floor:
        print(f"FAIL the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
