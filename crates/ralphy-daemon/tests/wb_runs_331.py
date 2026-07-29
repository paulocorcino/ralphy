"""#331 browser acceptance: the Runs panel's chrome — a contained feed, gated verbs.

One Playwright pass over a REAL daemon proving (a) the raw output feed arrives
COLLAPSED and is a bounded, scrolling, wrapping box taken OUT of the panel's flex
arithmetic, and (b) the three run verbs are disabled while a run holds the lock,
state the reason IN THEIR TITLE, re-enable when it is released, and surface a CLI
refusal in the panel instead of leaving it in the raw stdout.

Scenario 0  the feed arrives collapsed: the head appears with the first chunk,
            the bytes only after the operator asks (the panel's job is the
            structured view)
Scenario 1  the feed is bounded and scrolls inside itself
Scenario 2  `.runs-body` height is identical across a 300-, 500- and 5000-line
            feed (all past the cap); the run card and plan viewer keep theirs
Scenario 3  a 400-char space-free URL wraps instead of clipping
Scenario 4  the feed collapses (buffer kept) and dismisses (buffer cleared)
Scenario 5  the panel stays usable at a phone width (390x780)
Scenario 6  run/triage/push disabled while locked, each stating the reason in its
            own title and NOWHERE else on the toolbar, while reading and
            monitoring keep working
Scenario 7  the verbs re-enable within 15 s of the lock being released
Scenario 8  a click path that is not gate-guarded reaches the CLI, and the CLI's
            refusal is surfaced as a panel line that outlives the 2.6 s action
            flash (see app.js `verbLocked` on what the gate does and does not do)

Boots a Localhost daemon on 7421 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/331-runs-chrome-2026-07-26.png.
Run: python crates/ralphy-daemon/tests/wb_runs_331.py   (exit 0 = all pass)
"""

import json
import os
import re
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


def expand(page):
    """Open the feed the way the operator does — the box is collapsed by default,
    so every geometry assertion below has to ask for the bytes first."""
    page.wait_for_function(
        "() => document.querySelector('.runs-feed-head')?.offsetParent !== null", timeout=15000
    )
    if page.evaluate("() => document.querySelector('.runs-raw')?.offsetParent === null"):
        page.click('[data-act="feed-collapse"]:visible')


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
        # …and it must be OUR daemon. If anything already held 7421, `launch()`
        # would have failed to bind while `wait_listening` answered True against
        # the foreign one — surfacing later as an opaque wait_for_function
        # timeout instead of "port in use".
        if proc.poll() is not None:
            check(f"the daemon WE launched owns {PORT}", False, f"exited rc={proc.returncode}")
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

            # --- scenario 0: the feed arrives COLLAPSED ------------------------
            # The default is the whole point: a box that opens itself takes up to
            # 30vh of the panel on every verb click. The head must still arrive
            # with the first chunk, or the buffer would be silent rather than
            # opt-in.
            body_before_feed = page.evaluate("() => document.querySelector('.runs-body').clientHeight")
            feed(page, "x\n" * 300)
            page.wait_for_function(
                "() => document.querySelector('.runs-feed-head')?.offsetParent !== null", timeout=15000
            )
            arrival = page.evaluate(
                f"() => ({{ head: document.querySelector('.runs-feed-head')?.offsetParent !== null,"
                " raw: document.querySelector('.runs-raw')?.offsetParent !== null,"
                f" buffered: {SH}.rawFeed.length,"
                f" open: {SH}.rawFeedOpen,"
                " body: document.querySelector('.runs-body').clientHeight })"
            )
            check(
                "the first output chunk raises the feed HEAD but not the bytes",
                arrival["head"] is True and arrival["raw"] is False and arrival["open"] is False,
                f"head={arrival['head']} raw={arrival['raw']} open={arrival['open']}",
            )
            check(
                "the buffer is held while collapsed, so nothing is lost by the default",
                arrival["buffered"] > 0,
                f"buffered={arrival['buffered']}",
            )
            check(
                "an arriving feed does not take the structured view's height",
                arrival["body"] >= body_before_feed - 24,
                f"before={body_before_feed} after={arrival['body']}",
            )

            # --- scenario 1: the feed is bounded and scrolls inside itself ----
            expand(page)
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
                # `dismissFeed()` re-collapses (that IS the default), so the box
                # has to be re-opened before anything measures it.
                expand(page)
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
            page.evaluate(f"() => {SH}.dismissFeed()")

            # --- scenario 6: the verbs are gated while the lock is held -------
            # The live snapshot document IS the lock derivation (`runs.list`,
            # the same one the Changes panel's write controls use).
            page.wait_for_function(f"() => {SH}.verbLocked() === true", timeout=15000)
            verbs = page.evaluate(
                "() => Array.from(document.querySelectorAll('.run-verb')).map(b => ({"
                " label: b.textContent.trim(), disabled: b.disabled, title: b.title }))"
            )
            reason = page.evaluate(f"() => {SH}.writeLockReason()")
            check(
                "run / triage / push are all disabled while a run holds the lock",
                len(verbs) == 3 and all(v["disabled"] is True for v in verbs),
                f"got={[(v['label'], v['disabled']) for v in verbs]}",
            )
            check(
                "every disabled verb states the reason in its title",
                reason != "" and all(v["title"] == reason for v in verbs),
                f"reason={reason!r} titles={[v['title'] for v in verbs]}",
            )
            # …and the toolbar says it NOWHERE ELSE. The reason used to be
            # restated as a visible note beside the verbs; the operator asked for
            # the toolbar to carry no message, so the title is the single place
            # the gate explains itself. Pinned as an ABSENCE so nobody
            # reintroduces the note by reflex.
            lock_note = page.evaluate("() => document.querySelectorAll('.runs-lock-note').length")
            toolbar_text = page.evaluate(
                "() => (document.querySelector('.runs-actions')?.innerText || '').trim()"
            )
            check(
                "the locked toolbar carries no message of its own",
                lock_note == 0 and reason not in toolbar_text,
                f"noteNodes={lock_note} toolbar={toolbar_text!r}",
            )
            # …and the gate must not have cost the operator the panel's reading.
            live = page.evaluate(
                "() => ({ steps: document.querySelectorAll('.plan-steps li').length,"
                " trail: document.querySelectorAll('.trail-node').length })"
            )
            check(
                "reading keeps working while locked: the plan and the trail still render",
                live["steps"] == 3 and live["trail"] == 3,
                f"steps={live['steps']} trail={live['trail']}",
            )
            write(
                plan=plan_block(
                    steps(
                        ("first step body", "checked"),
                        ("second step body", "checked"),
                        ("third step body", "checked"),
                    )
                )
            )
            # No click, no reload: monitoring must advance under the lock.
            page.wait_for_function(
                "() => { const li = Array.from(document.querySelectorAll('.plan-steps li'));"
                " return li.length === 3 && li.every(e => e.className.includes('st-checked')); }",
                timeout=15000,
            )
            check("monitoring keeps advancing while locked, with no interaction", True, "3/3 st-checked")
            page.screenshot(path=os.path.join(SHOT_DIR, "331-runs-chrome-2026-07-26.png"))

            # --- scenario 7: the lock's release re-enables the verbs ----------
            doc.unlink()
            page.wait_for_function(
                "() => document.querySelector('.run-verb')?.disabled === false", timeout=15000
            )
            released = page.evaluate(
                "() => ({ disabled: Array.from(document.querySelectorAll('.run-verb')).map(b => b.disabled),"
                " titles: Array.from(document.querySelectorAll('.run-verb')).map(b => b.title) })"
            )
            check(
                "the verbs re-enable when the lock is released, unaided",
                released["disabled"] == [False, False, False],
                f"disabled={released['disabled']}",
            )
            check(
                "an enabled verb's title returns to its own description",
                all(reason not in t for t in released["titles"]),
                f"titles={released['titles']}",
            )

            # --- scenario 8: the gate is a HINT; the CLI still refuses --------
            # Dispatched on the SAME door a click uses, while UNLOCKED — nothing
            # on this path consults the gate, which is what makes the CLI the
            # authority. The fixture repo has no GitHub remote, so `ralphy
            # triage` streams its complaint and exits 1.
            page.evaluate(
                "(slug) => document.dispatchEvent(new CustomEvent('workbench:action',"
                " { detail: { action: 'command', verb: 'triage', project: slug } }))",
                slug,
            )
            page.wait_for_function(
                "() => document.querySelector('.runs-verb-error')?.offsetParent !== null", timeout=60000
            )
            surfaced = page.evaluate(
                f"() => {{ const lines = {SH}.rawFeed.split(/\\r?\\n/).filter(l => l.trim() !== '');"
                " return { err: document.querySelector('.runs-verb-error span').textContent.trim(),"
                " last: (lines[lines.length - 1] || '').trim(),"
                f" flash: {SH}.runsActionMsg }}; }}"
            )
            check(
                "an unlocked click still reaches the CLI and its refusal is surfaced",
                "refused" in surfaced["err"] and "exit 1" in surfaced["err"],
                f"err={surfaced['err']!r}",
            )
            check(
                "the panel line carries the CLI's own last output line",
                surfaced["last"] != "" and surfaced["last"] in surfaced["err"],
                f"last={surfaced['last']!r}",
            )
            # `refused (exit 1)` is synthesized from the exit CODE alone, so it
            # would also pass on a panic or a missing `git`. The fixture's real
            # precondition — no GitHub remote — has to be read off the message.
            check(
                "the surfaced refusal is the NO-REMOTE one, not any non-zero exit",
                re.search(r"remote|github", surfaced["err"], re.I) is not None,
                f"err={surfaced['err']!r}",
            )
            # The 2.6 s `_flashAction` timer is the bar: a refusal that expires
            # with it is the defect this replaces. Waiting on the FLASH's own
            # expiry rather than a fixed sleep makes the comparison exact
            # instead of leaving 600 ms of margin to a timing change.
            page.wait_for_function(
                "() => document.querySelector('.runs-action-status')?.offsetParent == null",
                timeout=15000,
            )
            outlived = page.evaluate(
                "() => ({ err: document.querySelector('.runs-verb-error')?.offsetParent !== null,"
                " flash: document.querySelector('.runs-action-status')?.offsetParent !== null })"
            )
            check(
                "the refusal OUTLIVES the 2.6 s action flash",
                outlived["err"] is True and outlived["flash"] is False,
                f"errVisible={outlived['err']} flashVisible={outlived['flash']}",
            )
            page.click('[data-act="verb-error-dismiss"]:visible')
            page.wait_for_function(
                "() => document.querySelector('.runs-verb-error')?.offsetParent == null", timeout=10000
            )
            check("the operator can dismiss the refusal", True, "")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 27
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("RUNS CHROME")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
