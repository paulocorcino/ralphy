"""Browser acceptance: the file viewers and the issue drawer on a PHONE-width pane.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR` (own
port, own registry — the operator's own desk and login policy are untouched).

The pure rule (`pathLabel`) is tabled in `ui-tests/wb-viewer.test.mjs` and the
container declarations are pinned in `lib.rs`; this script proves what neither
can: that the `@container` rules FIRE at 390px and stay silent at 1280px, that
the controls a phone clipped are reachable, and that the overlay index and the
full-width drawer behave.

Scenario 1  the daemon is listening
Scenario 2  PHONE, markdown tab: the toolbar is one row; the path reads
            `docs/` + `COMPARATIVO.md` (no repo, no environment); every
            button is icon-only and inside the pane; Save is reachable
Scenario 3  PHONE, markdown tab: the outline is hidden until `Contents`;
            open → it overlays the article; a jump closes it; the article
            spans the pane
Scenario 4  PHONE, markdown tab: entering Edit hides the index even when it
            was open; leaving Edit restores the article
Scenario 5  PHONE, code tab: Save and Detach lie inside the pane; Monaco's
            gutter is the narrow shape (no folding margin)
Scenario 6  PHONE, board: a fake issue's drawer spans the pane, the close
            button is FIRST in its row and wears the back arrow, and it lies
            inside the pane; GitHub is a glyph
Scenario 6b PHONE, board: the head's close button lies inside the pane and
            the scope label is folded (the sidebar names the project)
Scenario 6c PHONE, a maximized console: the titlebar is ONE row, its close
            lies inside the window, the title's tail ellipsizes and the
            tooltip carries the whole title
Scenario 6d TWO PAGES on one desk: page B loaded before page A opened a
            console; B's next flush keeps A's record and its sessionId (the
            read-before-write fold); a third page attaches to A's record —
            one window, not an adopted second one
Scenario 6e the daemon's own fold: a raw PUT that never read A's record but
            carries `removed` keeps it; a raw PUT naming it in `removed`
            drops it; a pre-amendment body (no `removed`) still replaces
Scenario 7  DESKTOP, the same tabs: the outline is the 190px column, captions
            are visible, the drawer is not full width, and the label is
            STILL the path only (the policy is the same on every width)
Scenario 8  no page errors

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/narrow-panes-2026-09-20.png.
Run: python crates/ralphy-daemon/tests/wb_narrow_panes.py   (exit 0 = all pass)
"""

import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7453
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
PHONE = {"width": 390, "height": 844}
DESKTOP = {"width": 1280, "height": 800}

results = []

MD = """# Comparativo vivo — VibForge × Heroku × Kubero × Coolify

Natureza: documento vivo.

## 0. Postura

Texto.

## 1. Fontes

Texto.

### 1.1 Superfície

Texto.

## 2. A tabela

""" + "\n\n".join(f"Parágrafo {i} de enchimento para dar altura ao documento." for i in range(40)) + """

## 6. Registo de mudanças

Fim.
"""

ENV = "\n".join(f"export VAR_{i}=\"value {i}\"  # a long trailing comment to push the line past the pane" for i in range(60)) + "\n"


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


def port_already_listening(port, host="127.0.0.1"):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(0.5)
    try:
        return s.connect_ex((host, port)) == 0
    finally:
        s.close()


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def empty_env(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wbnarrow_empty_")
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
    d = tempfile.mkdtemp(prefix="wbnarrow_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n", encoding="utf-8")
    (Path(d) / "docs").mkdir()
    (Path(d) / "docs" / "COMPARATIVO.md").write_text(MD, encoding="utf-8")
    (Path(d) / "infra").mkdir()
    (Path(d) / "infra" / "bootstrap.env.example").write_text(ENV, encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbnarrow@example.com"],
        ["git", "config", "user.name", "wbnarrow"],
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
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # the previous build's viewers.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def rect(page, selector):
    return page.evaluate(
        "(s) => { const e = document.querySelector(s); if (!e) return null;"
        " const r = e.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height, right: r.right }; }",
        selector,
    )


def display(page, selector):
    """Computed `display`. A flex ITEM is blockified — `inline-flex` reads
    `flex`, `inline` reads `block` — so callers compare against `none` only."""
    return page.evaluate(
        "(s) => { const e = document.querySelector(s); return e ? getComputedStyle(e).display : null; }", selector
    )


def open_tab(page, slug, path):
    title = path.rsplit("/", 1)[-1]
    ftype = "markdown" if title.endswith(".md") else "code"
    page.evaluate(
        f"() => {SH}.openTab({{ project: '{slug}', path: '{path}', title: '{title}', ftype: '{ftype}' }})"
    )
    page.wait_for_selector(f".viewer:not([style*='display: none']) .viewer-file", timeout=8000)
    page.wait_for_timeout(400)


def active_viewer(page):
    return ".viewer:not([style*='display: none'])"


def inject_issue(page, slug):
    # The board's own `board.list` fold answers AFTER toggleKanban and would
    # overwrite an earlier injection — wait until it has settled (the refresh
    # button re-enables), then plant the row.
    page.wait_for_function("() => !document.querySelector('.kanban-refresh')?.disabled", timeout=8000)
    page.wait_for_timeout(200)
    page.evaluate(
        f"""() => {{
          const sh = {SH};
          sh.boardIssues['{slug}'] = [{{
            number: 2, title: 'Gate humano: credenciais e recursos externos para destravar a Fase 0 (A2 + A3a)',
            state: 'closed', reason: 'not_planned', column: 'closed', labels: [{{ name: 'ready-for-human', color: '0e8a16' }}],
            assignees: [], created_at: '2026-06-16T00:00:00Z', updated_at: '2026-09-02T00:00:00Z',
            body: 'O código de infra do VibeForge está **pronto e validado**.', comments: [], blocked_by: [],
          }}];
        }}"""
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    if port_already_listening(PORT):
        check(f"port {PORT} is free", False, "something is already listening — is a previous run still up?")
        sys.exit(1)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbnarrow_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            errors = []

            # ---------------------------------------------------- PHONE
            ctx = browser.new_context(viewport=PHONE, has_touch=True, is_mobile=True)
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {SH}.toggle('{slug}')")
            page.wait_for_timeout(600)
            # The phone in the report had the sidebar folded (the rail's Projects
            # button, tapped once more); with it open a 390px viewport leaves the
            # workspace 42px, which is not the pane anyone reads in.
            page.evaluate(f"() => {{ {SH}.sideOpen = false; }}")
            page.wait_for_timeout(300)

            # --- scenario 2: markdown toolbar --------------------------------
            open_tab(page, slug, "docs/COMPARATIVO.md")
            v = active_viewer(page)
            pane = rect(page, v)
            check("phone pane is narrower than the 560px threshold", pane and pane["w"] <= 560, f"w={pane and pane['w']}")
            d = page.locator(f"{v} .viewer-dir").inner_text()
            f = page.locator(f"{v} .viewer-file").inner_text()
            check("the label is the path alone: dir + file", (d, f) == ("docs/", "COMPARATIVO.md"), f"{d!r} {f!r}")
            title = page.get_attribute(f"{v} .viewer-path", "title")
            check("the full `repo / path` form rides the title", title == f"{slug} / docs/COMPARATIVO.md", repr(title))
            check("captions are folded", display(page, f"{v} [data-act='save'] .vbtn-label") == "none")
            tb = rect(page, f"{v} .viewer-toolbar")
            save = rect(page, f"{v} [data-act='save']")
            det = rect(page, f"{v} [data-act='detach']")
            check("the toolbar is ONE row", tb and save and abs(save["y"] - tb["y"]) < tb["h"] and tb["h"] < 60, f"tb={tb} save={save}")
            check("Save lies inside the pane", save and save["right"] <= pane["right"] + 0.5, f"save.right={save and save['right']} pane.right={pane['right']}")
            check("Detach lies inside the pane", det and det["right"] <= pane["right"] + 0.5, f"det.right={det and det['right']}")
            check("the Contents button is shown", display(page, f"{v} .md-toc-btn") != "none", display(page, f"{v} .md-toc-btn"))

            # --- scenario 3: the outline overlay -----------------------------
            check("the outline starts hidden", display(page, f"{v} .md-outline") == "none")
            art = rect(page, f"{v} .md-scroll")
            check("the article spans the pane", art and art["w"] >= pane["w"] - 2, f"art.w={art and art['w']} pane.w={pane['w']}")
            page.click(f"{v} [data-act='outline']")
            page.wait_for_timeout(150)
            check("Contents opens the outline", display(page, f"{v} .md-outline") == "block")
            nav = rect(page, f"{v} .md-outline")
            check("the open outline overlays the article's top edge", nav and abs(nav["y"] - art["y"]) < 2 and nav["w"] >= pane["w"] - 2, f"nav={nav} art={art}")
            page.screenshot(path=os.path.join(SHOT_DIR, "narrow-panes-2026-09-20.png"))
            page.locator(f"{v} .outline-item").last.click()
            page.wait_for_timeout(200)
            check("a jump closes the outline", display(page, f"{v} .md-outline") == "none")
            top = page.evaluate(f"() => document.querySelector(\"{v} .md-scroll\").scrollTop")
            check("…and lands on the heading", top > 100, f"scrollTop={top}")

            # --- scenario 4: edit mode wins over the index -------------------
            page.click(f"{v} [data-act='outline']")
            page.wait_for_timeout(100)
            page.click(f"{v} [data-act='toggle']")
            page.wait_for_timeout(300)
            check("Edit hides the index that was open", display(page, f"{v} .md-outline") == "none")
            check("Edit clears toc-open", page.evaluate(f"() => document.querySelector(\"{v} .md-split\").classList.contains('toc-open')") is False)
            page.click(f"{v} [data-act='toggle']")
            page.wait_for_timeout(300)
            check("Preview restores the article", display(page, f"{v} .md-scroll") == "block")

            # --- scenario 5: code tab ----------------------------------------
            open_tab(page, slug, "infra/bootstrap.env.example")
            v = active_viewer(page)
            page.wait_for_selector(f"{v} .monaco-editor", timeout=15000)
            page.wait_for_timeout(600)
            d = page.locator(f"{v} .viewer-dir").inner_text()
            f = page.locator(f"{v} .viewer-file").inner_text()
            check("code tab label is the path alone", (d, f) == ("infra/", "bootstrap.env.example"), f"{d!r} {f!r}")
            pane = rect(page, v)
            save = rect(page, f"{v} [data-act='save']")
            det = rect(page, f"{v} [data-act='detach']")
            check("code: Save lies inside the pane", save and save["right"] <= pane["right"] + 0.5, f"{save}")
            check("code: Detach lies inside the pane", det and det["right"] <= pane["right"] + 0.5, f"{det}")
            opts = page.evaluate(
                f"() => {{ const ed = window.WBViewer && Array.from(document.querySelectorAll('.monaco-editor'));"
                f" const w = document.querySelector(\"{v} .margin\"); return w ? w.getBoundingClientRect().width : null; }}"
            )
            check("code: the gutter is the narrow shape (< 40px)", opts is not None and opts < 40, f"margin.w={opts}")

            # --- scenario 6: the board drawer --------------------------------
            page.evaluate(f"() => {{ if (!{SH}.kanbanOpen) {SH}.toggleKanban(); }}")
            page.wait_for_timeout(300)
            inject_issue(page, slug)
            page.evaluate(f"() => {SH}.openIssue(2)")
            page.wait_for_selector(".kanban-detail.open .kd-head", timeout=5000)
            page.wait_for_timeout(300)
            board = rect(page, ".kanban")
            drawer = rect(page, ".kanban-detail.open")
            check("drawer spans the board", drawer and board and abs(drawer["w"] - board["w"]) < 1, f"drawer={drawer} board={board}")
            x = rect(page, ".kanban-detail.open .kd-x")
            state = rect(page, ".kanban-detail.open .kd-state")
            check("close is FIRST in its row", x and state and x["x"] < state["x"], f"x={x} state={state}")
            check("close lies inside the pane", x and x["right"] <= board["right"] + 0.5 and x["x"] >= board["x"] - 0.5)
            check("close wears the back arrow", display(page, ".kanban-detail.open .kd-x .bi-arrow-left") != "none" and display(page, ".kanban-detail.open .kd-x .bi-x-lg") == "none", f"{display(page, '.kanban-detail.open .kd-x .bi-arrow-left')} {display(page, '.kanban-detail.open .kd-x .bi-x-lg')}")
            check("GitHub caption folded", display(page, ".kanban-detail.open .kd-gh-label") in ("none", None))
            page.click(".kanban-detail.open .kd-x")
            page.wait_for_timeout(300)
            check("the back arrow closes the drawer", page.evaluate(f"() => {SH}.kanbanSel") is None)

            # --- scenario 6b: the board head ---------------------------------
            close = rect(page, ".kanban-close")
            check("board head: close lies inside the pane", close and board and close["right"] <= board["right"] + 0.5, f"close={close} board={board}")
            check("board head: the scope label is folded", display(page, ".kanban-scope") == "none", display(page, ".kanban-scope"))
            check("board head: the search is reachable", (rect(page, ".kanban-search input") or {}).get("w", 0) > 80, rect(page, ".kanban-search input"))
            page.evaluate(f"() => {SH}.toggleKanban()")
            page.wait_for_timeout(200)

            # --- scenario 6c: a maximized console's titlebar -----------------
            page.evaluate(f"() => {SH}.activate('consoles')")
            page.wait_for_timeout(300)
            before = page.locator(".session-window").count()
            page.evaluate(f"() => window.WBConsole.open({{ repo: '{slug}', plain: true }})")
            page.wait_for_function(f"() => document.querySelectorAll('.session-window').length === {before + 1}", timeout=8000)
            page.wait_for_timeout(600)
            page.evaluate("() => document.querySelector('.session-window:last-of-type .session-max').click()")
            page.wait_for_timeout(300)
            win = rect(page, ".session-window.maximized")
            bar = rect(page, ".session-window.maximized .session-titlebar")
            xbtn = rect(page, ".session-window.maximized .session-close")
            check("console: the titlebar is one row (< 40px)", bar and bar["h"] < 40, f"bar={bar}")
            check("console: close lies inside the window", xbtn and win and xbtn["right"] <= win["right"] + 0.5, f"x={xbtn} win={win}")
            tail = page.evaluate(
                "() => { const e = document.querySelector('.session-window.maximized .session-title-rest');"
                " return e ? { cut: e.scrollWidth > e.clientWidth + 1, ellipsis: getComputedStyle(e).textOverflow, tip: e.parentElement.title } : null; }"
            )
            check("console: the title's tail ellipsizes", tail and tail["ellipsis"] == "ellipsis" and tail["cut"], f"{tail}")
            check("console: the tooltip carries the whole title", tail and slug in tail["tip"], f"{tail}")
            page.screenshot(path=os.path.join(SHOT_DIR, "narrow-console-2026-09-20.png"))
            # Closed for real (session ended, record forgotten): scenario 6d
            # counts windows, and a live console left here would be restored
            # into every page it opens.
            page.click(".session-window.maximized .session-close")
            page.wait_for_selector(".wb-confirm", timeout=4000)
            page.locator(".wb-confirm .btn.danger, .wb-confirm .btn.accent").first.click()
            page.wait_for_function("() => document.querySelectorAll('.session-window').length === 0", timeout=8000)
            page.wait_for_timeout(600)
            ctx.close()

            # --- scenario 6d: two pages on one desk ---------------------------
            ctx_b = browser.new_context(viewport=DESKTOP)
            page_b = ctx_b.new_page()
            page_b.on("pageerror", lambda e: errors.append(str(e)))
            page_b.goto(BASE)
            page_b.wait_for_selector("[x-data]", timeout=8000)
            page_b.evaluate("() => window.WBConsole.whenDeskLoaded()")
            page_b.wait_for_timeout(800)
            known_b = page_b.request.get(BASE + "api/desk").json()["windows"]
            known_b = [r["id"] for r in known_b]

            ctx_a = browser.new_context(viewport=DESKTOP)
            page_a = ctx_a.new_page()
            page_a.on("pageerror", lambda e: errors.append(str(e)))
            page_a.goto(BASE)
            page_a.wait_for_selector("[x-data]", timeout=8000)
            page_a.evaluate(f"() => {SH}.toggle('{slug}')")
            page_a.wait_for_timeout(800)
            before = page_a.locator(".session-window").count()
            page_a.evaluate(f"() => window.WBConsole.open({{ repo: '{slug}', plain: true }})")
            page_a.wait_for_function(f"() => document.querySelectorAll('.session-window').length === {before + 1}", timeout=8000)
            page_a.wait_for_function(
                "() => [...document.querySelectorAll('.session-window')].some((w) => w._term && w._term.sessionId != null)", timeout=15000
            )
            page_a.wait_for_timeout(800)
            a_rec = page_a.evaluate(
                "() => { const w = [...document.querySelectorAll('.session-window')].find((w) => w._term && w._term.sessionId != null);"
                " return { id: w._deskId, sessionId: w._term.sessionId }; }"
            )
            stored = page_a.request.get(BASE + "api/desk").json()["windows"]
            check("A's record reached the daemon with its sessionId", any(r["id"] == a_rec["id"] and r.get("sessionId") == a_rec["sessionId"] for r in stored), f"a={a_rec} stored={[(r['id'], r.get('sessionId')) for r in stored]}")
            check("B's mirror predates A's console", a_rec["id"] not in known_b)
            # B mutates the desk — a selection is the cheapest act that flushes —
            # WITHOUT having re-read it since A's console opened.
            page_b.evaluate(f"() => window.WBConsole.setCheckout('{slug}', null)")
            page_b.wait_for_timeout(1200)
            stored = page_b.request.get(BASE + "api/desk").json()["windows"]
            check("B's flush keeps A's record and its sessionId", any(r["id"] == a_rec["id"] and r.get("sessionId") == a_rec["sessionId"] for r in stored), f"a={a_rec} stored={[(r['id'], r.get('sessionId')) for r in stored]}")

            ctx_c = browser.new_context(viewport=DESKTOP)
            page_c = ctx_c.new_page()
            page_c.on("pageerror", lambda e: errors.append(str(e)))
            page_c.goto(BASE)
            page_c.wait_for_selector("[x-data]", timeout=8000)
            page_c.wait_for_function("() => document.querySelectorAll('.session-window').length >= 1", timeout=8000)
            page_c.wait_for_timeout(1000)
            c_wins = page_c.evaluate("() => [...document.querySelectorAll('.session-window')].map((w) => w._deskId)")
            check("a third page attaches to A's record — one window, not two", c_wins == [a_rec["id"]], f"c={c_wins} a={a_rec['id']}")

            # --- scenario 6e: the daemon folds, whatever the page read ---------
            def put_desk(body):
                return page_c.request.put(BASE + "api/desk", data=body, headers={"Content-Type": "application/json"})

            rect_json = {"left": 1, "top": 1, "width": 300, "height": 200}
            stranger = {"id": "w-raw-stranger", "repo": slug, "agent": "console", "kind": "console", "rect": rect_json, "max": False, "sessionId": None, "ts": 1}
            r = put_desk({"windows": [stranger], "fences": [], "removed": {"windows": [], "fences": [], "checkouts": []}})
            ids = [w["id"] for w in r.json()["windows"]]
            check("a PUT that never read A's record keeps it (the daemon folds)", r.status == 200 and a_rec["id"] in ids and "w-raw-stranger" in ids, f"{r.status} {ids}")
            r = put_desk({"windows": [], "fences": [], "removed": {"windows": ["w-raw-stranger"], "fences": [], "checkouts": []}})
            ids = [w["id"] for w in r.json()["windows"]]
            check("a PUT naming a record in `removed` drops it and only it", "w-raw-stranger" not in ids and a_rec["id"] in ids, f"{ids}")
            r = put_desk({"windows": [stranger], "fences": []})
            ids = [w["id"] for w in r.json()["windows"]]
            check("a pre-amendment body (no `removed`) still replaces wholesale", ids == ["w-raw-stranger"], f"{ids}")
            # Put A's record back the way the shell would, so the pages closing
            # below have nothing surprising to reconcile.
            put_desk({"windows": [], "fences": [], "removed": {"windows": ["w-raw-stranger"], "fences": [], "checkouts": []}})
            ctx_c.close()
            ctx_a.close()
            ctx_b.close()

            # ---------------------------------------------------- DESKTOP
            ctx = browser.new_context(viewport=DESKTOP)
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {SH}.toggle('{slug}')")
            page.wait_for_timeout(600)
            open_tab(page, slug, "docs/COMPARATIVO.md")
            v = active_viewer(page)
            pane = rect(page, v)
            check("desktop pane is wider than the threshold", pane and pane["w"] > 560, f"w={pane and pane['w']}")
            d = page.locator(f"{v} .viewer-dir").inner_text()
            f = page.locator(f"{v} .viewer-file").inner_text()
            check("desktop: the label is STILL the path alone", (d, f) == ("docs/", "COMPARATIVO.md"), f"{d!r} {f!r}")
            check("desktop: captions visible", display(page, f"{v} [data-act='save'] .vbtn-label") != "none", display(page, f"{v} [data-act='save'] .vbtn-label"))
            check("desktop: no Contents button", display(page, f"{v} .md-toc-btn") == "none")
            nav = rect(page, f"{v} .md-outline")
            check("desktop: the outline is the 190px column", nav and abs(nav["w"] - 190) < 1 and display(page, f"{v} .md-outline") == "block", f"nav={nav}")
            page.evaluate(f"() => {{ if (!{SH}.kanbanOpen) {SH}.toggleKanban(); }}")
            page.wait_for_timeout(300)
            inject_issue(page, slug)
            page.evaluate(f"() => {SH}.openIssue(2)")
            page.wait_for_selector(".kanban-detail.open .kd-head", timeout=5000)
            page.wait_for_timeout(300)
            board = rect(page, ".kanban")
            drawer = rect(page, ".kanban-detail.open")
            check("desktop: the drawer is NOT full width", drawer and board and drawer["w"] < board["w"] * 0.7, f"drawer={drawer} board={board}")
            x = rect(page, ".kanban-detail.open .kd-x")
            state = rect(page, ".kanban-detail.open .kd-state")
            check("desktop: close is LAST, wearing the X", x and state and x["x"] > state["x"] and display(page, ".kanban-detail.open .kd-x .bi-x-lg") != "none", f"{x} {state} {display(page, '.kanban-detail.open .kd-x .bi-x-lg')}")
            ctx.close()

            check("no page errors", not errors, repr(errors[:3]))
            browser.close()
    finally:
        stop(proc)

    total, passed = len(results), sum(results)
    print(f"\n{passed}/{total} passed", flush=True)
    sys.exit(0 if passed == total else 1)


if __name__ == "__main__":
    main()
