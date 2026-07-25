"""#300 browser acceptance: live run updates pushed to the browser, seed retired.

One Playwright pass over a REAL daemon proving the Runs panel advances on its
own — every snapshot document is written by THIS python process, from outside
the browser, and no scenario below calls `hydrateRuns()` to make the panel move
(scenario 3 calls it once, deliberately, to prove the replacement is idempotent).

Scenario 1  a document appearing while the panel is open populates it (no reload)
Scenario 2  a status flip + a plan.md rewrite advance the trail, phase, counter,
            and the plan viewer, with no operator action
Scenario 3  applying the SAME snapshot twice changes nothing (byte-identical
            state JSON across a second `hydrateRuns()`)
Scenario 4  the document disappearing (a run that ended) empties the panel, and
            that is NOT an error state
Scenario 5  the panel survives a browser reload — the state lives on disk
Scenario 6  the panel recovers after a DAEMON RESTART on the same port, with no
            operator action (the 3s-backoff reconnect + its catch-up read)
Scenario 7  in daemon mode the ⚡ control is absent, the seed runs and seed plan
            blocks are unreachable, and the client-side fold is inert
Scenario 8  the static `file://` demo still renders a populated panel and its ⚡
            control still advances a run

Boots a Localhost daemon on 7398 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/300-runs-live-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_runs_300.py   (exit 0 = all pass)
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

PORT = 7398
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_runs_300.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
DEMO_HTML = Path(REPO_ROOT, "crates", "ralphy-daemon", "assets", "ui", "index.html")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

RUN_A = "01RUNAAAAAAAAAAAAAAAAA"
RUN_B = "01RUNBBBBBBBBBBBBBBBBB"

# The state the panel exposes, as ONE string — the idempotence oracle.
STATE_JSON = (
    f"() => JSON.stringify({{ runs: {SH}.projectRuns(), currentRunId: {SH}.currentRunId,"
    f" planSection: {SH}.planSection, runsError: {SH}.runsError }})"
)

PLAN_V1 = "# Plan for #72\n\n## Steps\n- [ ] first step body\n\n## Verify\ncargo fmt --check\n"
PLAN_V2 = "# Plan for #72\n\n## Steps\n- [x] first step body\n- [ ] second step body\n\n## Verify\ncargo fmt --check\n"

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
    empty = tempfile.mkdtemp(prefix="wb300_empty_")
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


def snapshot(runid, phase, issues, active=None):
    return {
        "v": 1,
        "runid": runid,
        "pid": os.getpid(),  # a LIVE pid, so the reader never sweeps it as an orphan
        "title": "the #300 fixture run",
        "repo": "owner/runs300",
        "branch": "afk/run-300",
        "plan_agent": "claude",
        "exec_agent": "opencode",
        "started_at": "2026-07-25T10:00:00-03:00",
        "plan_path": ".ralphy/plan.md",
        "queue": {"total": 3, "order": [71, 72, 73], "stop_before": None},
        "issues": issues,
        "phase": {"active": active, "state": phase, "sleep": None, "final_summary": None},
    }


def trail(status_72):
    return [
        {"number": 71, "title": "the done one", "status": "done", "blocked_by": []},
        {"number": 72, "title": "the active one", "status": status_72, "blocked_by": []},
        {"number": 73, "title": "the pending one", "status": "pending", "blocked_by": []},
    ]


def make_fixture_repo():
    """A throwaway git repo with a real plan.md and an EMPTY runstate dir — the
    panel starts at zero runs and every document below arrives while it is open.
    `.gitignore` hides `.ralphy/`, so the browser leg also pins the exemption."""
    d = tempfile.mkdtemp(prefix="wb300_fixture_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "plan.md").write_text(PLAN_V1, encoding="utf-8")
    (p / ".ralphy" / "runstate").mkdir()
    (p / "README.md").write_text("# fixture\n\nThe #300 live-runs fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb300@example.com"],
        ["git", "config", "user.name", "wb300"],
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


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb300_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    runstate = Path(fixture_dir, ".ralphy", "runstate")
    plan_md = Path(fixture_dir, ".ralphy", "plan.md")
    doc_a = runstate / f"{RUN_A}.json"
    doc_b = runstate / f"{RUN_B}.json"

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
            page.wait_for_timeout(300)

            # --- scenario 1: a run appearing while the panel is open ----------
            open_panel(page, slug)
            page.wait_for_function(f"() => {SH}.projectRuns().length === 0", timeout=8000)
            check("the panel opens empty (no runs on disk yet)", True)

            # Issue 72 starts `planning`, NOT `pending`: `issueState` reads the
            # ACTIVE issue's glyph as ⚙️ for any non-terminal status other than
            # `planning`, so a `pending` fixture would render the scenario-2
            # oracle (✅/⚙️/○) before scenario 2 ever ran.
            doc_a.write_text(json.dumps(snapshot(RUN_A, "planning", trail("planning"), active=72)), encoding="utf-8")
            page.wait_for_function(f"() => {SH}.projectRuns().length === 1", timeout=15000)
            check("a run started while the panel is open appears with NO operator action", True)

            label = page.evaluate(f"() => {SH}.runPhaseLabel({SH}.currentRun())")
            check("the pushed run reads its planning phase", label == "planning #72", f"got={label!r}")
            glyphs = [g.strip() for g in page.locator(".trail .trail-ic").all_inner_texts()]
            check("the trail starts at done / planning / pending", glyphs == ["✅", "🧠", "○"], f"got={glyphs}")

            # --- scenario 2: the panel advances live --------------------------
            plan_md.write_text(PLAN_V2, encoding="utf-8")
            doc_a.write_text(json.dumps(snapshot(RUN_A, "executing", trail("executing"), active=72)), encoding="utf-8")

            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('.trail .trail-ic'))"
                ".map(e => e.textContent.trim()).join(',') === '\u2705,\u2699\ufe0f,\u25cb'",
                timeout=15000,
            )
            glyphs = [g.strip() for g in page.locator(".trail .trail-ic").all_inner_texts()]
            check("the issue trail advances to done / executing / pending", glyphs == ["✅", "⚙️", "○"], f"got={glyphs}")

            page.wait_for_function(
                f"() => {SH}.runPhaseLabel({SH}.currentRun()) === 'executing #72'", timeout=15000
            )
            label = page.evaluate(f"() => {SH}.runPhaseLabel({SH}.currentRun())")
            check("the phase label advances to the executing issue", label == "executing #72", f"got={label!r}")

            prog = page.locator(".run-select-btn .run-prog").inner_text().strip()
            check("the progress counter reads completed/queue-total", prog == "1/3", f"got={prog!r}")

            page.wait_for_function(
                "() => (document.querySelector('.plan-block-steps .plan-md')?.innerText || '')"
                ".includes('second step body')",
                timeout=15000,
            )
            steps = page.locator(".plan-block-steps .plan-md").inner_text()
            check("the plan viewer picks up the rewritten plan.md", "second step body" in steps, f"got={steps[:80]!r}")

            # --- scenario 3: replacement is idempotent ------------------------
            page.wait_for_timeout(600)  # let any in-flight push settle
            before = page.evaluate(STATE_JSON)
            page.evaluate(f"async () => {{ await {SH}.hydrateRuns(); }}")
            page.wait_for_timeout(400)
            after = page.evaluate(STATE_JSON)
            check("applying the same snapshot twice changes nothing", before == after, f"len={len(before)}")

            # --- scenario 4: a run that ends leaves the panel cleanly ---------
            doc_a.unlink()
            page.wait_for_function(f"() => {SH}.projectRuns().length === 0", timeout=15000)
            page.wait_for_timeout(300)
            check("a finished run's document removal empties the panel", True)
            check(
                "an ended run is NOT an error state",
                not page.locator(".runs-error").is_visible(),
                f"runsError={page.evaluate(f'() => {SH}.runsError')!r}",
            )

            # --- scenario 5: the panel survives a browser reload --------------
            doc_a.write_text(json.dumps(snapshot(RUN_A, "executing", trail("executing"), active=72)), encoding="utf-8")
            page.wait_for_timeout(500)
            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_timeout(300)
            open_panel(page, slug)
            page.wait_for_function(f"() => {SH}.projectRuns().length === 1", timeout=15000)
            check("the panel repopulates after a browser reload", True)

            # --- scenario 6: the panel recovers after a daemon restart --------
            stop(proc)
            proc = launch(daemon_dir)
            if not wait_listening(BASE):
                check("daemon relistening on the same port", False)
                sys.exit(1)
            check("daemon relistening on the same port", True)
            doc_b.write_text(json.dumps(snapshot(RUN_B, "planning", trail("pending"), active=72)), encoding="utf-8")
            # No reload, no operator call: the 3s-backoff reconnect re-sends
            # `runs.watch` and does one catch-up read on open.
            page.wait_for_function(f"() => {SH}.projectRuns().length === 2", timeout=20000)
            check("the panel recovers after a daemon restart, unaided", True)

            page.screenshot(path=os.path.join(SHOT_DIR, "300-runs-live-2026-07-25.png"))

            # --- scenario 7: the seed and the fold are unreachable ------------
            check(
                "the ⚡ demo control is not rendered in daemon mode",
                page.locator(".runs-demo").is_visible() is False,
                "",
            )
            keys = page.evaluate(f"() => Object.keys({SH}.runsByProject)")
            check("only the registered repo has runs (no seed project keys)", keys == [slug], f"got={keys}")
            runids = page.evaluate(f"() => {SH}.projectRuns().map(r => r.runid)")
            check(
                "every listed run is a live snapshot, never a seed run",
                all(r.startswith("01RUN") for r in runids),
                f"got={runids}",
            )
            plan_texts = page.locator(".plan-md").all_inner_texts()
            check(
                "no seed plan block reaches the daemon-served viewer",
                not any("Walking skeleton" in t for t in plan_texts),
                f"blocks={len(plan_texts)}",
            )
            before = page.evaluate(STATE_JSON)
            page.evaluate(
                f"() => {SH}.applyRunEvent({{ type: 'dev.ralphy.issue.closed',"
                f" runid: '{RUN_A}', data: {{ number: 72 }} }})"
            )
            check("the client-side fold is inert in daemon mode", page.evaluate(STATE_JSON) == before)
            page.evaluate(f"() => {SH}.demoTick()")
            check("the demo advance is inert in daemon mode", page.evaluate(STATE_JSON) == before)

            # --- scenario 8: the static file:// demo still works --------------
            page.goto(DEMO_HTML.as_uri())
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_timeout(300)
            open_panel(page, "fincal")
            page.wait_for_timeout(400)
            n = page.evaluate(f"() => {SH}.projectRuns().length")
            check("the file:// demo still renders a populated panel", n == 2, f"got={n}")
            check("the ⚡ demo control IS rendered under file://", page.locator(".runs-demo").is_visible())
            glyph_count = page.locator(".trail .trail-ic").count()
            check("the demo trail still renders its issues", glyph_count == 14, f"got={glyph_count}")
            open_steps = page.evaluate(f"() => ({SH}.currentRun().planMd.match(/-\\s+\\[ \\]/g) || []).length")
            page.evaluate(f"() => {SH}.demoTick()")
            now_open = page.evaluate(f"() => ({SH}.currentRun().planMd.match(/-\\s+\\[ \\]/g) || []).length")
            check(
                "the demo advance still ticks a step under file://",
                open_steps > 0 and now_open == open_steps - 1,
                f"{open_steps} -> {now_open}",
            )

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 20
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("RUNS LIVE, SEED RETIRED")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
