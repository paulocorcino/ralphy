"""ADR-0036 amendment 2026-09-22 browser acceptance: text has an encoding.

One Playwright pass over a REAL daemon proving that a Windows working tree's
files open, read right, save back byte-for-byte, and say what they are.

Scenario 1  `spike-cp1252.md` (a `§`/`×`/`—` file as Set-Content writes it) opens
            a markdown tab — it no longer flashes `binary` and closes — and the
            pill says `Windows-1252`
Scenario 2  `ps5-utf16.txt` (Windows PowerShell 5 `Out-File`) opens with the
            pill `UTF-16 LE BOM`, and its glyphs are right
Scenario 3  `bom8.txt` (UTF-8 with a BOM) reads `olá`, pill `UTF-8 BOM`
Scenario 4  saving each of the three back writes the SAME bytes the fixture had
Scenario 5  `blob.dat` (a NUL-bearing file under an extension the client does not
            pre-classify) opens a refused pane that names `binary` — the tab STAYS
Scenario 6  typing `→` into the cp1252 file and saving raises the design-system
            dialog ("Cannot save as windows-1252"); confirming rewrites the file
            as UTF-8 and the pill flips to `UTF-8`
Scenario 7  "Save with…" UTF-16 LE from the pill's menu writes a `FF FE` file
Scenario 8  `tree.grep` finds `240×180` inside the cp1252 file

Requires `playwright` (`pip install playwright`, then `playwright install chromium`).

Boots a Localhost daemon on 7462 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/0036-encoding-2026-09-22.png.
Run: python crates/ralphy-daemon/tests/wb_encoding.py   (exit 0 = all pass)
"""

import os
import subprocess
import sys
import tempfile
import time
import urllib.request

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7462
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

# The fixtures, as bytes: what the three Windows writers produce.
CP1252_TEXT = "# Spike §6\r\n\r\n240×180 cards — ok\r\n"
CP1252 = CP1252_TEXT.encode("cp1252")
UTF16_TEXT = "Out-File wrote this: § × —\r\n"
UTF16 = b"\xff\xfe" + UTF16_TEXT.encode("utf-16-le")
BOM8_TEXT = "olá com BOM"
BOM8 = b"\xef\xbb\xbf" + BOM8_TEXT.encode("utf-8")

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
    empty = tempfile.mkdtemp(prefix="wbenc_empty_")
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
    d = tempfile.mkdtemp(prefix="wbenc_repo_")
    subprocess.run(["git", "init", "-q"], cwd=d, check=True)
    with open(os.path.join(d, "spike-cp1252.md"), "wb") as f:
        f.write(CP1252)
    with open(os.path.join(d, "ps5-utf16.txt"), "wb") as f:
        f.write(UTF16)
    with open(os.path.join(d, "bom8.txt"), "wb") as f:
        f.write(BOM8)
    with open(os.path.join(d, "blob.dat"), "wb") as f:
        f.write(bytes([0, 1, 2, 3, 0, 0, 255]))
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = empty_env(daemon_dir)
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


def tab_ids(page):
    return page.evaluate(f"() => {SH}.tabs.map(t => t.id)")


def open_from_tree(page, title):
    page.wait_for_function(
        f"(t) => !!{SH}._tree && !!{SH}._tree.findFirst(n => n.title === t)", arg=title, timeout=20000
    )
    page.evaluate(f"(t) => {SH}.openFile({SH}._tree.findFirst(n => n.title === t))", arg=title)


def wait_pane(page, tab_id, cls, timeout=20000):
    page.wait_for_function(
        "([id, cls]) => !!document.querySelector(`.${cls}[data-tab-id=\"${id}\"]`)",
        arg=[tab_id, cls],
        timeout=timeout,
    )


def pill(page, tab_id):
    return page.evaluate(
        "(id) => document.querySelector(`.viewer[data-tab-id=\"${id}\"] .viewer-enc-label`)?.textContent",
        tab_id,
    )


def wait_editor(page, tab_id):
    page.wait_for_function(
        "(id) => { const el = document.querySelector(`.viewer[data-tab-id=\"${id}\"]`);"
        " return !!el && !!el.querySelector('.monaco-editor'); }",
        arg=tab_id,
        timeout=30000,
    )


def save_and_wait(page, tab_id, fixture, rel, want, timeout=10.0):
    """Click the pane's Save and wait for the file's bytes to become `want`."""
    page.evaluate(
        "(id) => document.querySelector(`.viewer[data-tab-id=\"${id}\"] [data-act=\"save\"]`).click()", tab_id
    )
    path = os.path.join(fixture, rel)
    deadline = time.time() + timeout
    while time.time() < deadline:
        with open(path, "rb") as f:
            got = f.read()
        if got == want:
            return got
        time.sleep(0.2)
    return got


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbenc_reg_")
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
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)
            page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)

            # --- scenario 1: the cp1252 markdown opens ---------------------------
            cp_id = f"file:{slug}:spike-cp1252.md"
            open_from_tree(page, "spike-cp1252.md")
            wait_pane(page, cp_id, "md-viewer")
            check("the cp1252 file opens a markdown tab (no binary close)", cp_id in tab_ids(page))
            body = page.evaluate(
                "(id) => document.querySelector(`.md-viewer[data-tab-id=\"${id}\"] .md-body`).textContent", cp_id
            )
            check("its glyphs decode right", "§6" in body and "240×180" in body and "—" in body, f"got={body!r}")
            check("the pill says Windows-1252", pill(page, cp_id) == "Windows-1252", f"got={pill(page, cp_id)!r}")

            # --- scenario 2: the PS5 UTF-16 file ---------------------------------
            u16_id = f"file:{slug}:ps5-utf16.txt"
            open_from_tree(page, "ps5-utf16.txt")
            wait_pane(page, u16_id, "code-viewer")
            wait_editor(page, u16_id)
            check("the UTF-16 pill says UTF-16 LE BOM", pill(page, u16_id) == "UTF-16 LE BOM", f"got={pill(page, u16_id)!r}")
            u16_text = page.evaluate(f"() => WBViewer.descOf('{u16_id}').content")
            check("its glyphs decode right", u16_text == UTF16_TEXT, f"got={u16_text!r}")

            # --- scenario 3: UTF-8 BOM ------------------------------------------
            b8_id = f"file:{slug}:bom8.txt"
            open_from_tree(page, "bom8.txt")
            wait_pane(page, b8_id, "code-viewer")
            wait_editor(page, b8_id)
            check("the BOM'd UTF-8 pill says UTF-8 BOM", pill(page, b8_id) == "UTF-8 BOM", f"got={pill(page, b8_id)!r}")
            b8_text = page.evaluate(f"() => WBViewer.descOf('{b8_id}').content")
            check("the BOM is stripped from the text", b8_text == BOM8_TEXT, f"got={b8_text!r}")

            page.screenshot(path=os.path.join(SHOT_DIR, "0036-encoding-2026-09-22.png"))

            # --- scenario 4: each saves back byte-for-byte -----------------------
            for tab, rel, want in [(cp_id, "spike-cp1252.md", CP1252), (u16_id, "ps5-utf16.txt", UTF16), (b8_id, "bom8.txt", BOM8)]:
                page.evaluate(f"() => {SH}.activate('{tab}')")
                got = save_and_wait(page, tab, fixture, rel, want)
                check(f"{rel} saves back byte-for-byte", got == want, f"got={got[:16]!r}")

            # --- scenario 5: a binary keeps its tab ------------------------------
            bin_id = f"file:{slug}:blob.dat"
            before = tab_ids(page)
            open_from_tree(page, "blob.dat")
            wait_pane(page, bin_id, "refused-viewer")
            check("blob.dat's tab STAYS with a refused pane", bin_id in tab_ids(page), f"before={before}")
            why = page.evaluate(
                "(id) => document.querySelector(`.refused-viewer[data-tab-id=\"${id}\"] .refused-text`).textContent", bin_id
            )
            check("the pane names the reason", "binary" in why, f"got={why!r}")

            # --- scenario 6: unencodable → dialog → UTF-8 ------------------------
            page.evaluate(f"() => {SH}.activate('{cp_id}')")
            # Edit through the markdown pane's editor (toggle to edit, then set).
            page.evaluate(
                "(id) => document.querySelector(`.md-viewer[data-tab-id=\"${id}\"] [data-act=\"toggle\"]`).click()", cp_id
            )
            wait_editor(page, cp_id)
            page.evaluate(
                f"""() => {{ const d = WBViewer.descOf('{cp_id}');
                    const ed = monaco.editor.getModels().find(m => m.uri.path.endsWith('spike-cp1252.md'));
                    ed.setValue(d.content + '\\u2192 arrow\\r\\n'); }}"""
            )
            page.evaluate(
                "(id) => document.querySelector(`.md-viewer[data-tab-id=\"${id}\"] [data-act=\"save\"]`).click()", cp_id
            )
            page.wait_for_function(f"() => {SH}.confirmModal.open === true", timeout=10000)
            modal = page.evaluate(f"() => {SH}.confirmModal")
            check(
                "the design-system dialog offers UTF-8",
                "windows-1252" in modal["title"] and modal["confirmLabel"] == "Save as UTF-8",
                f"got={modal}",
            )
            with open(os.path.join(fixture, "spike-cp1252.md"), "rb") as f:
                check("nothing was written before the answer", f.read() == CP1252)
            page.evaluate(f"() => {SH}.confirmRespond(true)")
            want = (CP1252_TEXT + "→ arrow\r\n").encode("utf-8")
            deadline = time.time() + 10
            got = None
            while time.time() < deadline:
                with open(os.path.join(fixture, "spike-cp1252.md"), "rb") as f:
                    got = f.read()
                if got == want:
                    break
                time.sleep(0.2)
            check("confirming rewrote the file as UTF-8", got == want, f"got={got[-20:]!r}")
            page.wait_for_function(
                "(id) => document.querySelector(`.viewer[data-tab-id=\"${id}\"] .viewer-enc-label`).textContent === 'UTF-8'",
                arg=cp_id,
                timeout=5000,
            )
            check("the pill flipped to UTF-8", pill(page, cp_id) == "UTF-8")

            # --- scenario 7: save with UTF-16 LE from the menu -------------------
            page.evaluate(f"() => {SH}.activate('{b8_id}')")
            page.evaluate(
                "(id) => document.querySelector(`.viewer[data-tab-id=\"${id}\"] [data-act=\"encoding\"]`).click()", b8_id
            )
            page.evaluate(
                "(id) => [...document.querySelectorAll(`.viewer[data-tab-id=\"${id}\"] .enc-item[data-savewith]`)]"
                ".find(b => b.textContent.trim() === 'UTF-16 LE').click()",
                b8_id,
            )
            want16 = b"\xff\xfe" + BOM8_TEXT.encode("utf-16-le")
            deadline = time.time() + 10
            while time.time() < deadline:
                with open(os.path.join(fixture, "bom8.txt"), "rb") as f:
                    got = f.read()
                if got == want16:
                    break
                time.sleep(0.2)
            check("Save with UTF-16 LE wrote FF FE + UTF-16", got == want16, f"got={got!r}")
            check("and the pill says so", pill(page, b8_id) == "UTF-16 LE BOM", f"got={pill(page, b8_id)!r}")

            # --- scenario 8: grep sees the cp1252 text ---------------------------
            # Re-create a cp1252 file (the first became UTF-8 in scenario 6).
            with open(os.path.join(fixture, "again-cp1252.md"), "wb") as f:
                f.write(CP1252)
            hits = page.evaluate(
                f"""async () => (await WBDaemon.observe('tree.grep', {{ repo: '{slug}', query: '240×180' }})).hits.map(h => h.path)"""
            )
            check("tree.grep finds 240×180 in the cp1252 file", "again-cp1252.md" in hits, f"got={hits}")

            browser.close()
    finally:
        stop(proc)

    ok = all(results)
    print(f"\n{sum(results)}/{len(results)} passed")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
