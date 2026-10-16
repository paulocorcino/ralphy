"""Browser acceptance: a Mermaid file (`.mmd`) opens as a drawn diagram, and a
database markup file (`.dbml`) opens as a grouped grid of table cards.

One Playwright pass over a REAL daemon. A fixture repo holds three Mermaid
files under `docs/`: a small flowchart, a file Mermaid rejects, and an
`erDiagram` over Mermaid's default limit of 50,000 characters. It also holds
three synthetic DBML files: a schema, a table whose name is markup, and a
file with comments only.

Scenario 1  `flow.mmd` opens as one diagram (one svg, with the label `Start`)
Scenario 2  Edit shows the file text in the editor; Preview draws it again
Scenario 3  the diagram has one Enlarge button, and the pane shows no heading
            index (no outline, no Contents button)
Scenario 4  a change on disk draws the diagram again
Scenario 5  `broken.mmd` says "Mermaid error:" in the pane
Scenario 6  `big.mmd` (more than 50,000 characters) is drawn, all 120 entities
Scenario 7  the detached file window draws the diagram
Scenario 8  `schema.dbml`: one card per table, grouped by name prefix, the
            side index lists groups and tables, the summary counts the lines
            not understood, only the exceptions are tagged, no card overflows
Scenario 9  Edit on the `.dbml` tab shows the source; Preview shows the cards
Scenario 10 a table name with markup shows as text; a file with no tables
            says so
Scenario 11 Find filters the cards, a match in a hidden column opens its card,
            closing the bar shows every card again
Scenario 12 a long card shows 8 columns until it is opened; Expand all and
            Collapse all; a click on a column marks the columns with the same
            name
Scenario 13 a ref is a link to its table, a table lists the columns that point
            to it, a ref to a missing table is plain text
Scenario 14 a project search opens the grid already filtered
Scenario 15 the selected tables open as a diagram in the zoom view; a group is
            selected at once; the draw button keeps the limit of 12 tables;
            Escape closes the view and the focus returns to the button; a
            table name with markup is drawn as text

Requires `playwright` (`pip install playwright`, then
`playwright install chromium`).

Boots a Localhost daemon over a SCRATCH `RALPHY_DAEMON_DIR`. The daemon is
stopped by its own subprocess handle, NEVER by name.

Writes .ralphy/screenshots/rendered-files-*.png.
Run: python tests/browser/files/wb_rendered_files.py   (exit 0 = all pass)
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

PORT = 7581
BASE = f"http://127.0.0.1:{PORT}/"

# tests/browser/files/wb_rendered_files.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
TABS = "Alpine.store('tabs')"
FILES = "Alpine.$data(document.querySelector('[x-data=\"wbFiles\"]'))"

FLOW = "flowchart TD\n  A[Start] --> B[Finish]\n"
CHANGED = "flowchart TD\n  A[Changed] --> B[Finish]\n"
BROKEN = "flowchart TD\n  A[Start --> (((\n  this is not mermaid ]]]\n"
BIG_ENTITIES = 120
BIG_COLUMNS = 8
MERMAID_DEFAULT_LIMIT = 50000

# Synthetic. Ten tables: `ledger` (2), `shop` (3) and `Other tables` (5).
# Two lines are garbage, which the summary line must count. `stock_levels` has
# 12 columns, so it is closed; its column 10 and `ab_cust`, `cd_cust` hold the
# `xx_nb_owner` names. `shop_items.order_id` and `cd_cust.order_ref` point to
# `shop_orders`; `shop_users.country_id` points to a table that is not here.
SCHEMA = """\
// A synthetic shop schema.
this line is garbage

Table shop_orders {
  id int [pk, not null]
  buyer varchar(40)
  total decimal(10, 2) [not null]
  a_very_long_column_name_that_goes_on_for_a_long_while decimal(10, 2) [unique]
  Note: 'Orders of the shop'
}

Table shop_items {
  id int [pk, not null]
  sku varchar(20) [not null, unique]
  order_id int [not null, ref: > shop_orders.id]
}

Table shop_users {
  id int [pk, not null]
  login varchar(40) [not null]
  country_id int
}

Table ledgerBook {
  id int [pk, not null]
  title varchar(40) [not null]
}

Table ledgerEntry {
  id int [pk, not null]
  amount decimal(10, 2) [not null]
}

Table audit {
  id int [pk, not null]
}

Table ab_cust {
  id int [pk, not null]
  ab_nb_owner int
}

Table cd_cust {
  id int [pk, not null]
  cd_nb_owner int
  order_ref int [ref: > shop_orders.id]
}

Table stock_levels {
  id int [pk, not null]
  sku varchar(20)
  warehouse varchar(20)
  shelf varchar(10)
  bin varchar(10)
  counted date
  checked_by varchar(40)
  batch varchar(20)
  quantity int
  ef_nb_owner int
  warehouse_zone_code varchar(8)
  updated date
}

Table zone {
  id int [pk, not null]
}

Ref: shop_users.country_id > geo.countries.id

another loose line
"""
SCHEMA_TABLES = 10
SCHEMA_GROUPS = ["ledger", "shop", "Other tables"]
MARKUP_NAME = "<img src=x onerror=alert(1)>"
MARKUP = 'Table "' + MARKUP_NAME + '" {\n  id int [pk, not null]\n}\n'
EMPTY = "// A file with comments only.\n/* No table here. */\n"
# One group of 14 tables, over the limit of 12 for a diagram.
MANY_TABLES = 14
DRAW_LIMIT = 12
MANY = "".join(f"Table box_{i:02d} {{\n  id int [pk, not null]\n}}\n" for i in range(1, MANY_TABLES + 1))

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def big_diagram():
    """An `erDiagram` with no relations, over Mermaid's default text limit."""
    lines = ["erDiagram"]
    for e in range(BIG_ENTITIES):
        lines.append(f"  table_number_{e:03d} {{")
        for c in range(BIG_COLUMNS):
            lines.append(f'    string column_number_{c:02d}_of_the_table "a comment that makes the file longer"')
        lines.append("  }")
    text = "\n".join(lines) + "\n"
    assert len(text) > MERMAID_DEFAULT_LIMIT, len(text)
    return text


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
    """A scratch registry and empty vendor stores: the operator's own daemon is never touched."""
    empty = tempfile.mkdtemp(prefix="wbrender_empty_")
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
    d = tempfile.mkdtemp(prefix="wbrender_repo_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    docs = p / "docs"
    docs.mkdir()
    (docs / "flow.mmd").write_text(FLOW, encoding="utf-8", newline="\n")
    (docs / "broken.mmd").write_text(BROKEN, encoding="utf-8", newline="\n")
    (docs / "big.mmd").write_text(big_diagram(), encoding="utf-8", newline="\n")
    (docs / "schema.dbml").write_text(SCHEMA, encoding="utf-8", newline="\n")
    (docs / "search.dbml").write_text(SCHEMA, encoding="utf-8", newline="\n")
    (docs / "markup.dbml").write_text(MARKUP, encoding="utf-8", newline="\n")
    (docs / "empty.dbml").write_text(EMPTY, encoding="utf-8", newline="\n")
    (docs / "many.dbml").write_text(MANY, encoding="utf-8", newline="\n")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbrender@example.com"],
        ["git", "config", "user.name", "wbrender"],
        ["git", "config", "core.autocrlf", "false"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run([EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8")
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are embedded in the binary, so it is rebuilt after any assets/ui edit.
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def open_path(page, slug, path):
    """Open a repo file the way a link does: the shell classifies its name."""
    page.evaluate(
        "([s, p]) => Alpine.store('tabs').openLink({ project: s, path: p, checkout: null })",
        [slug, path],
    )


def open_from_tree(page, folder, title):
    """Open a file the way a double-click does. The folder stays expanded, so
    the daemon's change nudge for it reaches the open tab."""
    page.wait_for_function(
        f"(f) => !!{FILES}._tree && !!{FILES}.rawTree().findFirst(n => n.title === f)", arg=folder, timeout=20000
    )
    page.evaluate(f"async (f) => {{ await {FILES}.rawTree().findFirst(n => n.title === f).setExpanded(true); }}", folder)
    page.wait_for_function(f"(t) => !!{FILES}.rawTree().findFirst(n => n.title === t)", arg=title, timeout=20000)
    page.evaluate(f"(t) => {FILES}.openFile({FILES}.rawTree().findFirst(n => n.title === t))", arg=title)


def pane(path):
    """The selector of the pane that shows `path` (its tab id ends with it)."""
    return f'.viewer[data-tab-id$=":{path}"]'


def svg_count(page, sel):
    return page.evaluate("(s) => document.querySelectorAll(s + ' .md-body svg').length", sel)


def svg_text(page, sel):
    return page.evaluate(
        "(s) => [...document.querySelectorAll(s + ' .md-body svg')].map(e => e.textContent).join(' ')", sel
    )


def wait_svg(page, sel, timeout=30000):
    page.wait_for_function("(s) => !!document.querySelector(s + ' .md-body .mermaid svg')", arg=sel, timeout=timeout)


def scenario_flow(page, fixture):
    sel = pane("docs/flow.mmd")
    wait_svg(page, sel)
    n, text = svg_count(page, sel), svg_text(page, sel)
    check("mmd: the file is drawn as one diagram", n == 1 and "Start" in text, f"svgs={n} text={text[:60]!r}")
    page.screenshot(path=os.path.join(SHOT_DIR, "rendered-files-flow.png"))

    page.wait_for_function("(s) => !!document.querySelector(s + ' .md-body .zoom-btn')", arg=sel, timeout=10000)
    buttons = page.evaluate("(s) => document.querySelectorAll(s + ' .zoom-btn').length", sel)
    check("mmd: the diagram has an Enlarge button", buttons == 1, f"buttons={buttons}")
    hidden = page.evaluate(
        """(s) => {
          const p = document.querySelector(s);
          const shown = (q) => getComputedStyle(p.querySelector(q)).display;
          return { outline: shown('nav.md-outline'), toc: shown('.md-toc-btn') };
        }""",
        sel,
    )
    check(
        "mmd: the pane shows no heading index",
        hidden == {"outline": "none", "toc": "none"},
        f"got={hidden}",
    )

    # Edit shows the source in Monaco; Preview draws it again.
    page.locator(f'{sel} [data-act="toggle"]').click()
    page.wait_for_function(
        "(s) => !!document.querySelector(s + ' .md-editor .monaco-editor')", arg=sel, timeout=30000
    )
    page.wait_for_function(
        "(src) => !!window.monaco && monaco.editor.getModels().some(m => m.getValue() === src)", arg=FLOW, timeout=15000
    )
    editor_shown = page.evaluate("(s) => getComputedStyle(document.querySelector(s + ' .md-editor')).display", sel)
    page.locator(f'{sel} [data-act="toggle"]').click()
    wait_svg(page, sel)
    n = svg_count(page, sel)
    check(
        "mmd: Edit shows the source and Preview draws it again",
        editor_shown == "block" and n == 1,
        f"editor={editor_shown} svgs={n}",
    )

    # A change on disk draws the diagram again.
    Path(fixture, "docs", "flow.mmd").write_text(CHANGED, encoding="utf-8", newline="\n")
    try:
        page.wait_for_function(
            "(s) => [...document.querySelectorAll(s + ' .md-body svg')].some(e => e.textContent.includes('Changed'))",
            arg=sel,
            timeout=10000,
        )
        changed = True
    except Exception:
        changed = False
    badge = page.evaluate("(s) => getComputedStyle(document.querySelector(s + ' .viewer-disk-badge')).display", sel)
    check(
        "mmd: a change on disk draws the diagram again",
        changed and svg_count(page, sel) == 1,
        f"svgs={svg_count(page, sel)} disk-badge={badge}",
    )


def scenario_broken(page, slug):
    open_path(page, slug, "docs/broken.mmd")
    sel = pane("docs/broken.mmd")
    page.wait_for_function("(s) => !!document.querySelector(s + ' .mermaid-error')", arg=sel, timeout=30000)
    text = page.evaluate("(s) => document.querySelector(s + ' .mermaid-error').textContent", sel)
    check("mmd: a broken diagram says so in the pane", text.startswith("Mermaid error:"), f"text={text[:80]!r}")


def scenario_big(page, slug):
    open_path(page, slug, "docs/big.mmd")
    sel = pane("docs/big.mmd")
    wait_svg(page, sel, timeout=60000)
    got = page.evaluate(
        """(s) => {
          const svg = document.querySelector(s + ' .md-body svg');
          const names = new Set([...svg.querySelectorAll('text, tspan')]
            .map(t => t.textContent.trim())
            .filter(t => /^table_number_\\d{3}$/.test(t)));
          return { entities: names.size, tooBig: svg.textContent.includes('Maximum text size') };
        }""",
        sel,
    )
    check(
        "mmd: a diagram over 50,000 characters is drawn",
        got["entities"] == BIG_ENTITIES and not got["tooBig"],
        f"got={got}",
    )


def scenario_detached(page, ctx, slug):
    open_path(page, slug, "docs/flow.mmd")
    sel = pane("docs/flow.mmd")
    wait_svg(page, sel)
    with ctx.expect_page() as popup:
        page.locator(f'{sel} [data-act="detach"]').click()
    detached = popup.value
    detached.on("pageerror", lambda e: print("detached pageerror:", e, flush=True))
    try:
        detached.wait_for_function("() => !!document.querySelector('.md-body .mermaid svg')", timeout=30000)
        n = detached.evaluate("() => document.querySelectorAll('.md-body svg').length")
    except Exception:
        n = 0
    detached.screenshot(path=os.path.join(SHOT_DIR, "rendered-files-detached.png"))
    check("mmd: the detached window draws the diagram", n == 1, f"svgs={n}")


def count(page, sel, inner):
    return page.evaluate("([s, q]) => document.querySelectorAll(s + ' ' + q).length", [sel, inner])


def texts(page, sel, inner):
    return page.evaluate(
        "([s, q]) => [...document.querySelectorAll(s + ' ' + q)].map(e => e.textContent.trim())", [sel, inner]
    )


def wait_cards(page, sel):
    page.wait_for_function("(s) => !!document.querySelector(s + ' .md-body .dbml-card')", arg=sel, timeout=30000)


def tags_of(page, sel, table, column):
    return page.evaluate(
        """([s, t, c]) => {
          const card = [...document.querySelectorAll(s + ' .dbml-card')].find(e => e.dataset.table === t);
          const row = card && [...card.querySelectorAll('tr.dbml-col')].find(e => e.dataset.col === c);
          return row ? [...row.querySelectorAll('.dbml-tag')].map(e => e.textContent) : null;
        }""",
        [sel, table, column],
    )


def scenario_dbml(page, slug):
    open_path(page, slug, "docs/schema.dbml")
    sel = pane("docs/schema.dbml")
    wait_cards(page, sel)
    summary = texts(page, sel, ".dbml-summary")
    check(
        "dbml: the summary counts the lines not understood",
        len(summary) == 1 and summary[0].endswith("2 lines were not understood."),
        f"summary={summary}",
    )
    cards = count(page, sel, ".dbml-card")
    check("dbml: the file shows one card per table", cards == SCHEMA_TABLES, f"cards={cards}")
    labels = texts(page, sel, ".md-body h2")
    check("dbml: tables are grouped by name prefix", labels == SCHEMA_GROUPS, f"labels={labels}")
    items = count(page, sel, ".md-outline .outline-item")
    check(
        "dbml: the side index lists groups and tables",
        items == len(SCHEMA_GROUPS) + SCHEMA_TABLES,
        f"items={items}",
    )
    long_name = "a_very_long_column_name_that_goes_on_for_a_long_while"
    tags = {
        "not null": tags_of(page, sel, "shop_orders", "total"),
        "nullable": tags_of(page, sel, "shop_orders", "buyer"),
        "unique": tags_of(page, sel, "shop_orders", long_name),
        "pk": tags_of(page, sel, "shop_orders", "id"),
    }
    check(
        "dbml: only the exceptions are tagged",
        tags == {"not null": [], "nullable": ["null"], "unique": ["null", "unique"], "pk": []},
        f"tags={tags}",
    )
    wide = page.evaluate(
        "(s) => [...document.querySelectorAll(s + ' .dbml-card')].filter(e => e.scrollWidth > e.clientWidth).length",
        sel,
    )
    check("dbml: no card overflows", wide == 0, f"overflowing={wide}")
    page.screenshot(path=os.path.join(SHOT_DIR, "rendered-files-dbml.png"))

    # Edit shows the source in Monaco; Preview shows the cards again.
    page.locator(f'{sel} [data-act="toggle"]').click()
    page.wait_for_function(
        "(s) => !!document.querySelector(s + ' .md-editor .monaco-editor')", arg=sel, timeout=30000
    )
    page.wait_for_function(
        "(src) => !!window.monaco && monaco.editor.getModels().some(m => m.getValue() === src)", arg=SCHEMA, timeout=15000
    )
    editor_shown = page.evaluate("(s) => getComputedStyle(document.querySelector(s + ' .md-editor')).display", sel)
    page.locator(f'{sel} [data-act="toggle"]').click()
    wait_cards(page, sel)
    again = count(page, sel, ".dbml-card")
    check(
        "dbml: Edit shows the source and Preview shows the cards again",
        editor_shown == "block" and again == SCHEMA_TABLES,
        f"editor={editor_shown} cards={again}",
    )


def scenario_dbml_edges(page, slug):
    open_path(page, slug, "docs/markup.dbml")
    sel = pane("docs/markup.dbml")
    wait_cards(page, sel)
    heads = texts(page, sel, ".dbml-card h3")
    images = count(page, sel, ".md-body img")
    check(
        "dbml: a table name with markup shows as text",
        heads == [MARKUP_NAME] and images == 0,
        f"h3={heads} img={images}",
    )

    open_path(page, slug, "docs/empty.dbml")
    sel = pane("docs/empty.dbml")
    page.wait_for_function("(s) => !!document.querySelector(s + ' .md-body .dbml-empty')", arg=sel, timeout=30000)
    empty = texts(page, sel, ".dbml-empty")
    check(
        "dbml: a file with no tables says so",
        empty == ["No tables found in this file."] and count(page, sel, ".dbml-card") == 0,
        f"empty={empty}",
    )


def shown_cards(page, sel):
    """The cards on screen: a card with `hidden`, or in a hidden group, takes no space."""
    return page.evaluate(
        "(s) => [...document.querySelectorAll(s + ' .dbml-card')].filter(e => e.offsetParent !== null).map(e => e.dataset.table)",
        sel,
    )


def shown_groups(page, sel):
    return page.evaluate(
        "(s) => [...document.querySelectorAll(s + ' .dbml-group')].filter(e => e.offsetParent !== null).map(e => e.querySelector('h2').textContent)",
        sel,
    )


def shown_rows(page, sel, table):
    """The column names a card shows."""
    return page.evaluate(
        """([s, t]) => {
          const card = [...document.querySelectorAll(s + ' .dbml-card')].find(e => e.dataset.table === t);
          return [...card.querySelectorAll('tr.dbml-col')].filter(e => e.offsetParent !== null).map(e => e.dataset.col);
        }""",
        [sel, table],
    )


def hidden_rows(page, sel):
    return page.evaluate("(s) => document.querySelectorAll(s + ' tr.dbml-col[hidden]').length", sel)


def more_button(page, sel, table):
    return page.evaluate(
        """([s, t]) => {
          const card = [...document.querySelectorAll(s + ' .dbml-card')].find(e => e.dataset.table === t);
          const b = card.querySelector('button.dbml-more');
          return b ? b.textContent : null;
        }""",
        [sel, table],
    )


def marked(page, sel, cls):
    """`table.column` of every row that carries the class `cls`."""
    return page.evaluate(
        """([s, c]) => [...document.querySelectorAll(s + ' tr.dbml-col.' + c)]
            .map(e => e.closest('.dbml-card').dataset.table + '.' + e.dataset.col).sort()""",
        [sel, cls],
    )


def wait_shown(page, sel, n):
    """Wait until `n` cards are on screen. A miss is not an error: the check that follows reports it."""
    try:
        page.wait_for_function(
            "([s, n]) => [...document.querySelectorAll(s + ' .dbml-card')].filter(e => e.offsetParent !== null).length === n",
            arg=[sel, n],
            timeout=5000,
        )
    except Exception:
        pass


def open_find(page, sel):
    page.locator(f'{sel} [data-act="find"]').click()
    return page.locator(f"{sel} .md-find-input")


def scenario_dbml_find(page, slug):
    sel = pane("docs/schema.dbml")
    box = open_find(page, sel)
    box.fill("buyer")
    wait_shown(page, sel, 1)
    cards, groups = shown_cards(page, sel), shown_groups(page, sel)
    check(
        "dbml find: a term hides the cards that do not contain it",
        cards == ["shop_orders"] and groups == ["shop"],
        f"cards={cards} groups={groups}",
    )
    box.press("Escape")
    wait_shown(page, sel, SCHEMA_TABLES)
    after, groups = len(shown_cards(page, sel)), shown_groups(page, sel)
    check(
        "dbml find: closing the search shows every card again",
        after == SCHEMA_TABLES and groups == SCHEMA_GROUPS,
        f"cards={after} groups={groups}",
    )

    # Column 11 of `stock_levels` is hidden while the card is closed.
    box = open_find(page, sel)
    box.fill("warehouse_zone")
    wait_shown(page, sel, 1)
    rows = shown_rows(page, sel, "stock_levels")
    hit_in = page.evaluate(
        "(s) => [...document.querySelectorAll(s + ' mark.find-hit')].map(e => e.closest('tr.dbml-col')?.dataset.col)", sel
    )
    check(
        "dbml find: a match in a hidden column opens its card",
        len(rows) == 12 and "warehouse_zone_code" in rows and hit_in == ["warehouse_zone_code"],
        f"rows={len(rows)} hits={hit_in}",
    )
    page.screenshot(path=os.path.join(SHOT_DIR, "rendered-files-dbml-find.png"))
    box.press("Escape")
    wait_shown(page, sel, SCHEMA_TABLES)


def scenario_dbml_cards(page, slug):
    sel = pane("docs/schema.dbml")
    closed = len(shown_rows(page, sel, "stock_levels"))
    label = more_button(page, sel, "stock_levels")
    short = more_button(page, sel, "shop_orders")
    page.locator(f'{sel} .dbml-card[data-table="stock_levels"] button.dbml-more').click()
    opened = len(shown_rows(page, sel, "stock_levels"))
    label_open = more_button(page, sel, "stock_levels")
    check(
        "dbml cards: a long table shows 8 columns until it is opened",
        closed == 8 and label == "Show 4 more columns" and short is None and opened == 12 and label_open == "Show fewer columns",
        f"closed={closed} label={label!r} short={short!r} opened={opened} label_open={label_open!r}",
    )
    page.locator(f'{sel} .dbml-card[data-table="stock_levels"] button.dbml-more').click()
    page.get_by_role("button", name="Expand all").click()
    all_open = hidden_rows(page, sel)
    page.get_by_role("button", name="Collapse all").click()
    back = len(shown_rows(page, sel, "stock_levels"))
    check(
        "dbml cards: Expand all opens every card",
        all_open == 0 and back == 8,
        f"hidden-after-expand={all_open} rows-after-collapse={back}",
    )

    row = f'{sel} .dbml-card[data-table="ab_cust"] tr.dbml-col[data-col="ab_nb_owner"]'
    page.locator(f"{row} .dbml-col-name").click()
    origin, same = marked(page, sel, "dbml-origin"), marked(page, sel, "dbml-same")
    rows = len(shown_rows(page, sel, "stock_levels"))
    page.locator(f"{row} .dbml-col-name").click()
    cleared = marked(page, sel, "dbml-origin") + marked(page, sel, "dbml-same")
    check(
        "dbml columns: a click marks the columns with the same name",
        origin == ["ab_cust.ab_nb_owner"]
        and same == ["cd_cust.cd_nb_owner", "stock_levels.ef_nb_owner"]
        and rows == 12
        and cleared == [],
        f"origin={origin} same={same} stock-rows={rows} after-second-click={cleared}",
    )
    page.get_by_role("button", name="Collapse all").click()


def scenario_dbml_refs(page, slug):
    sel = pane("docs/schema.dbml")
    # A short window, so the target card is off screen while the link is clicked.
    page.set_viewport_size({"width": 1500, "height": 450})
    page.evaluate("(s) => { const m = document.querySelector(s + ' .md-scroll'); m.scrollTop = m.scrollHeight; }", sel)
    url = page.url
    link = f'{sel} .dbml-card[data-table="cd_cust"] tr.dbml-col[data-col="order_ref"] a.dbml-ref'
    text = page.locator(link).text_content()
    probe = """(s) => {
      const box = document.querySelector(s + ' .md-scroll').getBoundingClientRect();
      const top = document.querySelector(s + ' .dbml-card[data-table="shop_orders"] h3').getBoundingClientRect().top;
      return top >= box.top - 1 && top < box.bottom;
    }"""
    before = page.evaluate(probe, sel)
    page.locator(link).dispatch_event("click")
    try:
        page.wait_for_function(probe, arg=sel, timeout=10000)
        moved = True
    except Exception:
        moved = False
    check(
        "dbml refs: a link moves the view to the target table",
        text == "→ shop_orders.id" and not before and moved and page.url == url,
        f"text={text!r} visible-before={before} moved={moved} url-same={page.url == url}",
    )
    page.set_viewport_size({"width": 1500, "height": 950})

    incoming = page.evaluate(
        """(s) => {
          const card = [...document.querySelectorAll(s + ' .dbml-card')].find(e => e.dataset.table === 'shop_orders');
          const p = card.querySelector('p.dbml-in');
          return p ? { text: p.textContent, links: [...p.querySelectorAll('a')].map(a => a.textContent) } : null;
        }""",
        sel,
    )
    check(
        "dbml refs: a table lists the columns that point to it",
        incoming is not None
        and incoming["text"].startswith("Referenced by:")
        and sorted(incoming["links"]) == ["cd_cust.order_ref", "shop_items.order_id"],
        f"incoming={incoming}",
    )

    missing = page.evaluate(
        """(s) => [...document.querySelectorAll(s + ' .dbml-ref')]
            .filter(e => e.textContent.includes('geo.countries'))
            .map(e => e.tagName + ':' + e.textContent)""",
        sel,
    )
    check(
        "dbml refs: a ref to a missing table is plain text",
        missing == ["SPAN:→ geo.countries.id"],
        f"missing={missing}",
    )


def scenario_dbml_project_search(page, slug):
    page.evaluate(
        f"([s]) => {TABS}.openTab({{ project: s, path: 'docs/search.dbml', title: 'search.dbml', ftype: 'dbml', find: 'buyer' }})",
        [slug],
    )
    sel = pane("docs/search.dbml")
    wait_cards(page, sel)
    wait_shown(page, sel, 1)
    got = page.evaluate(
        """(s) => ({
          open: document.querySelector(s + ' .md-find').classList.contains('open'),
          term: document.querySelector(s + ' .md-find-input').value,
        })""",
        sel,
    )
    cards = shown_cards(page, sel)
    check(
        "dbml find: a project search opens the grid filtered",
        got == {"open": True, "term": "buyer"} and cards == ["shop_orders"],
        f"find={got} cards={cards}",
    )


def pick_boxes(page, sel, tables):
    for table in tables:
        page.locator(f'{sel} .dbml-card[data-table="{table}"] input.dbml-pick').check()


def draw_button(page, sel):
    """The text and state of the draw button."""
    return page.evaluate(
        "(s) => { const b = document.querySelector(s + ' button.dbml-draw'); return [b.textContent, b.disabled]; }", sel
    )


def wait_overlay(page, timeout=30000):
    """Wait for the zoom view with a diagram in it. A miss is not an error: the check that follows reports it."""
    try:
        page.wait_for_function("() => !!document.querySelector('.zoom-overlay svg')", timeout=timeout)
    except Exception:
        pass


def overlay_parts(page):
    return page.evaluate(
        """() => {
          const o = document.querySelector('.zoom-overlay');
          if (!o) return null;
          const svg = o.querySelector('svg');
          const names = new Set([...svg.querySelectorAll('text, tspan')].map(t => t.textContent.trim()));
          return {
            entities: svg.querySelectorAll('g.node').length,
            lines: svg.querySelectorAll('path.relationshipLine').length,
            names: [...names],
            images: o.querySelectorAll('img').length,
          };
        }"""
    )


def checked_count(page, sel):
    return page.evaluate("(s) => document.querySelectorAll(s + ' input.dbml-pick:checked').length", sel)


def clear_selection(page, sel):
    page.locator(f"{sel} button.dbml-btn", has_text="Clear selection").click()


def scenario_dbml_draw(page, slug):
    sel = pane("docs/schema.dbml")
    # `shop_items.order_id` points to `shop_orders.id`: one relation between two of the three.
    pick_boxes(page, sel, ["shop_orders", "shop_items", "shop_users"])
    label = draw_button(page, sel)
    page.locator(f"{sel} button.dbml-draw").click()
    wait_overlay(page)
    parts = overlay_parts(page)
    page.screenshot(path=os.path.join(SHOT_DIR, "rendered-files-dbml-draw.png"))
    check(
        "dbml draw: the selected tables open as a diagram",
        label == ["Draw 3 tables", False]
        and parts is not None
        and parts["entities"] == 3
        and parts["lines"] == 1
        and {"shop_orders", "shop_items", "shop_users"} <= set(parts["names"]),
        f"button={label} overlay={parts}",
    )

    page.keyboard.press("Escape")
    closed = page.evaluate("() => !document.querySelector('.zoom-overlay')")
    focus = page.evaluate("() => document.activeElement && document.activeElement.className")
    again = draw_button(page, sel)
    check(
        "dbml draw: Escape closes the diagram",
        closed and "dbml-draw" in (focus or "") and again == ["Draw 3 tables", False],
        f"closed={closed} focus={focus!r} button={again}",
    )
    clear_selection(page, sel)
    cleared = draw_button(page, sel)
    boxes = checked_count(page, sel)
    check(
        "dbml draw: Clear selection unchecks every table",
        cleared == ["Draw 0 tables", True] and boxes == 0,
        f"button={cleared} checked={boxes}",
    )

    # A group of 3 is selected at once, and Find keeps the selection.
    page.locator(f'{sel} .dbml-group:has(h2:text-is("shop")) button', has_text="Select group").click()
    group = draw_button(page, sel)
    checked = checked_count(page, sel)
    box = open_find(page, sel)
    box.fill("buyer")
    wait_shown(page, sel, 1)
    kept = draw_button(page, sel)
    box.press("Escape")
    wait_shown(page, sel, SCHEMA_TABLES)
    check(
        "dbml draw: a group can be selected at once",
        group == ["Draw 3 tables", False] and checked == 3 and kept == ["Draw 3 tables", False],
        f"button={group} checked={checked} after-find={kept}",
    )
    clear_selection(page, sel)


def scenario_dbml_limit(page, slug):
    open_path(page, slug, "docs/many.dbml")
    sel = pane("docs/many.dbml")
    wait_cards(page, sel)
    start = draw_button(page, sel)
    pick_boxes(page, sel, [f"box_{i:02d}" for i in range(1, MANY_TABLES + 1)])
    fourteen = draw_button(page, sel)
    page.locator(f'{sel} .dbml-card[data-table="box_14"] input.dbml-pick').uncheck()
    thirteen = draw_button(page, sel)
    page.locator(f'{sel} .dbml-card[data-table="box_13"] input.dbml-pick').uncheck()
    twelve = draw_button(page, sel)
    over = ["Select 12 tables or fewer", True]
    check(
        "dbml draw: the button keeps the limit",
        start == ["Draw 0 tables", True]
        and fourteen == over
        and thirteen == over
        and twelve == [f"Draw {DRAW_LIMIT} tables", False],
        f"start={start} fourteen={fourteen} thirteen={thirteen} twelve={twelve}",
    )
    # The most the button allows is drawn.
    page.locator(f"{sel} button.dbml-draw").click()
    wait_overlay(page)
    parts = overlay_parts(page)
    check(
        "dbml draw: 12 tables are drawn",
        parts is not None and parts["entities"] == DRAW_LIMIT and parts["lines"] == 0,
        f"overlay={parts}",
    )
    page.keyboard.press("Escape")


def scenario_dbml_draw_markup(page, slug):
    dialogs = []
    page.on("dialog", lambda d: (dialogs.append(d.message), d.dismiss()))
    open_path(page, slug, "docs/markup.dbml")
    sel = pane("docs/markup.dbml")
    wait_cards(page, sel)
    pick_boxes(page, sel, [MARKUP_NAME])
    page.locator(f"{sel} button.dbml-draw").click()
    wait_overlay(page)
    parts = overlay_parts(page)
    time.sleep(0.5)
    check(
        "dbml draw: a table name with markup is drawn as text",
        parts is not None and MARKUP_NAME in parts["names"] and parts["images"] == 0 and dialogs == [],
        f"overlay={parts} dialogs={dialogs}",
    )
    page.keyboard.press("Escape")


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbrender_reg_")
    fixture = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1500, "height": 950})
            page = ctx.new_page()
            page.on("pageerror", lambda e: print("pageerror:", e, flush=True))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function("() => Alpine.store('projects').projects.length === 1", timeout=15000)
            page.evaluate(
                "(s) => { if (Alpine.store('projects').openSlug !== s)"
                " Alpine.$data(document.querySelector('[x-data]')).toggle(s); }",
                arg=slug,
            )
            open_from_tree(page, "docs", "flow.mmd")
            scenario_flow(page, fixture)
            scenario_broken(page, slug)
            scenario_big(page, slug)
            page.screenshot(path=os.path.join(SHOT_DIR, "rendered-files-big.png"))
            scenario_detached(page, ctx, slug)
            scenario_dbml(page, slug)
            scenario_dbml_find(page, slug)
            scenario_dbml_cards(page, slug)
            scenario_dbml_refs(page, slug)
            scenario_dbml_draw(page, slug)
            scenario_dbml_project_search(page, slug)
            scenario_dbml_edges(page, slug)
            scenario_dbml_limit(page, slug)
            scenario_dbml_draw_markup(page, slug)
            browser.close()
    finally:
        stop(proc)

    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
