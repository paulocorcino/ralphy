"""#302 browser acceptance: the issue drawer actually shows the issue.

One Playwright pass over a REAL daemon. `board.list` and `issue.show` are
intercepted in the page (both spawn a CLI that needs a real GitHub tracker,
which a throwaway fixture repo has not); every other verb stays real. The
CLI<->daemon contract itself is covered deterministically by
`cli::tests::issues_show_documented_form_parses` — a parse-level assertion that
needs no tracker, no network and no authentication (PRD #296).

Because `issue.show` is stubbed, these scenarios cover the CLIENT render path
only — the wire shape itself is pinned by `issues::tests::show_view_json_includes_comments`
and the argv contract by `cli::tests::issues_show_documented_form_parses`.

Scenario 1   the board loads and a card opens the drawer
Scenario 2   the drawer renders the issue BODY delivered by `issue.show`
Scenario 3   each comment renders its author and a FORMATTED date from
             the `{author, at, body}` record
Scenario 4   the drawer is wide enough to read a long issue
Scenario 5   a failed detail fetch shows a visible error banner — and the banner
             CLEARS on the next successful open
Scenario 6   a STALE failed load never paints over the newer successful one

Boots a Localhost daemon on 7398 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/302-issue-drawer-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_issue_302.py   (exit 0 = all pass)
"""

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

PORT = 7398
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_issue_302.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

RAW_AT = "2026-07-23T17:21:43Z"
BODY = "the #302 fixture body"

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
    empty = tempfile.mkdtemp(prefix="wb302_empty_")
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


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb302_fixture_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "runstate").mkdir()
    (p / "README.md").write_text("# fixture\n\nThe #302 issue-drawer fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb302@example.com"],
        ["git", "config", "user.name", "wb302"],
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
    # after any assets/ui edit or the browser loads yesterday's drawer.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# The spy: answer `board.list` from a fixed fold and `issue.show` from the
# structured detail the CLI now emits (`comments[] = {author, at, body}`), and
# delegate everything else to the real daemon. Installed in the page so the
# client's own call path — `loadIssueDetail` and its error branch — is exercised.
SPY_JS = """
(k) => {
  window.__showCalls = [];
  window.__showMode = "ok";        // "ok" | "error" | "deferError"
  window.__deferred = null;        // the held resolver, for the stale-reply leg
  const real = window.WBDaemon.observe.bind(window.WBDaemon);
  const row = (n, title) => ({
    number: n, title, state: "open", labels: ["ready-for-agent"],
    assignees: [], blocked_by: [], created: "2026-07-20T10:00:00Z", updated: "2026-07-24T10:00:00Z",
  });
  window.WBDaemon.observe = (verb, payload) => {
    if (verb === "board.list") {
      return Promise.resolve({
        status: "ok",
        board: {
          issues: [row(71, "the first one"), row(72, "the second one"), row(73, "the third one")],
          labels: [{ name: "ready-for-agent", color: "0E8A16" }],
        },
      });
    }
    if (verb === "issue.show") {
      window.__showCalls.push(payload);
      if (window.__showMode === "error") {
        return Promise.resolve({ status: "error", message: "detail read failed" });
      }
      // A failure held open, released by the test after a LATER load settles.
      if (window.__showMode === "deferError") {
        return new Promise((res) => {
          window.__deferred = () => res({ status: "error", message: "stale failure" });
        });
      }
      return Promise.resolve({
        status: "ok",
        issue: {
          number: payload.number,
          body: k.body,
          comments: [
            { author: "octocat", at: k.rawAt, body: "first comment" },
            { author: "paulocorcino", at: "2026-07-24T09:00:00Z", body: "second comment" },
          ],
          blocked_by: [],
        },
      });
    }
    return real(verb, payload);
  };
}
"""


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb302_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

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
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(SPY_JS, {"body": BODY, "rawAt": RAW_AT})
            page.wait_for_timeout(300)

            # --- scenario 1: the board, then the drawer -----------------------
            page.evaluate(f"() => {SH}.toggle('{slug}')")
            page.wait_for_timeout(400)
            page.evaluate(f"() => {SH}.toggleKanban()")
            page.wait_for_timeout(500)
            cards = page.locator(".kanban-card").count()
            check("the board renders the fixture rows", cards == 3, f"got={cards}")

            page.evaluate(f"() => {SH}.openIssue(72)")
            page.wait_for_timeout(600)
            drawer = page.locator(".kanban-detail.open")
            check("clicking a card opens the detail drawer", drawer.is_visible())
            calls = page.evaluate("() => window.__showCalls.length")
            check("…and asks the daemon for the detail", calls == 1, f"got={calls}")

            # --- scenario 2: the BODY (empty for every project before #302) ----
            body_txt = page.locator(".kanban-detail.open .kd-body").inner_text().strip()
            check(
                "the drawer shows the issue body",
                BODY in body_txt,
                f"got={body_txt[:80]!r}",
            )

            # --- scenario 3: author + formatted date per comment ---------------
            # `inner_text` returns the RENDERED text, and the head is
            # `text-transform: uppercase` in styles.css — compare case-folded.
            head = page.locator(".kanban-detail.open .kd-comments-head").inner_text().strip()
            check("the drawer counts the thread", head.lower() == "2 comments", f"got={head!r}")
            first = page.locator(".kanban-detail.open .kd-comment").first
            head0 = first.locator(".kd-comment-head").inner_text().strip()
            check("the first comment names its author", "octocat" in head0, f"got={head0!r}")
            at0 = first.locator(".kd-comment-at").inner_text().strip()
            check("…and carries a non-empty date", at0 != "", f"got={at0!r}")
            # Not the raw ISO string: the drawer formats it, so a wire shape that
            # dropped `at` (rendering blank) and one that dumped it raw both fail.
            check("…rendered, not the raw ISO timestamp", at0 != RAW_AT, f"got={at0!r}")
            body0 = first.locator(".kd-comment-body").inner_text().strip()
            check("…and its body", "first comment" in body0, f"got={body0[:60]!r}")

            # --- scenario 4: the reading width --------------------------------
            box = page.locator(".kanban-detail.open").bounding_box()
            width = box["width"] if box else 0
            check(
                "the drawer is wide enough to read a long issue",
                width > 600,
                f"width={width} at a 1400px viewport",
            )

            page.screenshot(path=os.path.join(SHOT_DIR, "302-issue-drawer-2026-07-25.png"))

            # --- scenario 5: a failed fetch is VISIBLE -------------------------
            page.evaluate(f"() => {SH}.closeIssue()")
            page.wait_for_timeout(200)
            page.evaluate("() => { window.__showMode = 'error'; }")
            page.evaluate(f"() => {SH}.openIssue(73)")
            page.wait_for_timeout(700)
            err = page.locator(".kanban-detail.open .kd-error")
            shown = page.evaluate(
                "() => { const e = document.querySelector('.kanban-detail.open .kd-error');"
                " return !!e && e.offsetParent !== null; }"
            )
            check("a failed detail fetch raises a VISIBLE banner", shown is True)
            err_txt = err.inner_text().strip() if err.count() else ""
            check(
                "…carrying the daemon's own message",
                "detail read failed" in err_txt,
                f"got={err_txt!r}",
            )

            # The banner must not outlive the failure: a good open clears it.
            page.evaluate("() => { window.__showMode = 'ok'; }")
            page.evaluate(f"() => {SH}.closeIssue()")
            page.wait_for_timeout(200)
            page.evaluate(f"() => {SH}.openIssue(71)")
            page.wait_for_timeout(700)
            check(
                "…and clears on the next successful open",
                page.evaluate(f"() => {SH}.issueError") in (None, ""),
                f"issueError={page.evaluate(f'() => {SH}.issueError')!r}",
            )
            still = page.evaluate(
                "() => { const e = document.querySelector('.kanban-detail.open .kd-error');"
                " return !!e && e.offsetParent !== null; }"
            )
            check("…so the banner is gone from the DOM view", still is False)
            body_txt = page.locator(".kanban-detail.open .kd-body").inner_text().strip()
            check("…and the recovered drawer shows the body", BODY in body_txt, f"got={body_txt[:60]!r}")

            # --- scenario 6: a STALE failure never paints over a good drawer ---
            # The board fold re-fires `loadIssueDetail` for the open drawer on
            # every refresh (#301), so two loads for the SAME number can be in
            # flight. The newest one owns the drawer: an older failure landing
            # after it must be dropped, or the banner lies over real content.
            page.evaluate("() => { window.__showMode = 'deferError'; window.__deferred = null; }")
            # Fire-and-forget on purpose: returning the promise would make
            # `evaluate` await a load this leg deliberately leaves hanging.
            page.evaluate(f"() => {{ {SH}.loadIssueDetail(71); }}")
            page.wait_for_function("() => !!window.__deferred", timeout=5000)
            page.evaluate("() => { window.__showMode = 'ok'; }")
            page.evaluate(f"() => {{ {SH}.loadIssueDetail(71); }}")
            page.wait_for_timeout(500)
            page.evaluate("() => window.__deferred()")
            page.wait_for_timeout(500)
            check(
                "a stale failed load never overwrites the newer good one",
                page.evaluate(f"() => {SH}.issueError") in (None, ""),
                f"issueError={page.evaluate(f'() => {SH}.issueError')!r}",
            )
            body_txt = page.locator(".kanban-detail.open .kd-body").inner_text().strip()
            check("…and the drawer keeps its body", BODY in body_txt, f"got={body_txt[:60]!r}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The count floor is load-bearing: an early `sys.exit` or a scenario that
    # never ran must not report success on a handful of passing checks.
    ok = all(results) and len(results) >= 18
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("ISSUE DETAIL HONEST")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
