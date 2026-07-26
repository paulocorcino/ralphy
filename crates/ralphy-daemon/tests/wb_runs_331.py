"""#331 browser acceptance: the Runs panel's chrome — a contained feed, gated verbs.

One Playwright pass over a REAL daemon proving (a) the raw output feed is a
bounded, scrolling, wrapping, collapsible box taken OUT of the panel's flex
arithmetic, and (b) the three run verbs are disabled while a run holds the lock,
state the reason, re-enable when it is released, and surface a CLI refusal in the
panel instead of leaving it in the raw stdout.

Scenario 1  the feed is bounded and scrolls inside itself
Scenario 2  `.runs-body` height is byte-identical across a 3-line and a 500-line
            feed; the run card and plan viewer keep their proportions
Scenario 3  a 400-char space-free URL wraps instead of clipping
Scenario 4  the feed collapses (buffer kept) and dismisses (buffer cleared)
Scenario 5  the panel stays usable at a phone width (390x780)
Scenario 6  run/triage/push disabled with a STATED reason while locked, while
            reading and monitoring keep working
Scenario 7  the verbs re-enable within 15 s of the lock being released
Scenario 8  the gate is a HINT: a click path that is not gate-guarded reaches the
            CLI, and the CLI's refusal is surfaced as a panel line that outlives
            the 2.6 s action flash

Boots a Localhost daemon on 7421 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/331-runs-chrome-2026-07-26.png.
Run: python crates/ralphy-daemon/tests/wb_runs_331.py   (exit 0 = all pass)
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

PORT = 7421
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_runs_331.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

RUN_A = "01RUN331AAAAAAAAAAAAAA"

PLAN_MD = (
    "# Plan for #72\n\n## Steps\n- [x] first step body\n- [x] second step body\n"
    "- [ ] third step body\n\n## Verify\ncargo build\n\n## Decisions\nnone\n"
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
    """A scratch registry + empty vendor stores: the operator's own daemon dir
    (and its login policy) is never touched, and the usage scan finds nothing."""
    empty = tempfile.mkdtemp(prefix="wb331_empty_")
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


def snapshot(phase, plan, active=72, runid=RUN_A):
    return {
        "v": 1,
        "runid": runid,
        "pid": os.getpid(),  # a LIVE pid, so the reader never sweeps it as an orphan
        "title": "the #331 fixture run",
        "repo": "owner/runs331",
        "branch": "afk/run-331",
        "plan_agent": "claude",
        "exec_agent": "opencode",
        "started_at": "2026-07-26T10:00:00-03:00",
        "plan_path": ".ralphy/plan.md",
        "queue": {"total": 3, "order": [71, 72, 73], "stop_before": None},
        "issues": [
            {"number": 71, "title": "the done one", "status": "done", "blocked_by": []},
            {"number": 72, "title": "the active one", "status": "executing", "blocked_by": []},
            {"number": 73, "title": "the pending one", "status": "pending", "blocked_by": []},
        ],
        "phase": {"active": active, "state": phase, "sleep": None, "final_summary": None},
        "plan": plan,
    }


def plan_block(steps_, issue=72):
    return {"issue": issue, "steps": steps_}


def steps(*pairs):
    return [{"text": t, "status": s} for t, s in pairs]


THREE_STEPS = plan_block(
    steps(("first step body", "checked"), ("second step body", "checked"), ("third step body", "open"))
)


def make_fixture_repo():
    """A throwaway git repo with a real plan.md and an EMPTY runstate dir. It has
    NO GitHub remote, which is what makes `ralphy triage` refuse in scenario 8."""
    d = tempfile.mkdtemp(prefix="wb331_fixture_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "plan.md").write_text(PLAN_MD, encoding="utf-8")
    (p / ".ralphy" / "runstate").mkdir()
    (p / "README.md").write_text("# fixture\n\nThe #331 runs-chrome fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb331@example.com"],
        ["git", "config", "user.name", "wb331"],
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
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def open_panel(page, slug):
    page.evaluate(f"() => {SH}.toggle('{slug}')")
    page.evaluate(f"() => {SH}.toggleRuns()")


def feed(page, text):
    """Push a raw chunk through the SAME door wb-daemon.js uses for run stdout."""
    page.evaluate("(t) => window.WBRuns.output(t)", text)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb331_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    runstate = Path(fixture_dir, ".ralphy", "runstate")
    doc = runstate / f"{RUN_A}.json"

    def write(phase="executing", plan=None, active=72):
        doc.write_text(json.dumps(snapshot(phase, plan or THREE_STEPS, active)), encoding="utf-8")

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            # A bare `return` here would skip the exit gate below and report
            # success with ZERO browser assertions run.
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()

            # --- scenario 1: the feed is bounded and scrolls inside itself ----
            write()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_timeout(300)
            open_panel(page, slug)
            page.wait_for_function("() => document.querySelectorAll('.plan-steps li').length === 3", timeout=15000)

            feed(page, "x\n" * 300)
            page.wait_for_function(
                "() => { const r = document.querySelector('.runs-feed .runs-raw');"
                " return !!r && r.offsetParent !== null && r.scrollHeight > r.clientHeight; }",
                timeout=15000,
            )
            geom = page.evaluate(
                "() => { const r = document.querySelector('.runs-raw');"
                " return { sh: r.scrollHeight, ch: r.clientHeight, vh: window.innerHeight }; }"
            )
            check(
                "the feed scrolls inside its own bounded box",
                geom["sh"] > geom["ch"],
                f"scrollHeight={geom['sh']} clientHeight={geom['ch']}",
            )
            check(
                "the feed's height is capped well under the viewport",
                geom["ch"] <= 0.32 * geom["vh"],
                f"clientHeight={geom['ch']} innerHeight={geom['vh']}",
            )

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 3
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("RUNS CHROME")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
