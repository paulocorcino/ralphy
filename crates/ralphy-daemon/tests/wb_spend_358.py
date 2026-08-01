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
  // The panel that OWNS a given kind of row. #359 put four more `.spend-panel`
  // blocks on this page, so "the first panel figure" stopped naming the tokens
  // panel — the rows a panel contains are what identify it, not its position.
  const panel = (rows) => {
    const list = document.querySelector('.spend-tab ' + rows);
    return list ? list.closest('.spend-panel') : null;
  };
  const inPanel = (rows, sel) => {
    const p = panel(rows);
    const e = p && p.querySelector(sel);
    return laid(e) ? e.textContent.trim() : null;
  };
  const pane = document.querySelector('.spend-tab');
  return {
    visible: laid(pane),
    bar: laid(document.querySelector('.spend-tab .spend-bar')),
    project: txt('.spend-tab .spend-project'),
    blankTitle: txt('.spend-tab .spend-blank-title'),
    blankHint: txt('.spend-tab .spend-blank-hint'),
    figure: txt('.spend-tab .spend-figure'),
    caveat: txt('.spend-tab .spend-caveat'),
    // The composition PRD #355 fixed: the total is the primary TILE of a strip,
    // and the strip has to have room left in it for #359's other four.
    strip: (() => {
      const strip = document.querySelector('.spend-tab .kpi-strip');
      const tile = document.querySelector('.spend-tab .kpi-strip .kpi.kpi-primary');
      if (!laid(strip) || !laid(tile)) return null;
      return {
        // The figure is INSIDE the tile — not a sibling that merely looks like one.
        holdsFigure: !!tile.querySelector('.spend-figure'),
        tileWidth: tile.getBoundingClientRect().width,
        stripWidth: strip.getBoundingClientRect().width,
        // A section marker on each panel heading, measured: a `::before` that
        // collapsed to nothing is the failure this reads for.
        markers: [...document.querySelectorAll('.spend-tab .spend-panel-head .spend-label')]
          .filter(laid)
          .map(l => parseFloat(getComputedStyle(l, '::before').width) || 0),
      };
    })(),
    meterLine: txt('.spend-tab .meter-line'),
    tokensTotal: inPanel('.meter-rows', '.spend-panel-figure'),
    unpricedTotal: txt('.spend-tab .spend-panel-figure.gold'),
    unpricedShare: inPanel('.cause-rows', '.spend-panel-note'),
    coverage: (() => {
      const track = document.querySelector('.spend-tab .cov-track');
      if (!laid(track)) return null;
      const priced = track.querySelector('.cov-priced');
      const unpriced = track.querySelector('.cov-unpriced');
      return {
        priced: priced ? priced.style.width : null,
        unpriced: unpriced ? unpriced.style.width : null,
        // Widths as MEASURED, not as declared: a bar whose track collapsed
        // renders both segments at zero however the percentages read.
        pricedPx: priced ? priced.getBoundingClientRect().width : 0,
        unpricedPx: unpriced ? unpriced.getBoundingClientRect().width : 0,
      };
    })(),
    meterRows: [...document.querySelectorAll('.spend-tab .meter-row')]
      .filter(laid)
      .map(r => ({
        glyph: r.querySelector('.meter-glyph')?.textContent.trim(),
        name: r.querySelector('.meter-name')?.textContent.trim(),
        value: r.querySelector('.meter-value')?.textContent.trim(),
        share: r.querySelector('.meter-share')?.textContent.trim(),
        width: r.querySelector('.meter-fill')?.style.width,
      })),
    causes: [...document.querySelectorAll('.spend-tab .cause-row')]
      .filter(laid)
      .map(c => ({
        key: [...c.classList].find(k => k.startsWith('cause-') && k !== 'cause-row'),
        title: c.querySelector('.cause-title')?.textContent.trim(),
        value: c.querySelector('.cause-value')?.textContent.trim(),
        share: c.querySelector('.cause-share')?.textContent.trim(),
        width: c.querySelector('.cause-fill')?.style.width,
        hint: c.querySelector('.cause-hint')?.textContent.trim(),
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
                "() => { const e = document.querySelector('.spend-tab .spend-blank-title');"
                "  return !!e && e.offsetParent !== null && e.clientWidth > 0; }",
                timeout=10000,
            )
            pane = page.evaluate(PANE)
            check("the Spend pane is on screen", pane["visible"])
            check(
                "with no project open it tells the operator to open one",
                pane["blankTitle"] == "No project open" and "Open a project" in (pane["blankHint"] or ""),
                "title={} hint={}".format(pane["blankTitle"], pane["blankHint"]),
            )
            # The strip lives OUTSIDE the state branches: a pane with nothing to
            # show must still read as the Spend tab, not as a blank canvas.
            check("…while the tab's own header strip stays on screen", pane["bar"])
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
                pane["project"] == slug,
                "project={}".format(pane["project"]),
            )
            # 1M input at $15/1M — the only priceable line in the fixture.
            check(
                "it shows the project's total cost",
                (pane["figure"] or "").startswith("$15.00"),
                "figure={}".format(pane["figure"]),
            )
            check(
                "…with the token_meter split beside it, in the terminal's vocabulary",
                pane["meterLine"] == "↑1.8M ⚡0 ❄0 ↓0",
                "meter={}".format(pane["meterLine"]),
            )
            # The split is also laid out as four comparable rows — all four, so a
            # kind with nothing in it reads as "nothing was reused" rather than
            # as a column that does not exist.
            check(
                "…and as four rows, one per token kind, none of them dropped",
                [r["name"] for r in pane["meterRows"]] == ["input", "cache read", "cache write", "output"],
                "rows={}".format([r["name"] for r in pane["meterRows"]]),
            )
            check(
                "…each carrying the daemon's value and share",
                pane["meterRows"][0]["value"] == "1.8M" and pane["meterRows"][0]["share"] == "100.0%",
                "row0={}".format(pane["meterRows"][0]),
            )
            check(
                "…and the panel states the token total",
                pane["tokensTotal"] == "1.8M",
                "total={}".format(pane["tokensTotal"]),
            )

            # --- scenario d · the total is a floor ----------------------------
            check(
                "a total carrying unpriceable volume renders as a FLOOR",
                pane["figure"] == "$15.00+",
                "figure={}".format(pane["figure"]),
            )
            # Not merely the word "floor": the caveat must name the volume that
            # made it one, because "some of it" is not something to act on.
            check(
                "…and says in words that it is one, naming the volume responsible",
                (pane["caveat"] or "").startswith("a floor")
                and "750.0k" in (pane["caveat"] or "")
                and "42.9%" in (pane["caveat"] or ""),
                "caveat={}".format(pane["caveat"]),
            )
            # The coverage bar: priced vs unpriced as a proportion, measured in
            # PIXELS as well as declared, so a collapsed track cannot pass.
            # The declared width is asserted by PREFIX: the browser round-trips
            # `style.width` through its own precision, so pinning every digit
            # would red on a Chromium that serialises one fewer.
            cov = pane["coverage"] or {}
            check(
                "…and a coverage bar shows how much of the volume was priced",
                (cov.get("priced") or "").startswith("57.14")
                and (cov.get("unpriced") or "").startswith("42.85")
                and cov.get("pricedPx", 0) > cov.get("unpricedPx", 0) > 0,
                "coverage={}".format(cov),
            )

            # --- scenario d2 · the composition #359 has to append to ----------
            # PRD #355: "five tiles carry the executive read — total cost as a
            # floor, deliveries, cost per delivery, retry burn, cache hit". This
            # slice carries the first. What is checked is that it is a TILE IN A
            # STRIP and not a band: a band would have to be demolished for #359,
            # and the reason this issue is HITL is that every later component of
            # the Spend view copies whatever shape is accepted here.
            strip = pane["strip"] or {}
            check(
                "the total is the primary tile of a KPI strip, not a band of its own",
                strip.get("holdsFigure") is True,
                "strip={}".format(strip),
            )
            check(
                "…and the strip keeps room for #359's four remaining tiles",
                0 < strip.get("tileWidth", 0) < strip.get("stripWidth", 0) * 0.62,
                "tile={} strip={}".format(strip.get("tileWidth"), strip.get("stripWidth")),
            )
            check(
                "…and each panel heading below carries a section marker",
                # The claim is that the `::before` RENDERS on every panel
                # heading, not how many headings there are — #359 added more.
                len(strip.get("markers") or []) >= 2
                and all(w > 0 for w in strip["markers"]),
                "markers={}".format(strip.get("markers")),
            )

            # --- scenario e · the unpriced split ------------------------------
            check(
                "the unpriced volume is a first-class element beside the total",
                pane["unpricedTotal"] == "750.0k",
                "unpriced={}".format(pane["unpricedTotal"]),
            )
            causes = {c["title"]: c for c in pane["causes"]}
            check(
                "…split into recoverable and lost (ADR-0053 D4)",
                causes.get("recoverable", {}).get("value") == "500.0k"
                and causes.get("lost", {}).get("value") == "250.0k",
                "causes={}".format({k: v.get("value") for k, v in causes.items()}),
            )
            check(
                "…each carrying its share OF THE GAP and a bar drawn to it",
                causes.get("recoverable", {}).get("share") == "66.7%"
                and (causes.get("recoverable", {}).get("width") or "").startswith("66.6")
                and causes.get("lost", {}).get("share") == "33.3%",
                "shares={}".format({k: v.get("share") for k, v in causes.items()}),
            )
            # The hint is ON SCREEN, never a tooltip: which half of the gap is
            # worth working on is the reason the split exists at all.
            check(
                "…and each says on screen what it means, not behind a hover",
                "session id" in (causes.get("recoverable", {}).get("hint") or "")
                and "no amount of work" in (causes.get("lost", {}).get("hint") or ""),
                "hints={}".format({k: v.get("hint") for k, v in causes.items()}),
            )
            check(
                "…and the share of the project's tokens is stated",
                "42.9%" in (pane["unpricedShare"] or ""),
                "share={}".format(pane["unpricedShare"]),
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
                doc["tokens"]["meter"] == pane["meterLine"],
                "api={} screen={}".format(doc["tokens"]["meter"], pane["meterLine"]),
            )
            check(
                "…and so is every percentage, so no rounding lives there either",
                [p["share_label"] for p in doc["tokens"]["parts"]]
                == [r["share"] for r in pane["meterRows"]],
                "api={} screen={}".format(
                    [p["share_label"] for p in doc["tokens"]["parts"]],
                    [r["share"] for r in pane["meterRows"]],
                ),
            )
            by_key = {c["key"]: c for c in doc["unpriced"]["causes"]}
            check(
                "an unknown model contributes to the unpriced split, never $0",
                by_key["recoverable"]["tokens"] == 500_000
                and by_key["lost"]["tokens"] == 250_000
                and "no_price" not in by_key
                and doc["floor"] is True,
                "unpriced={}".format(doc["unpriced"]["causes"]),
            )
            # Not a substring hunt over the body — `"parts"` contains `"ts"`, and
            # that false positive is exactly the kind of assertion that rots. The
            # claim is structural: a FIXED set of top-level keys, every row array
            # among them CAPPED by the daemon, and a body that stays bounded
            # however long the ledger gets. #359 added the grids, the tiles, the
            # band and the period; the bound rose with them and stays a bound.
            check(
                "the response is a summary, not the ledger",
                sorted(doc.keys())
                == [
                    "activity",
                    "deliveries",
                    "deliveries_truncated",
                    "floor",
                    "kpis",
                    "models",
                    "overhead",
                    "period",
                    "project",
                    "tokens",
                    "total",
                    "unpriced",
                    "usd",
                ]
                and len(json.dumps(doc)) < 60_000,
                "keys={} bytes={}".format(sorted(doc.keys()), len(json.dumps(doc))),
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
    check_floor = 32
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
