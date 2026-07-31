"""#358 browser acceptance: the Spend tab's tracer bullet, end to end.

One Playwright pass over a REAL daemon proving the thin path through every layer
carries one honest number: the icon rail opens a Spend CANVAS TAB, the tab says
what to do when no project is open, and with a project open it shows that
project's total and its `token_meter` split — both rendered by the daemon.

The ledger fixture is built so the total CANNOT be exact: one line is priced
(`claude-opus-4-8`), one is `unknown` WITH a session id (recoverable) and one is
`unknown` with NO session id (lost). So the tab must read as a floor, with the
unpriced volume beside it split into those two causes — the slice's marquee rule
(ADR-0034 D3, ADR-0053 D4), and the one an implementation that prices an unknown
model at `$0` would fail.

Scenario a  the icon rail carries a Spend button, and clicking it opens a
            CLOSABLE canvas tab after Consoles (ADR-0037 §3's amendment)
Scenario b  with no project open the tab tells the operator to open one — the
            empty state, never a blank pane or a `$0`
Scenario c  with the project open the tab shows its total and the token meter
Scenario d  the total is a FLOOR (`+`), and says in words that it is one
Scenario e  the unpriced volume is on screen, split into recoverable and lost
Scenario f  every figure on screen is the daemon's: the strings the tab renders
            are byte-identical to `/api/spend`'s, so no client-side `k`/`M`
            abbreviation crept back in
Plus        the tab closes like any other tab and leaves Consoles alone; the
            summary response never ships ledger rows.

Boots a Localhost daemon on 7443 over a SCRATCH `RALPHY_DAEMON_DIR` and a scratch
`RALPHY_USAGE_DIR`, so the operator's own daemon registry, login policy and real
ledger are untouched. `RALPHY_PRICING_FILE` is pointed at a written-here table so
the figure is this fixture's, not the host's `pricing.toml`. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Every element assertion is gated on `offsetParent !== null && clientWidth > 0`:
a measurement of a zero-width element passes a "visible" test vacuously
(CONTEXT.md, the vacuous-geometry trap).

Writes docs/screenshots/358-spend-2026-07-31.png.
Run: python crates/ralphy-daemon/tests/wb_spend_358.py   (exit 0 = all pass)
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

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7443
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_spend_358.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "358-spend-2026-07-31.png")
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


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def seed_repo():
    """A committed git repo — the Spend tab needs a project to scope ON, not a
    project to read files from."""
    d = Path(tempfile.mkdtemp(prefix="wb358_repo_")) / "spend-fixture"
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "a.txt").write_text("alpha\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb358@example.com")
    git(d, "config", "user.name", "wb358")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return str(d)


def line(slug, issue, model, session, tokens):
    """One ledger line in the on-disk shape core writes (ADR-0008 D6). `session`
    is omitted entirely when absent — that is the shape a pre-ADR-0033 line has,
    and it is what makes a line UNRECOVERABLE."""
    row = {
        "project": slug,
        "actor_email": "wb358@example.com",
        "actor_name": "wb358",
        "ralphy_version": "0.0.0-test",
        "issue": issue,
        "phase": "execute",
        "agent": "claude",
        "model": model,
        "outcome": "success",
        "tokens": {"input": tokens, "output": 0, "cache_read": 0, "cache_creation": 0},
        "ts": "2026-07-30T12:00:00+00:00",
    }
    if session:
        row["session_id"] = session
    return json.dumps(row)


def seed_ledger(usage_dir, slug):
    """Three lines: one priceable, one recoverable, one lost. The mix is the
    point — a fixture that priced cleanly could not tell a floor from a total."""
    rows = [
        line(slug, 1, "claude-opus-4-8", "sess-a", 1_000_000),
        line(slug, 2, "unknown", "sess-b", 500_000),
        line(slug, 3, "unknown", None, 250_000),
    ]
    Path(usage_dir, "spend-fixture.jsonl").write_text("\n".join(rows) + "\n", encoding="utf-8")


def seed_pricing(dir_path):
    """A pricing table written for this test, so the figure below is the
    fixture's and not whatever the host operator put in their own overrides."""
    path = Path(dir_path, "pricing.toml")
    path.write_text(
        "[claude-opus-4-8]\ninput = 15.0\noutput = 75.0\ncache_read = 1.5\ncache_creation = 18.75\n",
        encoding="utf-8",
    )
    return str(path)


def empty_env(daemon_dir, usage_dir, pricing_file):
    """A scratch registry, a scratch ledger and EMPTY vendor stores: the operator's
    own daemon dir (and its login policy) is never touched, and the interactive
    scan finds nothing — so every token on screen came from the ledger above."""
    empty = tempfile.mkdtemp(prefix="wb358_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=usage_dir,
        RALPHY_PRICING_FILE=pricing_file,
        RALPHY_PRICING_CACHE=os.path.join(empty, "no-cache.json"),
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
        RALPHY_CURSOR_DIR=empty,
        RALPHY_GEMINI_DIR=empty,
        RALPHY_COPILOT_DB=os.path.join(empty, "none.db"),
    )


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's workbench.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(env):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# The visible tab titles, in strip order. The Consoles tab rides at index 0.
TAB_TITLES = (
    "() => [...document.querySelectorAll('.tabstrip .tab')]"
    "  .filter(t => t.offsetParent !== null && t.clientWidth > 0)"
    "  .map(t => t.querySelector('.tab-title')?.textContent.trim())"
)

# Everything the Spend pane has on screen, read the way an operator sees it: only
# LAID-OUT elements, text trimmed. A zero-width figure must not pass as a figure.
PANE = """
() => {
  const laid = (el) => !!el && el.offsetParent !== null && el.clientWidth > 0;
  const txt = (sel) => { const e = document.querySelector(sel); return laid(e) ? e.textContent.trim() : null; };
  const pane = document.querySelector('.spend-tab');
  return {
    visible: laid(pane),
    emptyTitle: txt('.spend-tab .spend-empty-title'),
    emptyHint: txt('.spend-tab .spend-empty-hint'),
    figure: txt('.spend-tab .spend-figure'),
    floorNote: txt('.spend-tab .spend-floor-note'),
    meter: txt('.spend-tab .spend-meter-value'),
    scope: txt('.spend-tab .spend-scope'),
    unpricedTotal: txt('.spend-tab .spend-unpriced-total'),
    share: txt('.spend-tab .spend-unpriced-share'),
    causes: [...document.querySelectorAll('.spend-tab .spend-cause')]
      .filter(laid)
      .map(c => ({
        label: c.querySelector('.spend-cause-label')?.textContent.trim(),
        value: c.querySelector('.spend-cause-value')?.textContent.trim(),
      })),
  };
}
"""


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb358_reg_")
    usage_dir = tempfile.mkdtemp(prefix="wb358_usage_")
    pricing_file = seed_pricing(tempfile.mkdtemp(prefix="wb358_pricing_"))
    fixture = seed_repo()
    slug = register_fixture(daemon_dir, fixture)
    seed_ledger(usage_dir, slug)

    proc = launch(empty_env(daemon_dir, usage_dir, pricing_file))
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)

            # --- scenario a · the rail opens a canvas tab ---------------------
            rail = page.evaluate(
                "() => [...document.querySelectorAll('.rail button')]"
                "  .filter(b => b.offsetParent !== null && b.clientWidth > 0)"
                "  .map(b => b.getAttribute('title'))"
            )
            check(
                "the icon rail carries a Spend button",
                any((t or "").startswith("Spend") for t in rail),
                "titles={}".format(rail),
            )
            page.evaluate(
                "() => [...document.querySelectorAll('.rail button')]"
                "  .find(b => (b.getAttribute('title') || '').startsWith('Spend')).click()"
            )
            page.wait_for_function(f"() => {SH}.active === 'spend'", timeout=10000)
            titles = page.evaluate(TAB_TITLES)
            check(
                "…and clicking it opens a Spend tab AFTER the fixed Consoles tab",
                titles[:2] == ["Consoles", "Spend"],
                "titles={}".format(titles),
            )
            check(
                "…which is closable, unlike Consoles (ADR-0037 §3, amended)",
                page.evaluate(f"() => {SH}.tabs.map(t => t.closable)")[:2] == [False, True],
            )

            # --- scenario b · the empty state ---------------------------------
            # No project is open yet: the pane must SAY so. A blank pane and a
            # `$0` are the two failures this scenario exists to catch.
            page.wait_for_function(
                "() => { const e = document.querySelector('.spend-tab .spend-empty-title');"
                "  return !!e && e.offsetParent !== null && e.clientWidth > 0; }",
                timeout=10000,
            )
            pane = page.evaluate(PANE)
            check("the Spend pane is on screen", pane["visible"])
            check(
                "with no project open it tells the operator to open one",
                pane["emptyTitle"] == "No project open." and "Open a project" in (pane["emptyHint"] or ""),
                "title={} hint={}".format(pane["emptyTitle"], pane["emptyHint"]),
            )
            check(
                "…and shows no figure at all — not even a zero",
                pane["figure"] is None,
                "figure={}".format(pane["figure"]),
            )

            # --- scenario c · a project's total -------------------------------
            # The slug rides as an ARGUMENT, never interpolated: a repo registered
            # from a Windows path carries backslashes a literal would swallow.
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
            page.wait_for_function(
                "() => { const e = document.querySelector('.spend-tab .spend-figure');"
                "  return !!e && e.offsetParent !== null && e.clientWidth > 0; }",
                timeout=15000,
            )
            pane = page.evaluate(PANE)
            check(
                "the tab is scoped to the open project",
                slug in (pane["scope"] or ""),
                "scope={}".format(pane["scope"]),
            )
            # 1M input at $15/1M — the only priceable line in the fixture.
            check(
                "it shows the project's total cost",
                (pane["figure"] or "").startswith("$15.00"),
                "figure={}".format(pane["figure"]),
            )
            check(
                "…with the token_meter split beside it, in the terminal's vocabulary",
                pane["meter"] == "↑1.8M ⚡0 ❄0 ↓0",
                "meter={}".format(pane["meter"]),
            )

            # --- scenario d · the total is a floor ----------------------------
            check(
                "a total carrying unpriceable volume renders as a FLOOR",
                pane["figure"] == "$15.00+",
                "figure={}".format(pane["figure"]),
            )
            check(
                "…and says in words that it is one, rather than leaving a `+` to teach it",
                "floor" in (pane["floorNote"] or ""),
                "note={}".format(pane["floorNote"]),
            )

            # --- scenario e · the unpriced split ------------------------------
            check(
                "the unpriced volume is a first-class element beside the total",
                pane["unpricedTotal"] == "750.0k",
                "unpriced={}".format(pane["unpricedTotal"]),
            )
            causes = {c["label"]: c["value"] for c in pane["causes"]}
            check(
                "…split into recoverable and lost (ADR-0053 D4)",
                causes.get("recoverable") == "500.0k" and causes.get("lost") == "250.0k",
                "causes={}".format(causes),
            )
            check(
                "…and the share of the project's tokens is stated",
                "42.9%" in (pane["share"] or ""),
                "share={}".format(pane["share"]),
            )

            # --- scenario f · every figure is the daemon's --------------------
            # Fetched IN-PAGE, from the same origin the tab reads. Byte-identical
            # strings are the proof the client abbreviates nothing itself.
            doc = page.evaluate(
                "(s) => fetch('/api/spend?project=' + encodeURIComponent(s)).then(r => r.json())",
                arg=slug,
            )
            check(
                "the total on screen is the daemon's string, verbatim",
                doc["total"] == pane["figure"],
                "api={} screen={}".format(doc["total"], pane["figure"]),
            )
            check(
                "…and so is the meter, so no k/M abbreviation lives in JavaScript",
                doc["tokens"]["meter"] == pane["meter"],
                "api={} screen={}".format(doc["tokens"]["meter"], pane["meter"]),
            )
            check(
                "an unknown model contributes to the unpriced split, never $0",
                doc["unpriced"]["recoverable"] == 500_000
                and doc["unpriced"]["lost"] == 250_000
                and doc["floor"] is True,
                "unpriced={}".format(doc["unpriced"]),
            )
            check(
                "the response is a summary, not the ledger",
                "ts" not in json.dumps(doc) and "phase" not in json.dumps(doc),
            )

            page.screenshot(path=SHOT)
            print(f"[INFO] screenshot {SHOT}", flush=True)

            # --- the tab behaves like a tab -----------------------------------
            page.evaluate(f"() => {SH}.closeTab('spend')")
            titles = page.evaluate(TAB_TITLES)
            check(
                "closing the Spend tab leaves the Consoles tab alone",
                titles == ["Consoles"] and page.evaluate(f"() => {SH}.active") == "consoles",
                "titles={}".format(titles),
            )

            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 21
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
