"""#330 browser acceptance: the plan is state — step progress reaches the Runs panel.

One Playwright pass over a REAL daemon proving the Steps block renders from the
run-snapshot document's `plan` block, not from a `plan.md` read: every document
below is written by THIS python process, from outside the browser, and no
scenario calls `hydrateRuns()` or clicks anything to make the panel move.

Scenario 1  a document written BEFORE the panel opens shows its ACCUMULATED steps
Scenario 2  a checked step renders ✅ and a noticed step a DIFFERENT glyph, with
            no operator interaction; the noticed row carries `st-noticed`
Scenario 3  deleting `.ralphy/plan.md` leaves the step list populated, keeps the
            last good prose on screen, and is NOT a run-list error
Scenario 3b the PREVIOUS issue's plan is never rendered as this issue's: prose
            keyed by the plan's own trailer, withheld sections, a note naming
            both issues, and the fresh plan arriving unaided
Scenario 4  the planning phase says the plan is being written; "no steps" and
            "could not read" are distinguishable on screen
Scenario 5  the prose sections stay available and the operator's chosen heading
            survives a background push
Scenario 6  step state survives a DAEMON RESTART on the same port, unaided
Scenario 7  a 40-step block scrolls inside its own box, not the run card

Boots a Localhost daemon on 7420 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/330-runs-plan-state-2026-07-26.png.
Run: python crates/ralphy-daemon/tests/wb_runs_330.py   (exit 0 = all pass)
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

# The Windows console's default codepage (cp1252 here) cannot encode the step
# glyphs this script prints in its detail strings; force utf-8 stdout so a
# PASSING assertion never dies on its own detail.
sys.stdout.reconfigure(encoding="utf-8")

PORT = 7420
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_runs_330.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

RUN_A = "01RUN330AAAAAAAAAAAAAA"

# A FINALIZED plan for #72 — the trailer is what a real plan.md carries once the
# planner is done (crates/ralphy-adapter-support/src/resume.rs `plan_trailer`), and
# the panel keys the prose block on it: without it the viewer cannot tell this
# plan from the previous issue's, which is the defect scenario 3b pins.
PLAN_MD = (
    "# Plan for #72\n\n## Steps\n- [x] first step body\n- [x] second step body\n"
    "- [ ] third step body\n\n## Verify\ncargo fmt --check\n\n## Decisions\nnone\n"
    "\n<!-- ralphy-plan: issue=72 -->\n"
)

# The SAME plan text, keyed to the issue BEFORE the active one: what sits on disk
# while a run plans #72 with #71's plan not yet overwritten.
STALE_PLAN_MD = PLAN_MD.replace("# Plan for #72", "# Plan for #71").replace(
    "issue=72", "issue=71"
).replace("cargo fmt --check", "cargo clippy --71-only")

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
    empty = tempfile.mkdtemp(prefix="wb330_empty_")
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
        "title": "the #330 fixture run",
        "repo": "owner/runs330",
        "branch": "afk/run-330",
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


def plan_block(steps, issue=72):
    return {"issue": issue, "steps": steps}


def steps(*pairs):
    return [{"text": t, "status": s} for t, s in pairs]


def make_fixture_repo():
    """A throwaway git repo with a real plan.md and an EMPTY runstate dir."""
    d = tempfile.mkdtemp(prefix="wb330_fixture_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "plan.md").write_text(PLAN_MD, encoding="utf-8")
    (p / ".ralphy" / "runstate").mkdir()
    (p / "README.md").write_text("# fixture\n\nThe #330 plan-state fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb330@example.com"],
        ["git", "config", "user.name", "wb330"],
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


def step_rows(page):
    """Every step row's glyph, text and class, read in ONE evaluate (a DOM handle
    cannot cross back into the sync API)."""
    return page.evaluate(
        "() => Array.from(document.querySelectorAll('.plan-steps li')).map(li => ({"
        " glyph: li.querySelector('.step-ic')?.textContent.trim() || '',"
        " text: li.querySelector('.step-tx')?.textContent.trim() || '',"
        " cls: li.className }))"
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb330_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    runstate = Path(fixture_dir, ".ralphy", "runstate")
    plan_md = Path(fixture_dir, ".ralphy", "plan.md")
    doc = runstate / f"{RUN_A}.json"

    def write(phase, plan, active=72):
        doc.write_text(json.dumps(snapshot(phase, plan, active)), encoding="utf-8")

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

            # --- scenario 1: the run PREDATES the panel -----------------------
            # The document (2 checked + 1 open) exists before the browser ever
            # opens the panel: opening mid-run must show accumulated progress.
            write(
                "executing",
                plan_block(steps(("first step body", "checked"), ("second step body", "checked"), ("third step body", "open"))),
            )
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_timeout(300)
            open_panel(page, slug)
            page.wait_for_function("() => document.querySelectorAll('.plan-steps li').length === 3", timeout=15000)
            rows = step_rows(page)
            check(
                "opening the panel mid-run shows the ACCUMULATED steps",
                [r["text"] for r in rows] == ["first step body", "second step body", "third step body"],
                f"got={[r['text'] for r in rows]}",
            )
            check(
                "their statuses are the ones the document carried",
                [r["cls"] for r in rows] == ["st-checked", "st-checked", "st-open"],
                f"got={[r['cls'] for r in rows]}",
            )
            issue_pill = page.locator(".plan-block-steps .plan-issue").inner_text().strip()
            check("the block names the issue the plan belongs to", issue_pill == "#72", f"got={issue_pill!r}")

            # --- scenario 2: a step advances with NO operator interaction -----
            write(
                "executing",
                plan_block(
                    steps(
                        ("first step body", "checked"),
                        ("second step body", "checked"),
                        ("third step body", "noticed"),
                    )
                ),
            )
            page.wait_for_function(
                "() => (document.querySelectorAll('.plan-steps li')[2]?.className || '').includes('st-noticed')",
                timeout=15000,
            )
            rows = step_rows(page)
            check("a checked step renders ✅ with no operator interaction", rows[0]["glyph"] == "✅", f"got={rows[0]!r}")
            check(
                "a noticed step is visually DISTINCT from a completed one",
                rows[2]["glyph"] != rows[0]["glyph"] and "st-noticed" in rows[2]["cls"],
                f"noticed={rows[2]['glyph']!r} checked={rows[0]['glyph']!r} cls={rows[2]['cls']!r}",
            )

            # --- scenario 3: a plan read failure never blanks the steps -------
            prose_before = page.locator(".plan-block-more .plan-md").inner_text()
            plan_md.unlink()
            # A fresh document write is the push that re-runs the (now failing)
            # plan read; the step list must be untouched by it.
            write(
                "executing",
                plan_block(
                    steps(
                        ("first step body", "checked"),
                        ("second step body", "checked"),
                        ("third step body", "noticed"),
                    )
                ),
                active=72,
            )
            page.wait_for_function(f"() => {SH}.currentRun()?.planReadFailed === true", timeout=15000)
            rows = step_rows(page)
            check(
                "a deleted plan.md leaves the step list POPULATED",
                len(rows) == 3,
                f"got={len(rows)} rows",
            )
            prose_after = page.locator(".plan-block-more .plan-md").inner_text()
            check(
                "the last good plan text stays on screen",
                "cargo fmt --check" in prose_after and prose_after.strip() == prose_before.strip(),
                f"got={prose_after[:60]!r}",
            )
            check(
                "the last-read plan text is still held",
                page.evaluate(f"() => {SH}.currentRun().planMd") != "",
                "",
            )
            runs_error = page.evaluate(f"() => {SH}.runsError")
            check(
                "a plan read failure is NOT reported as a run-list failure",
                runs_error == "" and not page.locator(".runs-error").is_visible(),
                f"runsError={runs_error!r}",
            )
            unread_note = page.locator(".plan-prose-note").inner_text().strip()
            check(
                "the unreadable prose says so",
                "could not read" in unread_note,
                f"got={unread_note!r}",
            )
            # The evidence PNG at the asserting moment: checked + noticed rows on
            # screen together, over the last good prose of a deleted plan.md.
            page.screenshot(path=os.path.join(SHOT_DIR, "330-runs-plan-state-2026-07-26.png"))

            # --- scenario 3b: the PREVIOUS issue's plan is never shown as this --
            # The run is on #72; the file on disk is #71's finalized plan — exactly
            # the state of `.ralphy/plan.md` for the whole planning phase of the
            # next issue. An unkeyed viewer rendered it under #72's chrome.
            plan_md.write_text(STALE_PLAN_MD, encoding="utf-8")
            write(
                "planning",
                plan_block(
                    steps(("first step body", "checked"), ("second step body", "checked")),
                    issue=71,
                ),
                active=72,
            )
            page.wait_for_function(
                f"() => {SH}.currentRun()?.planMd.includes('issue=71')", timeout=15000
            )
            stale = page.evaluate(
                f"() => ({{ prose: document.querySelector('.plan-block-more .plan-md').innerText.trim(),"
                f" headings: {SH}.planHeadings({SH}.currentRun()),"
                f" current: {SH}.planProseIsCurrent({SH}.currentRun()),"
                " note: (document.querySelector('.plan-prose-note')?.innerText || '').trim() })"
            )
            check(
                "the previous issue's plan is NOT rendered as the current one",
                stale["current"] is False
                and stale["prose"] == ""
                and "--71-only" not in stale["prose"],
                f"current={stale['current']} prose={stale['prose'][:60]!r}",
            )
            check(
                "its sections are withheld from the picker too",
                stale["headings"] == [],
                f"headings={stale['headings']}",
            )
            check(
                "the empty block says whose plan is on disk and whose is awaited",
                "#71" in stale["note"] and "#72" in stale["note"],
                f"note={stale['note']!r}",
            )
            # …and the fresh plan lands with no interaction: the write is the push.
            plan_md.write_text(PLAN_MD, encoding="utf-8")
            write(
                "executing",
                plan_block(
                    steps(("first step body", "checked"), ("second step body", "checked")),
                    issue=72,
                ),
                active=72,
            )
            page.wait_for_function(
                "() => document.querySelector('.plan-block-more .plan-md')"
                ".innerText.includes('cargo fmt --check')",
                timeout=15000,
            )
            recovered = page.evaluate(
                "() => (document.querySelector('.plan-prose-note')?.innerText || '').trim()"
            )
            check(
                "the plan for THIS issue appears unaided, and the note steps aside",
                recovered == "",
                f"note={recovered!r}",
            )

            # --- scenario 4: an empty block explains itself -------------------
            write("planning", plan_block([], issue=None), active=72)
            page.wait_for_function(
                "() => (document.querySelector('.plan-steps-note')?.textContent || '').includes('writing the plan')",
                timeout=15000,
            )
            planning_note = page.locator(".plan-steps-note").inner_text().strip()
            check("the planning phase says the plan is being written", "writing the plan" in planning_note, f"got={planning_note!r}")

            write("executing", plan_block([], issue=72), active=72)
            page.wait_for_function(
                "() => (document.querySelector('.plan-steps-note')?.textContent || '').includes('no steps')",
                timeout=15000,
            )
            empty_note = page.locator(".plan-steps-note").inner_text().strip()
            check("a plan with no steps says so", "no steps" in empty_note, f"got={empty_note!r}")
            check(
                "'no steps' and 'could not read' are distinguishable",
                empty_note != unread_note and "could not read" not in empty_note,
                f"{empty_note!r} vs {unread_note!r}",
            )

            # --- scenario 5: the operator's chosen section survives a push ----
            plan_md.write_text(PLAN_MD, encoding="utf-8")
            write(
                "executing",
                plan_block(steps(("first step body", "checked"), ("second step body", "open"))),
            )
            page.wait_for_function(f"() => {SH}.planHeadings({SH}.currentRun()).includes('Verify')", timeout=15000)
            page.evaluate(f"() => {{ {SH}.planSection = 'Verify'; }}")
            page.wait_for_timeout(200)
            # A background push (nobody touched the browser) must not reassign it.
            write(
                "executing",
                plan_block(steps(("first step body", "checked"), ("second step body", "checked"))),
            )
            page.wait_for_function(
                # the length guard is load-bearing: `every` is vacuously true on
                # an empty list, so this would resolve on a mid-replacement frame.
                "() => { const li = Array.from(document.querySelectorAll('.plan-steps li'));"
                " return li.length === 2 && li.every(e => e.className.includes('st-checked')); }",
                timeout=15000,
            )
            section = page.evaluate(f"() => {SH}.planSection")
            options = page.locator(".plan-picker option").all_inner_texts()
            check(
                "the operator's chosen heading survives a background push",
                section == "Verify",
                f"got={section!r}",
            )
            check(
                "the prose sections remain available in the dropdown",
                "Verify" in [o.strip() for o in options],
                f"got={[o.strip() for o in options]}",
            )

            # --- scenario 6: the step state survives a daemon restart ---------
            stop(proc)
            proc = launch(daemon_dir)
            if not wait_listening(BASE):
                check("daemon relistening on the same port", False)
                sys.exit(1)
            check("daemon relistening on the same port", True)
            write(
                "executing",
                plan_block(
                    steps(
                        ("first step body", "checked"),
                        ("second step body", "checked"),
                        ("a step written after the restart", "checked"),
                    )
                ),
            )
            # No reload, no operator call: the reconnect re-sends `runs.watch`.
            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('.plan-steps li .step-tx'))"
                ".some(e => e.textContent.includes('after the restart'))",
                timeout=25000,
            )
            rows = step_rows(page)
            check(
                "the step state returns after a daemon restart, unaided",
                len(rows) == 3 and all("st-checked" in r["cls"] for r in rows),
                f"got={[r['cls'] for r in rows]}",
            )

            # --- scenario 7: a long plan scrolls inside its own block ---------
            write("executing", plan_block(steps(*[(f"step number {i}", "open") for i in range(40)])))
            page.wait_for_function("() => document.querySelectorAll('.plan-steps li').length === 40", timeout=15000)
            geom = page.evaluate(
                "() => { const s = document.querySelector('.plan-steps');"
                " s.scrollTop = 400;"
                " return { steps: s.scrollTop, body: document.querySelector('.runs-body').scrollTop,"
                " picker: document.querySelector('.run-select-btn')?.offsetParent !== null }; }"
            )
            check("a 40-step block scrolls inside itself", geom["steps"] > 0, f"got={geom['steps']}")
            check("the run card does NOT scroll with it", geom["body"] == 0, f"got={geom['body']}")
            check("the run picker stays on screen", geom["picker"] is True, f"got={geom['picker']}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 18
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("PLAN IS STATE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
