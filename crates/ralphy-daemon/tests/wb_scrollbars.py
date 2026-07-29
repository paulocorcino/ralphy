"""Browser acceptance for the design-system scrollbar: MEASURED, not eyeballed.

The workbench's scrollbar treatment was reported as broken three times, and each
round added the reported surface to a list of selectors. This test exists because
the fourth round found that the list had never rendered at all — and no assertion
in the repo could have noticed, since the only gate over `assets/ui/**` is a
substring pin, and a rule that is present can still be inert.

MEASURED HERE (scenario 1), which is the fact the fix rests on:

    scrollbar-color + ::-webkit-scrollbar{width:8px}  ->  15px  (the platform's)
    ::-webkit-scrollbar{width:8px} alone              ->   8px
    scrollbar-width: thin (+ webkit)                  ->  10px
    neither                                           ->  15px

Setting `scrollbar-color` makes the browser IGNORE that element's
`::-webkit-scrollbar` rules. Every block in styles.css used to set both together,
so every 8px/10px geometry there was dead code and every "themed" surface drew a
platform-width bar in ralphy's colours. The standard properties are now the only
mechanism — and because `scrollbar-width` does NOT inherit (only
`scrollbar-color` does), the rule has to be `*` rather than `html`.

Scenario 1  the mechanism itself, on a bare page: the four cells above
Scenario 2  every scrolling surface of the shell is thin, INCLUDING the ones that
            never carried a rule (the plan steps, the Settings panes) — this is
            the operator's report, measured
Scenario 3  the thumb is the design system's colour, and the structural surfaces
            take the brighter one

NOTE ON HEADLESS: Chromium's headless mode passes `--hide-scrollbars`, so a
scrollbar has no width at all and every measurement here reads 0. The browser is
launched with that default REMOVED. Without this, the test silently passes
nothing — which is how the dead rules survived four rounds.

Boots a Localhost daemon on 7444 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_scrollbars.py   (exit 0 = all pass)
"""

import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7444
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_scrollbars.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

# The platform's default bar on this host; `thin` is narrower than it everywhere
# the standard property is honoured. Compared as an INEQUALITY, never as `== 10`:
# the exact `thin` width is the browser's choice and not ralphy's contract.
PLATFORM = 15

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
    empty = tempfile.mkdtemp(prefix="wbsb_empty_")
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
    d = tempfile.mkdtemp(prefix="wbsb_fixture_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "runstate").mkdir()
    (p / "README.md").write_text("# fixture\n\nThe scrollbar fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbsb@example.com"],
        ["git", "config", "user.name", "wbsb"],
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
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# Scenario 1 — the mechanism, isolated from the shell entirely.
MECH_HTML = """
<style>
  div { width: 200px; height: 80px; overflow-y: scroll; }
  #both { scrollbar-color: red transparent; }
  #both::-webkit-scrollbar { width: 8px; }
  #webkitonly::-webkit-scrollbar { width: 8px; }
  #stdthin { scrollbar-width: thin; }
  #stdthin::-webkit-scrollbar { width: 8px; }
</style>
<div id="both"><p style="height:400px">x</p></div>
<div id="webkitonly"><p style="height:400px">x</p></div>
<div id="stdthin"><p style="height:400px">x</p></div>
<div id="plain"><p style="height:400px">x</p></div>
"""

MECH_JS = """
() => {
  const g = (id) => { const e = document.getElementById(id); return e.offsetWidth - e.clientWidth; };
  return { both: g("both"), webkitonly: g("webkitonly"), stdthin: g("stdthin"), plain: g("plain") };
}
"""

# Scenario 2/3 — the shell's own surfaces. `overflow-y: scroll` is forced so the
# bar exists even where the content happens to fit; the gutter is then the width
# the browser gave it.
SHELL_JS = """
(sels) => {
  const out = {};
  for (const sel of sels) {
    const e = document.querySelector(sel);
    if (!e) { out[sel] = "missing"; continue; }
    e.style.overflowY = "scroll";
    const cs = getComputedStyle(e);
    out[sel] = { gutter: e.offsetWidth - e.clientWidth, width: cs.scrollbarWidth, color: cs.scrollbarColor };
  }
  return out;
}
"""

# Two groups with different expected COLOURS. The second is the structural set —
# dragged rather than skimmed, so a brighter thumb.
QUIET = [".settings-content", ".settings-nav", ".kd-inner", ".kanban-col-body", ".about-body"]
BRIGHT = [".projects", "#workspace"]


def main():
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbsb_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # `--hide-scrollbars` REMOVED: see the note in the module docstring.
            # With it, every gutter below reads 0 and this file proves nothing.
            browser = p.chromium.launch(
                headless=True,
                args=["--disable-webgl", "--disable-gpu"],
                ignore_default_args=["--hide-scrollbars"],
            )
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})

            # --- scenario 1: the mechanism ------------------------------------
            bare = ctx.new_page()
            bare.set_content(MECH_HTML)
            mech = bare.evaluate(MECH_JS)
            check(
                "a scrollbar is measurable at all (headless hides them by default)",
                mech["plain"] > 0,
                f"got={mech!r}",
            )
            check(
                "`scrollbar-color` DISABLES that element's ::-webkit-scrollbar",
                mech["both"] == mech["plain"] and mech["both"] > mech["webkitonly"],
                f"both={mech['both']} plain={mech['plain']} webkitonly={mech['webkitonly']}",
            )
            check(
                "…so the two mechanisms cannot be combined: each works only alone",
                mech["webkitonly"] < mech["plain"] and mech["stdthin"] < mech["plain"],
                f"webkitonly={mech['webkitonly']} stdthin={mech['stdthin']} plain={mech['plain']}",
            )
            bare.close()

            # --- scenario 2: every shell surface is thin ----------------------
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {SH}.toggle('{slug}')")
            page.wait_for_timeout(500)
            page.evaluate(f"() => {SH}.toggleKanban()")
            page.wait_for_timeout(400)
            page.evaluate(f"() => {SH}.openIssue(1)")
            page.wait_for_timeout(300)
            page.evaluate(f"() => {SH}.settingsOpen = true")
            page.wait_for_timeout(500)
            surfaces = page.evaluate(SHELL_JS, QUIET + BRIGHT)
            present = {k: v for k, v in surfaces.items() if v != "missing"}
            check(
                "the surfaces under test are actually on screen",
                len(present) >= 5,
                f"missing={[k for k, v in surfaces.items() if v == 'missing']}",
            )
            fat = {k: v["gutter"] for k, v in present.items() if v["gutter"] >= PLATFORM}
            check(
                "no surface wears the platform bar — including those that never had a rule",
                not fat,
                f"platform-width={fat!r}",
            )
            auto = {k: v["width"] for k, v in present.items() if v["width"] != "thin"}
            check(
                "…because `scrollbar-width: thin` reaches every element, not just the root",
                not auto,
                f"computed-auto={auto!r}",
            )

            # --- scenario 3: the colours --------------------------------------
            # `--border` is rgb(76,66,57) and `--border-focus` is brighter; compare
            # the two GROUPS rather than hard-coding either, so a palette change is
            # not a test failure.
            quiet = {k: present[k]["color"] for k in QUIET if k in present}
            bright = {k: present[k]["color"] for k in BRIGHT if k in present}
            check(
                "every quiet surface shares one thumb colour",
                len(set(quiet.values())) == 1,
                f"got={quiet!r}",
            )
            check(
                "the structural surfaces take a DIFFERENT, brighter one",
                bright and set(bright.values()).isdisjoint(set(quiet.values())),
                f"bright={bright!r} quiet={set(quiet.values())!r}",
            )
            check(
                "the track is transparent, not a painted rail",
                all("rgba(0, 0, 0, 0)" in c for c in list(quiet.values()) + list(bright.values())),
                f"got={ {**quiet, **bright} !r}",
            )

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    passed = sum(1 for r in results if r)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if passed == len(results) else 1)


if __name__ == "__main__":
    main()
