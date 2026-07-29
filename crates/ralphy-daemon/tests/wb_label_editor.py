"""Browser acceptance for the board's label editor: unclipped, and honest.

Two independent defects, reported together against a live run:

  (a) THE MENU WAS CLIPPED. It was a `.dropdown` — `position: absolute` — inside
      `.kd-inner`'s scroll inside `.kanban-detail`'s `overflow: hidden`. An
      absolutely-positioned box is clipped by the ancestor that hides overflow,
      so with sixteen labels (~380px, capped at 300) it was cut off on every card
      past the middle of the drawer. It now opens IN FLOW on its own row, where
      nothing can clip it.

  (b) SELECTING A LABEL DID NOTHING. `label.set` is a run-lock-aware Mutate
      (`mutate.rs`'s `guard_run_lock(&ws, "label set", …)`), so with a run holding
      the repo's lock the CLI refuses, the optimistic chip snaps back, and the
      operator sees a 2.6s flash at most. The editor was the ONE write control in
      the shell with no gate; it now carries the #318 treatment (disabled + the
      reason in the title).

Scenario 1  with no run, the menu opens FULLY INSIDE the drawer — and its last
            option is reachable, which is what "not clipped" means
Scenario 2  the same on a card at the BOTTOM of the drawer's scroll: the case
            that clipped, and the case `scrollIntoView` has to answer
Scenario 3  toggling a label sends `label.set` and the chip persists
Scenario 4  with a run holding the lock, every affordance is disabled, states the
            reason, and NO `label.set` is sent

`board.list`, `issue.show`, `runs.list` and `label.set` are intercepted in the
page: the first two need a real GitHub tracker, `label.set` would spawn a real
`gh` call, and `runs.list` is how a live run is seeded without running one. Every
other verb stays real. The gate's own predicate is unit-tested in
`ui-tests/wb-changes.test.mjs` and pinned from Rust by
`the_label_editor_is_unclipped_and_closed_under_a_live_run`.

Boots a Localhost daemon on 7443 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_label_editor.py   (exit 0 = all pass)
"""

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

PORT = 7443
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_label_editor.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

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
    empty = tempfile.mkdtemp(prefix="wblabel_empty_")
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
    d = tempfile.mkdtemp(prefix="wblabel_fixture_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "runstate").mkdir()
    (p / "README.md").write_text("# fixture\n\nThe label-editor fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wblabel@example.com"],
        ["git", "config", "user.name", "wblabel"],
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


# The spy. `window.__runs` drives the lock: [] is an idle repo, one entry is a
# live run — the SAME shape `runs.list` answers with, so the gate is exercised
# through its real derivation and not through a flag the test invented.
# `__labelCalls` is the proof of the negative in scenario 4.
SPY_JS = """
() => {
  window.__runs = [];
  window.__labelCalls = [];
  const real = window.WBDaemon.observe.bind(window.WBDaemon);
  const row = (n, title) => ({
    number: n, title, state: "open", labels: ["ready-for-agent"],
    assignees: [], blocked_by: [], created: "2026-07-20T10:00:00Z", updated: "2026-07-28T10:00:00Z",
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
      return Promise.resolve({
        status: "ok",
        issue: { number: payload.number, body: "a long body\\n\\n" + "filler paragraph\\n\\n".repeat(40),
                 comments: [], blocked_by: [] },
      });
    }
    if (verb === "runs.list") {
      return Promise.resolve({ status: "ok", runs: window.__runs });
    }
    if (verb === "label.set") {
      window.__labelCalls.push(payload);
      return Promise.resolve({ status: "ok" });
    }
    return real(verb, payload);
  };
}
"""

# THE defect, measured rather than described.
#
# MEASURED against the old geometry (the rule AND `.menu-wrap { position:
# relative }`, which was its containing block — restoring the rule alone anchors
# the menu somewhere else and measures nothing): with a wrapper only as wide as
# the `edit` chip, `right: 0` pushed a 200px-min menu to `left: 721` while the
# drawer's own left edge is at 748. The clip was HORIZONTAL — a 27px sliver of
# every row cut off at the drawer's edge, which is what the report showed.
# `insideLeft` is therefore the demonstrated discriminator, with `position` its
# structural companion.
#
# `lastHit` does NOT discriminate (it was true on both builds, because the clipped
# strip was at the side rather than under the pointer). It is kept as its own
# claim — after scrolling the list, its last option is clickable — not as proof of
# the fix.
GEOM_JS = """
() => {
  const m = document.querySelector(".kd-label-menu");
  const d = document.querySelector(".kanban-detail.open");
  const inner = document.querySelector(".kd-inner");
  if (!m || !d) return { menu: false };
  const mr = m.getBoundingClientRect();
  const dr = d.getBoundingClientRect();
  // The menu has its own 300px scroll, so the LAST of sixteen-plus labels is
  // legitimately out of view until the list is scrolled. Scroll it first, or the
  // hit test asks about a row nobody claimed was on screen.
  m.scrollTop = m.scrollHeight;
  const opts = m.querySelectorAll(".kd-label-opt");
  const lastEl = opts[opts.length - 1];
  const last = lastEl?.getBoundingClientRect();
  const hit = last && document.elementFromPoint(last.left + last.width / 2, last.top + last.height / 2);
  return {
    menu: true,
    options: opts.length,
    insideTop: mr.top >= dr.top - 1,
    insideBottom: mr.bottom <= dr.bottom + 1,
    insideLeft: mr.left >= dr.left - 1,
    insideRight: mr.right <= dr.right + 1,
    lastHit: !!hit && !!lastEl && lastEl.contains(hit),
    // In flow it participates in the drawer's own scroll, which is what makes
    // the whole list reachable instead of merely present.
    scrolls: inner.scrollHeight > inner.clientHeight,
    position: getComputedStyle(m).position,
  };
}
"""


def open_drawer(page, slug, number):
    page.evaluate(f"() => {SH}.toggle('{slug}')")
    page.wait_for_timeout(400)
    if not page.evaluate(f"() => {SH}.kanbanOpen"):
        page.evaluate(f"() => {SH}.toggleKanban()")
    page.wait_for_timeout(500)
    page.evaluate(f"() => {SH}.openIssue({number})")
    page.wait_for_selector(".kanban-detail.open", timeout=8000)
    page.wait_for_timeout(400)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wblabel_reg_")
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
            page.evaluate(SPY_JS)
            page.wait_for_timeout(300)

            # --- scenario 1: the menu opens fully inside the drawer -----------
            open_drawer(page, slug, 72)
            edit = page.locator(".kd-label-edit")
            check("the label editor is enabled on an idle repo", edit.is_enabled())
            edit.click()
            page.wait_for_selector(".kd-label-menu", state="visible", timeout=5000)
            page.wait_for_timeout(300)
            geom = page.evaluate(GEOM_JS)
            check("the menu offers every ralphy label", geom["options"] >= 10, f"got={geom['options']}")
            check(
                "the menu is NOT a floating dropdown",
                geom["position"] == "static",
                f"position={geom['position']!r}",
            )
            check(
                "the whole menu lies inside the drawer that used to clip it",
                all(geom[k] for k in ("insideTop", "insideBottom", "insideLeft", "insideRight")),
                f"got={ {k: geom[k] for k in ('insideTop', 'insideBottom', 'insideLeft', 'insideRight')} }",
            )
            check(
                "scrolled to its end, the last option is clickable",
                geom["lastHit"] is True,
                f"hit={geom['lastHit']}",
            )
            page.screenshot(path=str(Path(SHOT_DIR, "label-editor-unclipped.png")))

            # --- scenario 2: opened from the bottom of a scrolled drawer ------
            # The case that clipped: the row sits low, and an absolute menu
            # dropped straight through `.kanban-detail`'s hidden overflow.
            page.evaluate(f"() => {SH}.labelMenuOpen = false")
            page.wait_for_timeout(200)
            page.evaluate("() => { const i = document.querySelector('.kd-inner'); i.scrollTop = i.scrollHeight; }")
            page.wait_for_timeout(300)
            page.locator(".kd-label-edit").click()
            page.wait_for_selector(".kd-label-menu", state="visible", timeout=5000)
            page.wait_for_timeout(400)
            geom = page.evaluate(GEOM_JS)
            check(
                "the drawer genuinely scrolls (the clipping precondition held)",
                geom["scrolls"] is True,
                f"got={geom['scrolls']}",
            )
            check(
                "opened from the bottom of the scroll it is still fully inside and clickable",
                all(geom[k] for k in ("insideTop", "insideBottom", "insideLeft", "insideRight"))
                and geom["lastHit"] is True,
                f"got={geom!r}",
            )

            # --- scenario 3: a toggle reaches the daemon ----------------------
            before = page.evaluate("() => window.__labelCalls.length")
            page.locator(".kd-label-opt").first.click()
            page.wait_for_timeout(400)
            calls = page.evaluate("() => window.__labelCalls")
            check(
                "toggling a label sends label.set for this issue",
                len(calls) == before + 1 and calls[-1]["number"] == 72 and calls[-1]["repo"] == slug,
                f"got={calls[-1:]!r}",
            )

            # --- scenario 4: a live run closes the editor, honestly ------------
            # One `runs.list` entry IS the lock, in the shape the panel reads.
            page.evaluate(
                "() => { window.__runs = [{ runid: '01LOCKHOLDER', face: '🦊', agent: 'codex',"
                " phase: 'executing', active: 72, issues: [], completed: 0, queueTotal: 1 }]; }"
            )
            page.evaluate(f"() => {SH}.hydrateRuns()")
            page.wait_for_function(f"() => {SH}.labelsLocked() === true", timeout=8000)
            page.wait_for_timeout(300)
            reason = page.evaluate(f"() => {SH}.labelLockReason()")
            check(
                "the reason names the lock AND what it closed",
                "holds this repo's lock" in reason and "labels are read-only" in reason,
                f"got={reason!r}",
            )
            edit = page.locator(".kd-label-edit")
            check("the edit button is disabled under a live run", edit.is_disabled())
            check(
                "…and says why, where a dimmed control raises the question",
                edit.get_attribute("title") == reason,
                f"got={edit.get_attribute('title')!r}",
            )
            opts_enabled = page.evaluate(
                "() => Array.from(document.querySelectorAll('.kd-label-opt')).filter(b => !b.disabled).length"
            )
            check("every option in the open menu is disabled too", opts_enabled == 0, f"got={opts_enabled}")
            # THE POINT of the whole change: no optimistic edit the CLI is certain
            # to refuse. `force` because a disabled button ignores a real click —
            # this asks whether the HANDLER also refuses.
            before = page.evaluate("() => window.__labelCalls.length")
            page.locator(".kd-label-opt").first.click(force=True)
            page.wait_for_timeout(400)
            after = page.evaluate("() => window.__labelCalls.length")
            check("a locked toggle sends nothing at all", after == before, f"{before} -> {after}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    passed = sum(1 for r in results if r)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if passed == len(results) else 1)


if __name__ == "__main__":
    main()
