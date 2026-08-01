"""#359 browser acceptance: the Spend Overview, end to end.

One Playwright pass over a REAL daemon proving the whole page — not just that it
renders, but that each element carries the rule it exists for:

Scenario a  FIVE tiles are laid out in one strip, and every figure in them is
            byte-identical to `/api/spend`'s own `kpis.*_label` — no arithmetic
            and no rounding crept back into JavaScript
Scenario b  the deliveries grid is ordered by cost with the attempt count on the
            row, and the top row's cost includes its FAILED attempt
Scenario c  the models grid names each engine, with the unnameable volume as a
            row rather than a hole
Scenario d  the overhead lines render BESIDE the delivery rows — an overhead
            line inside `.delivery-rows` would read as an issue that cost that
            much
Scenario e  the period control rescopes the WHOLE page: the total, the tiles,
            both grids' row counts and the band's bucket count all move
Scenario f  the activity band draws both series as real pixel heights in the
            same day columns
Scenario g  titles ride the board and are never fetched: with the board cold the
            top row reads `#251` with no title, and a spy over
            `window.WBDaemon.observe` records ZERO `board.list` frames across
            opening Spend, refreshing it and switching the period. One
            deliberate `loadBoard()` later, the title renders — with the
            `board.list` count still at that one deliberate call.

The ledger fixture is dated RELATIVE TO NOW (the route derives `since` from the
real clock), so the 7-day window is meaningful whenever this suite is run: three
issues in the last day, one twenty days back. One line is `unknown` with no
session id, so the figures must read as FLOORS — a fixture that priced cleanly
could not tell a floor from a total.

Boots a Localhost daemon on 7444 over a SCRATCH `RALPHY_DAEMON_DIR` and a scratch
`RALPHY_USAGE_DIR`, so the operator's own daemon registry, login policy and real
ledger are untouched. `RALPHY_PRICING_FILE` is pointed at a written-here table so
every figure is this fixture's. The daemon is stopped by its own subprocess
handle, NEVER by name (`ralphy.exe` doubles as the orchestrator on this host).

Every element assertion is gated on `offsetParent !== null && clientWidth > 0`:
a measurement of a zero-width element passes a "visible" test vacuously
(CONTEXT.md, the vacuous-geometry trap).

Writes docs/screenshots/359-spend-2026-07-31.png.
Run: python crates/ralphy-daemon/tests/wb_spend_359.py   (exit 0 = all pass)
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

PORT = 7444
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_spend_359.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "359-spend-2026-07-31.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

OPUS = "claude-opus-4-8"
HAIKU = "claude-haiku-4-5"

# Dated from the REAL clock, because the route derives `since` from it. A window
# pinned to literal 2026-07 dates would empty out the day after it was written.
NOW = datetime.now(timezone.utc)
RECENT = (NOW - timedelta(days=1)).isoformat()
OLD = (NOW - timedelta(days=20)).isoformat()

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
    d = Path(tempfile.mkdtemp(prefix="wb359_repo_")) / "spend-overview"
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "a.txt").write_text("alpha\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb359@example.com")
    git(d, "config", "user.name", "wb359")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return str(d)


def line(slug, issue, model, session, tokens, outcome, ts, phase="execute"):
    """One ledger line in the on-disk shape core writes (ADR-0008 D6). `session`
    is omitted entirely when absent — that is the shape a pre-ADR-0033 line has,
    and it is what makes a line UNRECOVERABLE."""
    row = {
        "project": slug,
        "actor_email": "wb359@example.com",
        "actor_name": "wb359",
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
    """The fixture the whole suite reads off, at $15 per 1M input:

    #251  two runs — one `done` (1M = $15) and one `timeout` (2M = $30) — so the
          issue cost $45 and two thirds of it bought nothing; plus an `unknown`
          PLAN line with no session id, which makes every figure it touches a
          FLOOR without adding a third attempt
    #300  one `done` run (1M = $15)
    #12   one `done` run on a SECOND engine, twenty days back (200k = $3) — the
          row the 7-day window must drop, from the grid AND from the models grid
    issue 0  the run-level `consolidate` line (1M = $15): real spend, no delivery
    """
    rows = [
        line(slug, 251, OPUS, "s-a", 1_000_000, "done", RECENT),
        line(slug, 251, OPUS, "s-b", 2_000_000, "timeout", RECENT),
        # A PLAN phase, so it adds unpriceable volume to #251 without adding a
        # third attempt — the attempt count is `execute` lines, and nothing else.
        line(slug, 251, "unknown", None, 500_000, "ok", RECENT, phase="plan"),
        line(slug, 300, OPUS, "s-c", 1_000_000, "done", RECENT),
        line(slug, 12, HAIKU, "s-d", 200_000, "done", OLD),
        line(slug, 0, OPUS, "s-e", 1_000_000, "ok", RECENT, phase="consolidate"),
    ]
    Path(usage_dir, "spend-overview.jsonl").write_text("\n".join(rows) + "\n", encoding="utf-8")


def seed_pricing(dir_path):
    """A pricing table written for this test, so every figure below is the
    fixture's and not whatever the host operator put in their own overrides.
    Both engines price identically, so the arithmetic stays checkable by hand."""
    path = Path(dir_path, "pricing.toml")
    body = "input = 15.0\noutput = 75.0\ncache_read = 1.5\ncache_creation = 18.75\n"
    path.write_text(f"[{OPUS}]\n{body}\n[{HAIKU}]\n{body}", encoding="utf-8")
    return str(path)


def empty_env(daemon_dir, usage_dir, pricing_file):
    """A scratch registry, a scratch ledger and EMPTY vendor stores: the operator's
    own daemon dir (and its login policy) is never touched, and the interactive
    scan finds nothing — so every token on screen came from the ledger above."""
    empty = tempfile.mkdtemp(prefix="wb359_empty_")
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


# Everything the Spend page has on screen, read the way an operator sees it: only
# LAID-OUT elements, text trimmed, every bar measured in PIXELS rather than read
# off its declared style. A zero-height bar must not pass as a drawn bar.
PAGE = """
() => {
  const laid = (el) => !!el && el.offsetParent !== null && el.clientWidth > 0;
  const txt = (el) => (laid(el) ? el.textContent.trim() : null);
  const all = (sel) => [...document.querySelectorAll('.spend-tab ' + sel)].filter(laid);
  const one = (sel) => txt(document.querySelector('.spend-tab ' + sel));
  const primary = document.querySelector('.spend-tab .kpi.kpi-primary .spend-figure');
  return {
    // The five tiles, in strip order: the primary's figure then the four the
    // #359 template appends. Read from the TILES, so a figure rendered outside
    // one cannot pass.
    tiles: all('.kpi-strip .kpi').map((tile) => ({
      label: txt(tile.querySelector('.spend-label')),
      value: txt(tile.querySelector('.spend-figure')) ?? txt(tile.querySelector('.kpi-figure')),
      width: tile.getBoundingClientRect().width,
    })),
    figure: txt(primary),
    period: (() => {
      const s = document.querySelector('.spend-tab .spend-period-select');
      return laid(s) ? s.value : null;
    })(),
    deliveries: all('.delivery-rows .delivery-row').map((row) => ({
      issue: txt(row.querySelector('.delivery-issue')),
      title: txt(row.querySelector('.delivery-title')),
      attempts: txt(row.querySelector('.delivery-attempts')),
      value: txt(row.querySelector('.delivery-value')),
      share: txt(row.querySelector('.delivery-share')),
      fill: (row.querySelector('.delivery-fill') || {}).getBoundingClientRect
        ? row.querySelector('.delivery-fill').getBoundingClientRect().width
        : 0,
    })),
    models: all('.model-rows .model-row').map((row) => ({
      model: txt(row.querySelector('.model-name')),
      value: txt(row.querySelector('.model-value')),
      share: txt(row.querySelector('.model-share')),
      unpriced: row.classList.contains('unpriced'),
    })),
    // The overhead block, plus the structural claim that makes it the overhead
    // block: it must NOT live inside the delivery rows.
    overhead: all('.spend-overhead .overhead-line').map((p) => ({
      label: txt(p.querySelector('.overhead-label')),
      value: txt(p.querySelector('.overhead-value')),
    })),
    overheadInsideRows: !!document.querySelector('.spend-tab .delivery-rows .spend-overhead'),
    // Both series measured in pixels, per day column — a bar whose track
    // collapsed renders at zero however its declared height reads.
    band: all('.band-days .band-day').map((day) => {
      const usd = day.querySelector('.band-usd');
      const del = day.querySelector('.band-deliveries');
      return {
        date: txt(day.querySelector('.band-date')),
        usdPx: usd ? usd.getBoundingClientRect().height : 0,
        deliveriesPx: del ? del.getBoundingClientRect().height : 0,
      };
    }),
  };
}
"""

# A spy over the ONE frame sink the workbench talks to the daemon through. It is
# installed BEFORE Spend is opened and never removed, so every verb the page
# sends afterwards is on the record — the negative this suite has to prove
# ("Spend never triggers a board fold") is unprovable without it.
SPY = """
() => {
  window.__verbs = [];
  const real = window.WBDaemon.observe;
  window.WBDaemon.observe = function (verb, payload) {
    window.__verbs.push(verb);
    return real.apply(this, arguments);
  };
}
"""

READY = (
    "() => { const e = document.querySelector('.spend-tab .kpi.kpi-primary .spend-figure');"
    "  return !!e && e.offsetParent !== null && e.clientWidth > 0; }"
)


def main():
    build()
    os.makedirs(SHOT_DIR, exist_ok=True)
    daemon_dir = tempfile.mkdtemp(prefix="wb359_daemon_")
    usage_dir = tempfile.mkdtemp(prefix="wb359_usage_")
    pricing_file = seed_pricing(tempfile.mkdtemp(prefix="wb359_pricing_"))
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

            # The spy goes in BEFORE anything Spend-related happens, so scenario
            # g's zero is a zero over the whole session, not over a suffix of it.
            page.evaluate(SPY)

            # The slug rides as an ARGUMENT, never interpolated: a repo registered
            # from a Windows path carries backslashes a literal would swallow.
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
            page.evaluate(f"() => {SH}.openSpend()")
            page.wait_for_function(f"() => {SH}.active === 'spend'", timeout=10000)
            page.wait_for_function(READY, timeout=15000)

            doc = page.evaluate(
                "(s) => fetch('/api/spend?project=' + encodeURIComponent(s) + '&period=all')"
                "  .then(r => r.json())",
                arg=slug,
            )
            view = page.evaluate(PAGE)

            # --- scenario a · five tiles, all of them the daemon's -------------
            check(
                "the strip carries FIVE laid-out tiles",
                len(view["tiles"]) == 5 and all(t["width"] > 0 for t in view["tiles"]),
                "tiles={}".format([t["label"] for t in view["tiles"]]),
            )
            check(
                "…labelled total cost, deliveries, cost per delivery, retry burn, cache hit",
                [t["label"] for t in view["tiles"]]
                == ["total cost", "deliveries", "cost per delivery", "retry burn", "cache hit"],
                "labels={}".format([t["label"] for t in view["tiles"]]),
            )
            k = doc["kpis"]
            # Byte-identical to the API's own strings: the proof no rounding and
            # no `k`/`M` abbreviation crept back into JavaScript.
            check(
                "…and every figure in them is the daemon's string, verbatim",
                [t["value"] for t in view["tiles"]]
                == [
                    doc["total"],
                    str(k["deliveries"]),
                    k["cost_per_delivery_median_label"],
                    k["retry_burn_label"],
                    k["cache_hit_label"],
                ],
                "screen={} api={}".format(
                    [t["value"] for t in view["tiles"]],
                    [doc["total"], k["deliveries"], k["cost_per_delivery_median_label"]],
                ),
            )
            # $45 (#251) + $15 (#300) + $3 (#12) + $15 consolidation, and a `+`
            # because the fixture's `unknown` line could not be priced.
            check(
                "the total is the fixture's, and reads as a FLOOR",
                view["figure"] == "$78.00+",
                "figure={}".format(view["figure"]),
            )
            check(
                "…three deliveries, two thirds of #251's spend burned on a retry",
                k["deliveries"] == 3 and k["retry_burn_label"] == "38.5%",
                "deliveries={} burn={}".format(k["deliveries"], k["retry_burn_label"]),
            )
            check(
                "…and the cost-per-delivery pair carries the floor marker too",
                k["cost_per_delivery_median_label"] == "$15.00+"
                and k["cost_per_delivery_mean_label"] == "$21.00+",
                "median={} mean={}".format(
                    k["cost_per_delivery_median_label"], k["cost_per_delivery_mean_label"]
                ),
            )

            # --- scenario b · the deliveries grid ------------------------------
            check(
                "the deliveries grid is ordered by cost, descending",
                [d["issue"] for d in view["deliveries"]] == ["#251", "#300", "#12"],
                "rows={}".format([d["issue"] for d in view["deliveries"]]),
            )
            check(
                "…and #251's cost INCLUDES the run that timed out",
                view["deliveries"][0]["value"] == "$45.00+",
                "value={}".format(view["deliveries"][0]["value"]),
            )
            check(
                "…with the attempt count on the row that explains it",
                view["deliveries"][0]["attempts"] == "⟳2",
                "attempts={}".format(view["deliveries"][0]["attempts"]),
            )
            check(
                "…and every row's bar is drawn, not collapsed",
                all(d["fill"] > 0 for d in view["deliveries"]),
                "fills={}".format([d["fill"] for d in view["deliveries"]]),
            )

            # --- scenario c · the models grid ----------------------------------
            check(
                "the models grid names each engine with its cost and share",
                [(m["model"], m["value"], m["share"]) for m in view["models"]][:2]
                == [(OPUS, "$75.00", "96.2%"), (HAIKU, "$3.00", "3.8%")],
                "models={}".format([(m["model"], m["value"], m["share"]) for m in view["models"]]),
            )
            check(
                "…and the volume no engine could be named for is a ROW, not a hole",
                view["models"][-1]["model"] == "unknown"
                and view["models"][-1]["value"] == "~$?"
                and view["models"][-1]["unpriced"],
                "last={}".format(view["models"][-1]),
            )

            # --- scenario d · the overhead lines --------------------------------
            # `interactive` reads `$0.00`, not `~$?`: the vendor stores are empty,
            # so there was no interactive spend — a measured zero. `~$?` is
            # reserved for volume that EXISTS and could not be priced, and
            # spending it on an empty category tells the operator we failed to
            # price something that was never there.
            check(
                "the three overhead lines sum the total beside the grid",
                [(o["label"], o["value"]) for o in view["overhead"]]
                == [
                    ("deliveries", "$63.00+"),
                    ("interactive", "$0.00"),
                    ("consolidation", "$15.00"),
                ],
                "overhead={}".format([(o["label"], o["value"]) for o in view["overhead"]]),
            )
            # The placement IS the rule: an overhead line among the delivery rows
            # would read as an issue that cost that much.
            check(
                "…rendered OUTSIDE `.delivery-rows`, never as a row in it",
                view["overheadInsideRows"] is False,
            )

            # --- scenario f · the activity band --------------------------------
            # (read before the period switch, while `all` shows only active days)
            check(
                "the activity band draws one column per active day",
                len(view["band"]) == 2,
                "days={}".format([d["date"] for d in view["band"]]),
            )
            busy = view["band"][-1]
            check(
                "…with BOTH series drawn as real pixel heights in the same column",
                busy["usdPx"] > 1 and busy["deliveriesPx"] > 1,
                "usd={} deliveries={}".format(busy["usdPx"], busy["deliveriesPx"]),
            )
            # The peak column alone proves nothing: both shares are 1.0 there by
            # construction, so a binding hardcoded to `height:100%` — or one
            # that SWAPPED the two series — would pass on it. The quiet column
            # is the only discriminating one. #12 cost $3 of the $75 peak day
            # (a short bar) but is 1 delivery against that day's 2 (a tall one),
            # so the two series must disagree, and in that direction.
            quiet = view["band"][0]
            check(
                "…and the non-peak column discriminates the two series",
                1 < quiet["usdPx"] < quiet["deliveriesPx"] < busy["deliveriesPx"],
                "quiet usd={} deliveries={} peak deliveries={}".format(
                    quiet["usdPx"], quiet["deliveriesPx"], busy["deliveriesPx"]
                ),
            )

            page.screenshot(path=SHOT, full_page=True)
            print(f"[INFO] screenshot {SHOT}", flush=True)

            # --- scenario g · titles ride the board, never fetch it -------------
            # The board was never loaded, so the row carries its number and NO
            # title — and, the claim that matters, no frame was sent to get one.
            check(
                "with the board cold the top row is `#251` with no title",
                view["deliveries"][0]["title"] is None,
                "title={}".format(view["deliveries"][0]["title"]),
            )
            page.evaluate(f"() => {SH}.loadSpend()")
            page.wait_for_function(f"() => {SH}.spend.loading === false", timeout=15000)
            verbs = page.evaluate("() => window.__verbs.slice()")
            check(
                "…and opening + refreshing Spend sent ZERO `board.list` frames",
                verbs.count("board.list") == 0,
                "verbs={}".format(verbs),
            )

            # --- scenario e · the period rescopes the page ----------------------
            page.evaluate(f"() => {SH}.setSpendPeriod('7d')")
            page.wait_for_function(f"() => {SH}.spend.loading === false", timeout=15000)
            page.wait_for_function(
                "() => { const e = document.querySelector('.spend-tab .kpi.kpi-primary .spend-figure');"
                "  return !!e && e.clientWidth > 0 && e.textContent.trim() === '$75.00+'; }",
                timeout=15000,
            )
            week = page.evaluate(PAGE)
            check(
                "switching the period to 7d moves the total",
                week["figure"] == "$75.00+" and week["period"] == "7d",
                "figure={} period={}".format(week["figure"], week["period"]),
            )
            check(
                "…and the deliveries tile, which drops the 20-day-old issue",
                week["tiles"][1]["value"] == "2",
                "deliveries={}".format(week["tiles"][1]["value"]),
            )
            check(
                "…and the deliveries grid, which loses that issue's row",
                [d["issue"] for d in week["deliveries"]] == ["#251", "#300"],
                "rows={}".format([d["issue"] for d in week["deliveries"]]),
            )
            check(
                "…and the models grid, which loses the engine only it used",
                [m["model"] for m in week["models"]] == [OPUS, "unknown"],
                "models={}".format([m["model"] for m in week["models"]]),
            )
            # A bounded window emits every one of its days, zero-filled: a quiet
            # Tuesday must read as quiet rather than vanish and let two busy days
            # sit side by side.
            check(
                "…and the band, which zero-fills all seven days of the window",
                len(week["band"]) == 7,
                "days={}".format([d["date"] for d in week["band"]]),
            )
            verbs = page.evaluate("() => window.__verbs.slice()")
            check(
                "…all of it without a single `board.list` frame",
                verbs.count("board.list") == 0,
                "verbs={}".format(verbs),
            )

            # --- scenario g (second half) · the title, once the board holds it --
            # ONE deliberate board fold. Whether it succeeds against a remoteless
            # fixture is not this suite's subject; that it is the ONLY frame is.
            page.evaluate(f"() => {SH}.loadBoard()")
            page.wait_for_function(
                "() => (window.__verbs || []).includes('board.list')", timeout=15000
            )
            deliberate = page.evaluate("() => window.__verbs.filter(v => v === 'board.list').length")
            # The rows the board holds are what the title is read FROM — seeded
            # directly, because a fixture repo seeded with plain `git init` is
            # remoteless and its real fold has no tracker to answer it.
            page.evaluate(
                f"(s) => {{ {SH}.boardIssues[s] = [{{ number: 251, title: 'the retry that cost $30' }}]; }}",
                arg=slug,
            )
            page.evaluate(f"() => {SH}.setSpendPeriod('all')")
            page.wait_for_function(f"() => {SH}.spend.loading === false", timeout=15000)
            page.wait_for_function(
                "() => { const e = document.querySelector('.spend-tab .delivery-title');"
                "  return !!e && e.offsetParent !== null && e.clientWidth > 0; }",
                timeout=15000,
            )
            titled = page.evaluate(PAGE)
            check(
                "once the board holds the issue, the row carries its title",
                titled["deliveries"][0]["title"] == "the retry that cost $30",
                "title={}".format(titled["deliveries"][0]["title"]),
            )
            after = page.evaluate("() => window.__verbs.filter(v => v === 'board.list').length")
            check(
                "…with the `board.list` count still at that ONE deliberate load",
                after == deliberate == 1,
                "deliberate={} after={}".format(deliberate, after),
            )

            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    # A deleted scenario silently lowers the bar: the floor is the literal number
    # of checks this suite is known to run.
    check_floor = 29
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        results.append(False)
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
