"""#360 browser acceptance: the Spend tab's Ledger pane, end to end.

One Playwright pass over a REAL daemon proving the pane the Usage modal was
replaced by — not that it renders, but that each element carries the rule it
exists for:

Scenario a  the `Overview | Ledger` toggle SWITCHES the pane: the grid becomes
            laid out and the KPI strip stops being laid out. Both directions are
            asserted, because a pane that only ever appears is not a toggle
Scenario b  the grid carries every dimension the ledger record has, in the fixed
            column order, and the top row's cells are the fixture line's own
            values — including the four RAW token counts, unabbreviated
Scenario c  the grid is SCOPED: not one row belongs to the second fixture
            project, which shares the ledger file with the open one
Scenario d  the Overview's unpriced figure is the drill-down: clicking it lands
            on the Ledger pane, filtered, showing ONLY the unpriceable rows —
            2 of the open project's 3 — with a chip that says so and clears

The ledger fixture is dated RELATIVE TO NOW (the route derives `since` from the
real clock), so it stays meaningful whenever this suite is run. Two of the open
project's three lines are unpriceable and for DIFFERENT reasons — an `unknown`
model that still carries its `session_id` (*recoverable*) and a real model absent
from the written price table (*no_price*) — because a filter that only ever saw
one cause could be a filter on that one cause.

Boots a Localhost daemon on 7445 over a SCRATCH `RALPHY_DAEMON_DIR` and a scratch
`RALPHY_USAGE_DIR`, so the operator's own daemon registry, login policy and real
ledger are untouched. `RALPHY_PRICING_FILE` is pointed at a written-here table so
every verdict is this fixture's. The daemon is stopped by its own subprocess
handle, NEVER by name (`ralphy.exe` doubles as the orchestrator on this host).

Every element assertion is gated on `offsetParent !== null && clientWidth > 0`:
a measurement of a zero-width element passes a "visible" test vacuously
(CONTEXT.md, the vacuous-geometry trap).

Writes docs/screenshots/360-spend-ledger-2026-07-31.png.
Run: python crates/ralphy-daemon/tests/wb_spend_360.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7445
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_spend_360.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "360-spend-ledger-2026-07-31.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

OPUS = "claude-opus-4-8"
# A REAL model id that is deliberately ABSENT from the price table written below:
# unpriceable for a reason no session store can fix, which is what separates
# `no_price` from `recoverable`.
UNPRICED_MODEL = "big-pickle-3"
# The second project shares the ledger file with the open one, so scenario c's
# zero is a zero over rows the route actually had in hand.
OTHER_SLUG = "acme/other-repo"

# The column order the grid is contracted to, header labels verbatim.
COLUMNS = [
    "kind",
    "project",
    "issue",
    "phase",
    "agent",
    "model",
    "outcome",
    "actor",
    "version",
    "when",
    "↑ input",
    "⚡ cache read",
    "❄ cache write",
    "↓ output",
]

# Dated from the REAL clock, because the route derives `since` from it.
NOW = datetime.now(timezone.utc)
RECENT = (NOW - timedelta(days=1)).isoformat()

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
    """A committed git repo — the Spend tab needs a project to scope ON."""
    d = Path(tempfile.mkdtemp(prefix="wb360_repo_")) / "spend-ledger"
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "a.txt").write_text("alpha\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb360@example.com")
    git(d, "config", "user.name", "wb360")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return str(d)


def line(slug, issue, model, session, tokens, outcome, ts, phase="execute"):
    """One ledger line in the on-disk shape core writes (ADR-0008 D6). `session`
    is omitted entirely when absent — that is the shape a pre-ADR-0033 line has,
    and it is what makes a line UNRECOVERABLE rather than merely unpriced."""
    row = {
        "project": slug,
        "actor_email": "wb360@example.com",
        "actor_name": "wb360",
        "ralphy_version": "0.0.0-test",
        "issue": issue,
        "phase": phase,
        "agent": "claude",
        "model": model,
        "outcome": outcome,
        "tokens": {"input": tokens, "output": 0, "cache_read": 0, "cache_creation": 0},
        "ts": ts,
    }
    if session:
        row["session_id"] = session
    return json.dumps(row)


def seed_ledger(usage_dir, slug):
    """Five lines in ONE file, so the grid's row ORDER is the file's and the
    scoping assertion is over rows the route genuinely had in hand:

    open  #251 execute  opus, priced        — the negative control: no cause
    open  #251 plan     unknown + session   — *recoverable*
    open  #300 execute  a model with no price row — *no_price*
    other #7            opus, priced        — must never reach the grid
    other #8            unknown, no session — nor this one, cause or not
    """
    rows = [
        line(slug, 251, OPUS, "s-a", 1_000_000, "done", RECENT),
        line(slug, 251, "unknown", "s-b", 400_000, "timeout", RECENT, phase="plan"),
        line(slug, 300, UNPRICED_MODEL, "s-c", 60_000, "done", RECENT),
        line(OTHER_SLUG, 7, OPUS, "s-d", 5_000_000, "done", RECENT),
        line(OTHER_SLUG, 8, "unknown", None, 250_000, "ok", RECENT),
    ]
    Path(usage_dir, "fixture.jsonl").write_text("\n".join(rows) + "\n", encoding="utf-8")


def seed_pricing(dir_path):
    """A pricing table written for this test. It knows exactly ONE engine, which
    is what makes `UNPRICED_MODEL` a `no_price` line rather than a priced one."""
    path = Path(dir_path, "pricing.toml")
    body = "input = 15.0\noutput = 75.0\ncache_read = 1.5\ncache_creation = 18.75\n"
    path.write_text(f"[{OPUS}]\n{body}", encoding="utf-8")
    return str(path)


def empty_env(daemon_dir, usage_dir, pricing_file):
    """A scratch registry, a scratch ledger and EMPTY vendor stores: the operator's
    own daemon dir (and its login policy) is never touched, and the interactive
    scan finds nothing — so every row on screen came from the ledger above."""
    empty = tempfile.mkdtemp(prefix="wb360_empty_")
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


# The Ledger pane as an operator sees it: only LAID-OUT elements, text trimmed.
# `kpiLaid` is read in the SAME pass as the grid, so scenario a's two halves are
# one observation of one moment rather than two reads that could straddle a flip.
PANE = """
() => {
  const laid = (el) => !!el && el.offsetParent !== null && el.clientWidth > 0;
  const txt = (el) => (laid(el) ? el.textContent.trim() : null);
  const grid = document.querySelector('.spend-tab .ledger-grid');
  const chip = document.querySelector('.spend-tab .ledger-chip');
  return {
    gridLaid: laid(grid),
    kpiLaid: laid(document.querySelector('.spend-tab .kpi-strip')),
    columns: grid ? [...grid.querySelectorAll('thead th')].map((th) => th.textContent.trim()) : [],
    rows: [...document.querySelectorAll('.spend-tab .ledger-row')]
      .filter(laid)
      .map((tr) => ({
        cells: [...tr.querySelectorAll('td')].map((td) => td.textContent.trim()),
        unpriced: tr.classList.contains('unpriced'),
        // The CAUSE WORD, read off the cell the operator reads. The class alone
        // is a boolean; `recoverable` vs `no_price` is the whole point of the
        // split, and only this proves the vocabulary reached the screen.
        cause: txt(tr.querySelector('.ledger-cause')),
      })),
    chip: txt(chip),
  };
}
"""

GRID_READY = (
    "() => { const e = document.querySelector('.spend-tab .ledger-grid');"
    "  return !!e && e.offsetParent !== null && e.clientWidth > 0"
    "    && e.querySelectorAll('tbody tr').length > 0; }"
)

OVERVIEW_READY = (
    "() => { const e = document.querySelector('.spend-tab .kpi.kpi-primary .spend-figure');"
    "  return !!e && e.offsetParent !== null && e.clientWidth > 0; }"
)


def main():
    build()
    os.makedirs(SHOT_DIR, exist_ok=True)
    daemon_dir = tempfile.mkdtemp(prefix="wb360_daemon_")
    usage_dir = tempfile.mkdtemp(prefix="wb360_usage_")
    pricing_file = seed_pricing(tempfile.mkdtemp(prefix="wb360_pricing_"))
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
            ctx = browser.new_context(viewport={"width": 1440, "height": 1100})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)

            # The slug rides as an ARGUMENT, never interpolated: a repo registered
            # from a Windows path carries backslashes a literal would swallow.
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
            page.evaluate(f"() => {SH}.openSpend()")
            page.wait_for_function(f"() => {SH}.active === 'spend'", timeout=10000)
            page.wait_for_function(OVERVIEW_READY, timeout=15000)

            before = page.evaluate(PANE)
            check(
                "the Spend tab lands on the Overview, with no grid on screen",
                before["kpiLaid"] and not before["gridLaid"],
                "kpi={} grid={}".format(before["kpiLaid"], before["gridLaid"]),
            )

            # --- scenario a · the toggle switches the pane ---------------------
            # Clicked as the operator does, through the real control — not by
            # assigning `spendPane`, which would prove the state and not the UI.
            page.click('.spend-tab .spend-pane-toggle button[data-pane="ledger"]')
            page.wait_for_function(GRID_READY, timeout=15000)
            view = page.evaluate(PANE)
            check(
                "the Ledger toggle lays out the grid and takes the KPI strip away",
                view["gridLaid"] and not view["kpiLaid"],
                "grid={} kpi={}".format(view["gridLaid"], view["kpiLaid"]),
            )

            # --- scenario b · every dimension, in order ------------------------
            check(
                "the grid's header carries all fourteen columns, in order",
                view["columns"] == COLUMNS,
                "columns={}".format(view["columns"]),
            )
            check(
                "…one row per ledger line of the open project",
                len(view["rows"]) == 3,
                "rows={}".format([r["cells"][:3] for r in view["rows"]]),
            )
            # The FIRST fixture line, cell by cell. The four counts are RAW: an
            # abbreviation reintroduced in JavaScript would read `1.0M` here.
            check(
                "…and the top row is the fixture line's own values, unabbreviated",
                view["rows"][0]["cells"]
                == [
                    "ledger",
                    slug,
                    "#251",
                    "execute",
                    "claude",
                    OPUS,
                    "done",
                    "wb360",
                    "0.0.0-test",
                    RECENT,
                    "1000000",
                    "0",
                    "0",
                    "0",
                ],
                "cells={}".format(view["rows"][0]["cells"]),
            )
            # The verdict is the daemon's, per row, and it discriminates: the
            # priced line carries no mark while the other two do.
            check(
                "…with the daemon's unpriced verdict marking exactly the two offenders",
                [r["unpriced"] for r in view["rows"]] == [False, True, True],
                "marks={}".format([r["unpriced"] for r in view["rows"]]),
            )

            # --- scenario c · the grid is scoped to the open project -----------
            projects = sorted({r["cells"][1] for r in view["rows"]})
            check(
                "every row belongs to the open project, and the other one has none",
                projects == [slug] and OTHER_SLUG not in projects,
                "projects={}".format(projects),
            )
            # The route, independently of the DOM: the second project's rows are
            # not merely unrendered, they never crossed the wire.
            body = page.evaluate(
                "(s) => fetch('/api/usage?project=' + encodeURIComponent(s)).then(r => r.text())",
                arg=slug,
            )
            check(
                "…and the response itself never carries the second project",
                body.count(OTHER_SLUG) == 0,
                "occurrences={}".format(body.count(OTHER_SLUG)),
            )

            # --- scenario d · the unpriced figure is the drill-down ------------
            page.click('.spend-tab .spend-pane-toggle button[data-pane="overview"]')
            # Waits on the GRID going away as well as the strip coming back: the
            # toggle's other direction is a claim of its own, and a pane that only
            # ever appears is not a toggle.
            page.wait_for_function(
                "() => { const k = document.querySelector('.spend-tab .kpi.kpi-primary .spend-figure');"
                "  const g = document.querySelector('.spend-tab .ledger-grid');"
                "  return !!k && k.offsetParent !== null && k.clientWidth > 0"
                "    && (!g || g.offsetParent === null); }",
                timeout=15000,
            )
            back = page.evaluate(PANE)
            check(
                "the Overview toggle takes the grid away and brings the strip back",
                back["kpiLaid"] and not back["gridLaid"],
                "kpi={} grid={}".format(back["kpiLaid"], back["gridLaid"]),
            )

            # The gold figure in the GAP panel, identified by the rows that panel
            # owns rather than by position — a sibling panel with the same classes
            # has silently retargeted an unscoped selector here before.
            page.click(".spend-tab .spend-panel.gap .spend-panel-figure.gold")
            # Gated on the FILTERED row count, not on `GRID_READY`: the grid is
            # laid out with all three rows for a frame after the pane flips, and a
            # read that lands there samples the pre-filter render.
            page.wait_for_function(
                "() => { const g = document.querySelector('.spend-tab .ledger-grid');"
                "  return !!g && g.offsetParent !== null && g.clientWidth > 0"
                "    && g.querySelectorAll('tbody tr').length === 2; }",
                timeout=15000,
            )
            drilled = page.evaluate(PANE)
            pane = page.evaluate(f"() => {SH}.spendPane")
            check(
                "clicking the unpriced figure lands on the Ledger pane",
                pane == "ledger" and drilled["gridLaid"],
                "pane={} grid={}".format(pane, drilled["gridLaid"]),
            )
            check(
                "…with a chip on screen saying the grid is filtered",
                (drilled["chip"] or "").startswith("unpriced only"),
                "chip={}".format(drilled["chip"]),
            )
            # 2 of 3, not 3: a filter that showed everything would still "land on
            # the Ledger pane" and still show the chip.
            check(
                "…showing ONLY the unpriceable rows — 2 of the project's 3",
                len(drilled["rows"]) == 2 and all(r["unpriced"] for r in drilled["rows"]),
                "rows={}".format([r["cells"][:6] for r in drilled["rows"]]),
            )
            # The two are unpriceable for DIFFERENT reasons, so the filter is on
            # "has a cause" and not on one cause it happened to be written against.
            models = sorted(r["cells"][5] for r in drilled["rows"])
            check(
                "…and they are the recoverable one AND the unpriceable-model one",
                models == sorted(["unknown", UNPRICED_MODEL]),
                "models={}".format(models),
            )
            # The daemon's CAUSE WORD on screen, not merely a highlight class: a
            # row the operator cannot tell `recoverable` from `lost` on is a gap
            # with no owner, which is what this drill-down exists to give it.
            causes = sorted(r["cause"] for r in drilled["rows"])
            check(
                "…each labelled with the daemon's own cause word",
                causes == ["no_price", "recoverable"],
                "causes={}".format(causes),
            )

            page.screenshot(path=SHOT, full_page=True)
            print(f"[INFO] screenshot {SHOT}", flush=True)

            # Clearing the chip is what makes the filter a filter rather than a
            # one-way door: the operator arrived here by a click and must be able
            # to see the rest of the ledger without leaving the pane.
            page.click(".spend-tab .ledger-chip-clear")
            # Waits on the CHIP's own geometry, not only on the row count:
            # Alpine applies an `x-show` flip after the tick that changed the
            # data, so a read gated on the rows alone samples a chip that is
            # still laid out and the assertion below reds for nothing.
            page.wait_for_function(
                "() => { const c = document.querySelector('.spend-tab .ledger-chip');"
                "  return (!c || c.offsetParent === null)"
                "    && document.querySelectorAll('.spend-tab .ledger-row').length === 3; }",
                timeout=15000,
            )
            cleared = page.evaluate(PANE)
            check(
                "clearing the chip restores the full grid, still on the Ledger pane",
                len(cleared["rows"]) == 3 and cleared["chip"] is None and cleared["gridLaid"],
                "rows={} chip={}".format(len(cleared["rows"]), cleared["chip"]),
            )

            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    # A deleted scenario silently lowers the bar: the floor is the literal number
    # of checks this suite is known to run.
    check_floor = 17
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        results.append(False)
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
