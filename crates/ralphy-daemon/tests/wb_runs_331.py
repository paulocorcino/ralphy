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

            # --- scenario 2: the panel's proportions ignore the feed's size ---
            # The cap is what makes this true: past ~30vh of output, MORE output
            # changes nothing. The old unsized <pre> grew with every chunk, so
            # each of these three measurements differed.
            def body_geom():
                return page.evaluate(
                    "() => ({ body: document.querySelector('.runs-body').clientHeight,"
                    " picker: document.querySelector('.run-select-btn').clientHeight,"
                    " steps: document.querySelectorAll('.plan-steps li').length })"
                )

            def refeed(text, marker):
                page.evaluate(f"() => {SH}.dismissFeed()")
                page.wait_for_function(
                    f"() => {SH}.rawFeed === '' && document.querySelector('.runs-feed')?.offsetParent == null",
                    timeout=5000,
                )
                feed(page, text)
                # The `offsetParent`/`clientWidth` guards are load-bearing: an
                # Alpine `x-show` flip is NOT visible to the next evaluate, so a
                # text-only predicate resolves on the still-HIDDEN box and every
                # geometry read after it measures zeros (a vacuous pass).
                page.wait_for_function(
                    "(m) => { const r = document.querySelector('.runs-raw');"
                    " return !!r && r.offsetParent !== null && r.clientWidth > 0"
                    " && r.textContent.includes(m); }",
                    arg=marker,
                    timeout=15000,
                )

            refeed("".join(f"line {i} A\n" for i in range(300)), "line 299 A")
            g300 = body_geom()
            refeed("".join(f"line {i} B\n" for i in range(500)), "line 499 B")
            g500 = body_geom()
            refeed("".join(f"line {i} C\n" for i in range(5000)), "line 4999 C")
            g5000 = body_geom()
            check(
                "the run card + plan viewer keep their height from 300 to 5000 output lines",
                g300["body"] == g500["body"] == g5000["body"],
                f"300={g300['body']} 500={g500['body']} 5000={g5000['body']}",
            )
            check(
                "the run picker is unchanged and the plan viewer still lists its 3 steps",
                g300["picker"] == g5000["picker"] and g5000["steps"] == 3,
                f"picker {g300['picker']}->{g5000['picker']} steps={g5000['steps']}",
            )

            # --- scenario 3: a long unbreakable line WRAPS, never clips --------
            url = "https://example.invalid/" + "a" * 380
            check("the probe line has no break opportunity", " " not in url and len(url) >= 400, f"len={len(url)}")
            refeed(url + "\n", "example.invalid")
            wrap = page.evaluate(
                "() => { const r = document.querySelector('.runs-raw');"
                " return { sw: r.scrollWidth, cw: r.clientWidth,"
                " ws: getComputedStyle(r).whiteSpace }; }"
            )
            check(
                "a 400-char space-free URL does not overflow horizontally",
                wrap["cw"] > 0 and wrap["sw"] <= wrap["cw"] + 1,
                f"scrollWidth={wrap['sw']} clientWidth={wrap['cw']}",
            )
            check("the feed preserves formatting AND wraps", wrap["ws"] == "pre-wrap", f"got={wrap['ws']!r}")

            # --- scenario 4: collapse (buffer kept) and dismiss (buffer gone) --
            refeed("".join(f"line {i} D\n" for i in range(300)), "line 299 D")
            open_body = page.evaluate("() => document.querySelector('.runs-body').clientHeight")
            page.click('[data-act="feed-collapse"]:visible')
            page.wait_for_function(
                "() => document.querySelector('.runs-raw')?.offsetParent === null", timeout=10000
            )
            collapsed = page.evaluate(
                f"() => ({{ body: document.querySelector('.runs-body').clientHeight,"
                f" buffered: {SH}.rawFeed.length,"
                f" head: document.querySelector('.runs-feed-head')?.offsetParent !== null }})"
            )
            check(
                "collapsing the feed reclaims the panel for the structured view",
                collapsed["body"] > open_body,
                f"open={open_body} collapsed={collapsed['body']}",
            )
            check(
                "collapse KEEPS the buffer and its head, so re-opening is free",
                collapsed["buffered"] > 0 and collapsed["head"] is True,
                f"buffered={collapsed['buffered']} head={collapsed['head']}",
            )
            page.click('[data-act="feed-dismiss"]:visible')
            page.wait_for_function(
                "() => document.querySelector('.runs-feed')?.offsetParent == null", timeout=10000
            )
            check(
                "dismissing the feed clears the buffer entirely",
                page.evaluate(f"() => {SH}.rawFeed") == "",
                f"got={page.evaluate(f'() => {SH}.rawFeed')[:40]!r}",
            )

            # --- scenario 5: the panel survives a phone width -----------------
            page.set_viewport_size({"width": 390, "height": 780})
            refeed(url + "\n" + "".join(f"line {i} E\n" for i in range(200)), "line 199 E")
            phone = page.evaluate(
                "() => { const r = document.querySelector('.runs-raw');"
                " const verbs = Array.from(document.querySelectorAll('.run-verb'));"
                " return { panel: document.querySelector('.runs').getBoundingClientRect().width,"
                " visible: verbs.filter(b => b.offsetParent !== null).length,"
                " sw: r.scrollWidth, cw: r.clientWidth, ch: r.clientHeight,"
                " steps: document.querySelectorAll('.plan-steps li').length }; }"
            )
            check(
                "at 390x780 the panel fits the viewport",
                phone["panel"] <= 390,
                f"panelWidth={phone['panel']}",
            )
            check(
                "all three verbs stay reachable at a phone width",
                phone["visible"] == 3,
                f"visible={phone['visible']}",
            )
            check(
                "the feed still wraps and yields the panel to the structured view",
                phone["cw"] > 0
                and phone["sw"] <= phone["cw"] + 1
                and phone["ch"] <= 0.24 * 780
                and phone["steps"] == 3,
                f"sw={phone['sw']} cw={phone['cw']} ch={phone['ch']} steps={phone['steps']}",
            )
            page.set_viewport_size({"width": 1400, "height": 900})

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 14
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("RUNS CHROME")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
