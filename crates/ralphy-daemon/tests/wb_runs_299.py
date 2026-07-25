"""#299 browser acceptance: the Runs panel fed by real run snapshots.

One Playwright pass over a REAL daemon proving the panel renders the snapshot
documents a run publishes under its repo's `.ralphy/runstate/` (ADR-0047) —
documents written HERE, by this python process, so nothing in the daemon or the
browser ever spawned the runs they describe (AC 8: a run started from a terminal
must appear).

Scenario 1 (two live runs + one dead + one malformed):
  · the run picker lists exactly 2 runs, and both are selectable
  · the issue trail reads ✅ / ⚙️ / ○ for the done / executing / pending issues
  · the run phase label reads "executing #71" and the counter "1/3"
  · the plan viewer's Steps block carries the repo's real plan.md text
  · the sleeping run's line reads "waiting for reset ~14:30"
Scenario 2 (every valid document removed, only the malformed one left):
  · the panel shows the ERROR state, and NOT the "No active runs" empty state

Boots a Localhost daemon on 7399 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/299-runs-panel-2026-07-24.png.
Run: python crates/ralphy-daemon/tests/wb_runs_299.py   (exit 0 = all pass)
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

# The Windows console's default codepage (cp1252 here) cannot encode the trail
# glyphs this script prints in its detail strings; force utf-8 stdout so a
# PASSING assertion never dies on its own detail.
sys.stdout.reconfigure(encoding="utf-8")

PORT = 7399
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_runs_299.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

RUN_A = "01RUNAAAAAAAAAAAAAAAAA"
RUN_B = "01RUNBBBBBBBBBBBBBBBBB"
RUN_C = "01RUNCCCCCCCCCCCCCCCCC"

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
    empty = tempfile.mkdtemp(prefix="wb299_empty_")
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


def snapshot(runid, pid, started_at, phase, issues, sleep=None, active=None):
    return {
        "v": 1,
        "runid": runid,
        "pid": pid,
        "title": "the #299 fixture run",
        "repo": "owner/runs299",
        "branch": "afk/run-299",
        "plan_agent": "claude",
        "exec_agent": "opencode",
        "started_at": started_at,
        "plan_path": ".ralphy/plan.md",
        "queue": {"total": 3, "order": [71, 72, 73], "stop_before": None},
        "issues": issues,
        "phase": {"active": active, "state": phase, "sleep": sleep, "final_summary": None},
    }


def make_fixture_repo():
    """A throwaway git repo carrying a real plan.md and four runstate documents:
    two live-pid runs, one dead-pid orphan, and one malformed file."""
    d = tempfile.mkdtemp(prefix="wb299_fixture_")
    p = Path(d)
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "plan.md").write_text(
        "# Plan for #71\n\n## Steps\n- [ ] first step\n- [x] second step\n\n## Verify\ncargo test\n",
        encoding="utf-8",
    )
    runstate = p / ".ralphy" / "runstate"
    runstate.mkdir()

    trail = [
        {"number": 71, "title": "the done one", "status": "done", "blocked_by": []},
        {"number": 72, "title": "the executing one", "status": "executing", "blocked_by": []},
        {"number": 73, "title": "the pending one", "status": "pending", "blocked_by": []},
    ]
    live_pid = os.getpid()
    (runstate / f"{RUN_A}.json").write_text(
        json.dumps(snapshot(RUN_A, live_pid, "2026-07-24T10:00:00-03:00", "executing", trail, active=72)),
        encoding="utf-8",
    )
    (runstate / f"{RUN_B}.json").write_text(
        json.dumps(
            snapshot(
                RUN_B,
                live_pid,
                "2026-07-24T11:00:00-03:00",
                "sleeping",
                trail,
                sleep={"reset": "14:30", "target_epoch": int(time.time()) + 7200},
                active=72,
            )
        ),
        encoding="utf-8",
    )
    # Windows PIDs stay far below this: never a live process, so the reader
    # sweeps it and the panel must not list it.
    (runstate / f"{RUN_C}.json").write_text(
        json.dumps(snapshot(RUN_C, 4_000_001, "2026-07-24T09:00:00-03:00", "executing", trail, active=71)),
        encoding="utf-8",
    )
    (runstate / "bad.json").write_text("not json", encoding="utf-8")

    (p / "README.md").write_text("# fixture\n\nThe #299 runs-panel fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb299@example.com"],
        ["git", "config", "user.name", "wb299"],
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
    # after any assets/ui edit or the browser loads yesterday's panel.
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb299_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    runstate_dir = Path(fixture_dir, ".ralphy", "runstate")

    proc = subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        if not wait_listening(BASE):
            # A bare `return` here would skip the exit gate below and report
            # success with ZERO browser assertions run.
            check("daemon listening on 7399", False)
            sys.exit(1)
        check("daemon listening on 7399", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows (KNOWLEDGE.md).
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_timeout(300)

            # The verb itself, over the real wire: a dead-pid run is swept and a
            # malformed one is reported, never hidden.
            reply = page.evaluate(f"""() => window.WBDaemon.observe('runs.list', {{ repo: '{slug}' }})""")
            check("runs.list answers ok", reply.get("status") == "ok", f"{reply.get('status')!r}")
            listed = [r["runid"] for r in reply.get("runs", [])]
            check("runs.list omits the dead-pid run", RUN_C not in listed, f"{listed}")
            unreadable = reply.get("unreadable", [])
            check(
                "runs.list reports the malformed document",
                len(unreadable) == 1 and unreadable[0]["reason"] == "malformed",
                f"{unreadable}",
            )

            # --- scenario 1: the panel hydrates from the snapshots ------------
            # `toggle(slug)` is the project-change hydration path; `toggleRuns()`
            # is the panel-open one. The script exercises BOTH.
            page.evaluate(f"() => {SH}.toggle('{slug}')")
            page.evaluate(f"() => {SH}.toggleRuns()")
            page.wait_for_function(f"() => {SH}.projectRuns().length === 2", timeout=8000)
            page.wait_for_timeout(400)

            n_runs = page.evaluate(f"() => {SH}.projectRuns().length")
            check("picker lists exactly 2 live runs", n_runs == 2, f"got={n_runs}")

            glyphs = page.locator(".trail .trail-ic").all_inner_texts()
            glyphs = [g.strip() for g in glyphs]
            check(
                "issue trail glyphs read done / executing / pending",
                glyphs == ["✅", "⚙️", "○"],
                f"got={glyphs}",
            )

            label = page.evaluate(f"() => {SH}.runPhaseLabel({SH}.currentRun())")
            check("run phase label names the active issue", label == "executing #72", f"got={label!r}")

            prog = page.locator(".run-select-btn .run-prog").inner_text().strip()
            check("progress counter reads completed/queue-total", prog == "1/3", f"got={prog!r}")

            steps = page.locator(".plan-block-steps .plan-md").inner_text()
            check("plan viewer's Steps block carries the repo's plan.md", "first step" in steps, f"got={steps[:80]!r}")

            page.screenshot(path=os.path.join(SHOT_DIR, "299-runs-panel-2026-07-24.png"))

            # both runs are selectable: flip to the sleeping one.
            page.evaluate(f"() => {SH}.selectRun('{RUN_B}')")
            page.wait_for_timeout(300)
            selected = page.evaluate(f"() => {SH}.currentRun()?.runid")
            check("the second concurrent run is selectable", selected == RUN_B, f"got={selected!r}")
            sleep_text = page.locator(".trail-sleep").inner_text()
            check(
                "a run parked on a usage limit shows its reset time",
                "waiting for reset ~14:30" in sleep_text,
                f"got={sleep_text!r}",
            )

            # --- scenario 2: a failed read is never an idle project -----------
            for runid in (RUN_A, RUN_B):
                (runstate_dir / f"{runid}.json").unlink()
            page.evaluate(f"() => {SH}.hydrateRuns()")
            page.wait_for_function(f"() => {SH}.projectRuns().length === 0", timeout=8000)
            page.wait_for_timeout(300)

            runs_error = page.evaluate(f"() => {SH}.runsError")
            check("a failed read sets runsError", bool(runs_error), f"got={runs_error!r}")
            check(".runs-error is visible", page.locator(".runs-error").is_visible())
            empties = [
                page.locator(".runs-empty").nth(i).inner_text()
                for i in range(page.locator(".runs-empty").count())
                if page.locator(".runs-empty").nth(i).is_visible()
            ]
            check(
                'the "No active runs" empty state stays hidden',
                not any("No active runs" in t for t in empties),
                f"visible empties={empties}",
            )

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 12
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("RUNS PANEL FED BY REAL RUNS")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
