"""Browser acceptance: the board surfaces, shows and discards a READY plan.

A finalized `.ralphy/plan.md` is executed by the NEXT run — the planner's
`<!-- ralphy-plan: issue=N -->` trailer is the resume signal
(crates/ralphy-adapter-support/src/resume.rs), so `ralphy run` skips planning and
goes straight to that plan. Until this slice the board never said so: changing
your mind about a planned issue meant deleting the file by hand, outside the
workbench and every guard in it.

One Playwright pass over a REAL daemon. `board.list` is the ONLY stubbed verb (a
throwaway fixture repo has no tracker); the plan is read through the real
`file.read` and discarded through the real `plan.discard`, so every assertion
below about the plan is an assertion about bytes on disk.

Scenario 1  a finalized plan for an OPEN issue puts a `plan ready` pill on that
            issue's card and a chip in the board head, with no reload
Scenario 2  the pill opens the plan modal: the verdict banner, the step count and
            the plan's own prose
Scenario 3  a plan is shown against the issue its TRAILER names, and no other
Scenario 4  a `Feasible: no` bundle plan warns on the card and states the
            planner's reasons in the modal — the case that most needs a human
Scenario 5  an UNFINISHED plan (no trailer) is offered to nobody
Scenario 6  Discard is gated while a run holds the repo, stating the reason
Scenario 7  Discard removes the file from DISK after the design-system confirm,
            and both affordances leave the board unaided

Boots a Localhost daemon on 7442 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/plan-discard-2026-07-29.png.
Run: python crates/ralphy-daemon/tests/wb_plan_discard.py   (exit 0 = all pass)
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

# The Windows console's default codepage cannot encode the glyphs this script
# prints in its detail strings; force utf-8 so a PASSING assertion never dies on
# its own detail.
sys.stdout.reconfigure(encoding="utf-8")

PORT = 7442
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_plan_discard.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

RUN_A = "01PLANDISCARDAAAAAAAAA"

# A finalized, FEASIBLE plan for #72 — one open step left.
READY_PLAN = """# Plan for #72: the fixture issue

## Feasible: yes
The ground is present and the change is one seam wide.

## Steps
- [x] the done step
- [ ] the open step

## Verify
cargo fmt --check

<!-- ralphy-plan: issue=72 -->
"""

# The planner's refusal: a bundle verdict, which the runner labels `needs-split`
# by keying on the literal word "bundle" (ralphy-core `handoff::is_bundle_reason`).
# No open steps, which is the verdict the runner actually reads
# (`plan::count_open_steps`).
BUNDLE_PLAN = """# Plan for #72: too much at once

## Feasible: no
The issue bundles four separate tasks; split it into #90..#93 before running.

## Steps

<!-- ralphy-plan: issue=72 -->
"""

# The SAME plan keyed to another open issue, for the "shown against the issue it
# names" leg.
OTHER_PLAN = READY_PLAN.replace("#72", "#71").replace("issue=72", "issue=71")

# A plan the planner has not finished: no trailer, so it belongs to nobody yet.
UNFINISHED_PLAN = READY_PLAN.replace("\n<!-- ralphy-plan: issue=72 -->\n", "")

# `board.list` is the ONLY verb stubbed — a throwaway repo has no tracker. Every
# other verb DELEGATES to the real transport, which is what makes `file.read` and
# `plan.discard` below real round trips against the daemon.
SPY_JS = """
() => {
  const real = window.WBDaemon.observe.bind(window.WBDaemon);
  const row = (n, title, state) => ({
    number: n, title, state, labels: ["ready-for-agent"], assignees: [],
    blocked_by: [], created: "2026-07-20T10:00:00Z", updated: "2026-07-28T10:00:00Z",
  });
  window.WBDaemon.observe = (verb, payload) => {
    if (verb === "board.list") {
      return Promise.resolve({
        status: "ok",
        board: {
          issues: [
            row(71, "the other open one", "open"),
            row(72, "the planned one", "open"),
            row(73, "the closed one", "closed"),
          ],
          labels: [{ name: "ready-for-agent", color: "0E8A16" }],
        },
      });
    }
    return real(verb, payload);
  };
}
"""

# `.kc-plan` exists on EVERY card — it is `x-show`-gated, so the hidden ones are
# `display:none` and still in the DOM. A bare `querySelector('.kc-plan')` therefore
# answers about the FIRST card, not about the card that has the plan; every wait
# and click below has to speak about a VISIBLE pill.
VISIBLE_PILL_EXPR = "[...document.querySelectorAll('.kc-plan')].some((e) => e.offsetParent !== null)"
VISIBLE_PILL = f"() => {VISIBLE_PILL_EXPR}"

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
    empty = tempfile.mkdtemp(prefix="wbplan_empty_")
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


def snapshot(runid=RUN_A):
    """A live run holding this repo — the lock derivation the write controls and
    the discard both read (`runs.list`, ADR-0047 §9)."""
    return {
        "v": 1,
        "runid": runid,
        "pid": os.getpid(),  # a LIVE pid, so the reader never sweeps it as an orphan
        "title": "the plan-discard fixture run",
        "repo": "owner/planfix",
        "branch": "afk/plan-fix",
        "plan_agent": "claude",
        "exec_agent": "claude",
        "started_at": "2026-07-29T10:00:00-03:00",
        "plan_path": ".ralphy/plan.md",
        "queue": {"total": 1, "order": [72], "stop_before": None},
        "issues": [{"number": 72, "title": "the planned one", "status": "executing", "blocked_by": []}],
        "phase": {"state": "executing", "active": 72, "sleep": None},
        "plan": {"issue": 72, "steps": [{"text": "the open step", "status": "open"}]},
    }


def make_fixture_repo():
    """A throwaway git repo with a `.ralphy/` holding a plan and one other file —
    the other file is how "discard removes the plan and nothing else" is asserted
    on disk rather than in the reply."""
    d = tempfile.mkdtemp(prefix="wbplan_fixture_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "plan.md").write_text(READY_PLAN, encoding="utf-8")
    (p / ".ralphy" / "issue.json").write_text('{"number":72}', encoding="utf-8")
    (p / ".ralphy" / "runstate").mkdir()
    (p / "README.md").write_text("# fixture\n\nThe plan-discard fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbplan@example.com"],
        ["git", "config", "user.name", "wbplan"],
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


def card_plan(page, number):
    """The plan pill on ONE card: its words and whether it warns."""
    return page.evaluate(
        "(n) => { const card = [...document.querySelectorAll('.kanban-card')]"
        "   .find((c) => c.querySelector('.kc-num')?.textContent.trim() === '#' + n);"
        "  if (!card) return { card: false };"
        "  const pill = card.querySelector('.kc-plan');"
        "  return { card: true, shown: !!pill && pill.offsetParent !== null,"
        "    text: (pill?.innerText || '').trim(), warn: !!pill?.classList.contains('warn') }; }",
        number,
    )


def head_chip(page):
    return page.evaluate(
        "() => { const c = document.querySelector('.kanban-plan-chip');"
        " return { shown: !!c && c.offsetParent !== null, text: (c?.innerText || '').trim(),"
        "   warn: !!c?.classList.contains('warn') }; }"
    )


def reload_plan(page, slug):
    """Re-read the plan the way every board trigger does, then settle."""
    page.evaluate(f"(s) => {SH}.loadPlan(s)", slug)
    page.wait_for_timeout(350)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbplan_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    plan_md = Path(fixture_dir, ".ralphy", "plan.md")
    other_file = Path(fixture_dir, ".ralphy", "issue.json")
    doc = Path(fixture_dir, ".ralphy", "runstate", f"{RUN_A}.json")

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            # A bare `return` would skip the exit gate below and report success
            # with ZERO browser assertions run.
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        # …and it must be OUR daemon: if anything already held the port, `launch`
        # failed to bind while `wait_listening` answered True against the foreign
        # one, which would surface later as an opaque timeout.
        if proc.poll() is not None:
            check(f"the daemon WE launched owns {PORT}", False, f"exited rc={proc.returncode}")
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1500, "height": 950})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(SPY_JS)

            # --- scenario 1: the plan announces itself on the board -----------
            page.evaluate(f"(s) => {SH}.toggle(s)", slug)
            page.evaluate(f"() => {SH}.toggleKanban()")
            page.wait_for_function(
                "() => document.querySelectorAll('.kanban-card').length === 3", timeout=15000
            )
            page.wait_for_function(VISIBLE_PILL, timeout=15000)
            pill72 = card_plan(page, 72)
            pill71 = card_plan(page, 71)
            chip = head_chip(page)
            check(
                "the planned issue's card says a plan is ready",
                pill72["shown"] is True and "plan ready" in pill72["text"] and pill72["warn"] is False,
                f"got={pill72!r}",
            )
            check(
                "no other card claims the plan",
                pill71["card"] is True and pill71["shown"] is False,
                f"got={pill71!r}",
            )
            check(
                "the board head names the plan's issue, for the card it cannot reach",
                chip["shown"] is True and "#72" in chip["text"],
                f"got={chip!r}",
            )

            # --- scenario 2: the modal is the plan, not a summary of it --------
            page.click(".kc-plan:visible")
            page.wait_for_function(
                "() => document.querySelector('.plan-modal')?.offsetParent !== null", timeout=10000
            )
            modal = page.evaluate(
                "() => ({ title: document.querySelector('.plan-modal .modal-title').innerText.trim(),"
                " verdict: document.querySelector('.plan-verdict-head b').innerText.trim(),"
                " warn: document.querySelector('.plan-verdict').classList.contains('warn'),"
                " steps: document.querySelector('.plan-verdict-steps').innerText.trim(),"
                " why: document.querySelector('.plan-verdict-why').innerText.trim(),"
                " doc: document.querySelector('.plan-doc').innerText })"
            )
            check(
                "the modal names the issue whose plan it shows",
                modal["title"] == "Plan · #72",
                f"got={modal['title']!r}",
            )
            check(
                "a feasible plan reads as feasible, not as a warning",
                modal["verdict"] == "Feasible: yes" and modal["warn"] is False,
                f"verdict={modal['verdict']!r} warn={modal['warn']}",
            )
            check(
                "the banner counts the OPEN steps, the runner's own measure",
                modal["steps"] == "1 of 2 steps open",
                f"got={modal['steps']!r}",
            )
            check(
                "the planner's reason and the plan's own prose are both on screen",
                "one seam wide" in modal["why"] and "the open step" in modal["doc"],
                f"why={modal['why'][:50]!r} doc={modal['doc'][:60]!r}",
            )
            page.click(".plan-modal .modal-x")
            page.wait_for_timeout(250)

            # --- scenario 3: shown against the issue the TRAILER names --------
            plan_md.write_text(OTHER_PLAN, encoding="utf-8")
            reload_plan(page, slug)
            moved72 = card_plan(page, 72)
            moved71 = card_plan(page, 71)
            check(
                "rewriting the plan for another issue moves the pill to THAT card",
                moved71["shown"] is True and moved72["shown"] is False,
                f"71={moved71!r} 72={moved72!r}",
            )

            # --- scenario 4: the verdict a human has to see --------------------
            plan_md.write_text(BUNDLE_PLAN, encoding="utf-8")
            reload_plan(page, slug)
            bundle_pill = card_plan(page, 72)
            bundle_chip = head_chip(page)
            check(
                "a bundle plan WARNS on the card instead of inviting a run",
                bundle_pill["shown"] is True
                and "needs split" in bundle_pill["text"]
                and bundle_pill["warn"] is True,
                f"got={bundle_pill!r}",
            )
            check(
                "the head chip warns with it",
                bundle_chip["warn"] is True,
                f"got={bundle_chip!r}",
            )
            page.click(".kc-plan:visible")
            page.wait_for_function(
                "() => document.querySelector('.plan-modal')?.offsetParent !== null", timeout=10000
            )
            refusal = page.evaluate(
                "() => ({ verdict: document.querySelector('.plan-verdict-head b').innerText.trim(),"
                " warn: document.querySelector('.plan-verdict').classList.contains('warn'),"
                " chip: (document.querySelector('.plan-verdict-chip')?.innerText || '').trim(),"
                " why: document.querySelector('.plan-verdict-why').innerText.trim(),"
                " steps: document.querySelector('.plan-verdict-steps').innerText.trim() })"
            )
            check(
                "the modal states the refusal, its split recommendation and its reasons",
                refusal["verdict"] == "Feasible: no"
                and refusal["warn"] is True
                and refusal["chip"] == "needs split"
                and "#90..#93" in refusal["why"],
                f"got={refusal!r}",
            )
            check(
                "a refusal has no open steps, which is what the runner reads",
                refusal["steps"] == "0 of 0 steps open",
                f"got={refusal['steps']!r}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "plan-discard-2026-07-29.png"))
            page.click(".plan-modal .modal-x")
            page.wait_for_timeout(250)

            # --- scenario 5: an unfinished plan belongs to nobody --------------
            plan_md.write_text(UNFINISHED_PLAN, encoding="utf-8")
            reload_plan(page, slug)
            half = card_plan(page, 72)
            check(
                "a plan with no trailer is offered to nobody — it is not a plan yet",
                half["shown"] is False and head_chip(page)["shown"] is False,
                f"pill={half!r} chip={head_chip(page)!r}",
            )

            # --- scenario 6: a live run owns the plan it is executing ----------
            plan_md.write_text(READY_PLAN, encoding="utf-8")
            reload_plan(page, slug)
            doc.write_text(json.dumps(snapshot()), encoding="utf-8")
            page.wait_for_function(f"() => {SH}.writeLocked() === true", timeout=15000)
            page.click(".kc-plan:visible")
            page.wait_for_function(
                "() => document.querySelector('.plan-modal')?.offsetParent !== null", timeout=10000
            )
            gated = page.evaluate(
                "() => { const b = document.querySelector('[data-act=\"plan-discard\"]');"
                " return { disabled: b.disabled, title: b.title }; }"
            )
            check(
                "Discard is refused while a run holds the repo, and says why",
                gated["disabled"] is True and "run" in gated["title"].lower(),
                f"got={gated!r}",
            )
            doc.unlink()
            page.wait_for_function(
                "() => document.querySelector('[data-act=\"plan-discard\"]')?.disabled === false",
                timeout=15000,
            )
            check("the lock's release re-enables Discard, unaided", True, "")

            # --- scenario 7: the discard reaches the DISK ----------------------
            page.click('[data-act="plan-discard"]')
            page.wait_for_function(
                "() => document.querySelector('.confirm-modal')?.offsetParent !== null", timeout=10000
            )
            confirm_text = page.evaluate(
                "() => document.querySelector('.confirm-body').innerText.trim()"
            )
            check(
                "the confirm names the issue and what happens next",
                "#72" in confirm_text and "from scratch" in confirm_text,
                f"got={confirm_text!r}",
            )
            # Cancel first: a destructive act must be abandonable, and the file
            # must still be there afterwards.
            page.click(".confirm-modal .btn:not(.danger):not(.accent)")
            page.wait_for_timeout(300)
            check(
                "cancelling the confirm leaves the plan on disk",
                plan_md.exists(),
                "",
            )
            page.click('[data-act="plan-discard"]')
            page.wait_for_function(
                "() => document.querySelector('.confirm-modal')?.offsetParent !== null", timeout=10000
            )
            page.click(".confirm-modal .btn.danger")
            page.wait_for_function(f"() => !({VISIBLE_PILL_EXPR})", timeout=15000)
            check(
                "the plan is gone from DISK, not just from the board",
                not plan_md.exists(),
                f"still there={plan_md.exists()}",
            )
            check(
                "nothing else in .ralphy was touched",
                other_file.exists() and Path(fixture_dir, ".ralphy", "runstate").is_dir(),
                "",
            )
            after = page.evaluate(
                f"() => ({{ pill: {VISIBLE_PILL_EXPR},"
                " chip: !!document.querySelector('.kanban-plan-chip')"
                "   && document.querySelector('.kanban-plan-chip').offsetParent !== null,"
                " modal: !!document.querySelector('.plan-modal')"
                "   && document.querySelector('.plan-modal').offsetParent !== null })"
            )
            check(
                "the board drops both affordances and closes the modal, unaided",
                after["pill"] is False and after["chip"] is False and after["modal"] is False,
                f"got={after!r}",
            )
            # …and a second discard is refused rather than reported as a success.
            second = page.evaluate(
                f"async (s) => {{ const r = await window.WBDaemon.write('plan.discard', {{ repo: s }});"
                " return r; }",
                slug,
            )
            check(
                "discarding a plan that is not there answers `not found`",
                second.get("status") == "error" and second.get("reason") == "not found",
                f"got={second!r}",
            )

            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} passed", flush=True)
    sys.exit(0 if all(results) and results else 1)


if __name__ == "__main__":
    main()
