"""#308 browser acceptance: Monaco is the workbench's one editor.

One Playwright pass over a REAL daemon proving the source pane runs on Monaco
— resolving languages from the path, highlighting, finding, saving through the
unchanged Write path — and that CodeMirror is gone from the served tree.

Scenario 1  a .rs tab mounts .monaco-editor, its model language is `rust`
Scenario 2  the rendered lines carry >= 3 distinct `mtk*` token classes
Scenario 3  the Find button opens Monaco's own find widget
Scenario 4  typing + Save writes the edited bytes to the real file on disk
Scenario 5  window.CodeMirror is undefined and its vendor path 404s
Scenario 6  the editor ground is black (rgb(0, 0, 0))
Scenario 7  Cargo.toml resolves to the `ini` language
Scenario 8  the file:// demo mounts Monaco OR degrades to .code-fallback
Scenario 9  a .json tab is tokenized (its tokenizer is a mode-config provider,
            and JSON has no basic-languages grammar to fall back on)
Scenario 10 the async boot gap: mid-boot bytes reach the model, close disposes
            the model, close-then-reopen leaves exactly one
Scenario 11 the markdown raw-source editor mounts Monaco and its edits survive
            the toggle back to preview

Boots a Localhost daemon on 7408 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/308-monaco-source-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_monaco_308.py   (exit 0 = all pass)
"""

import io
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7408
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_monaco_308.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
UI_DIR = os.path.join(REPO_ROOT, "crates", "ralphy-daemon", "assets", "ui")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

MAIN_RS = """// the #308 fixture
use std::collections::HashMap;

/// A registered repository the daemon can launch agents into.
#[derive(Debug, Clone)]
pub struct Repo {
    pub slug: String,
    pub reachable: bool,
}

fn main() {
    let mut seen: HashMap<&str, u32> = HashMap::new();
    seen.insert("monaco", 308);
    println!("{seen:?}");
}
"""

CARGO_TOML = """# the #308 fixture
[package]
name = "wb308"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
"""

PKG_JSON = '{\n  "name": "wb308",\n  "version": "0.1.0",\n  "private": true\n}\n'

# Pinned exactly (see the exit gate): a scenario that silently stops running
# must fail the run, not shrink it.
EXPECTED_CHECKS = 22

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
    empty = tempfile.mkdtemp(prefix="wb308_empty_")
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
    d = tempfile.mkdtemp(prefix="wb308_repo_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "src").mkdir()
    (p / "src" / "main.rs").write_text(MAIN_RS, encoding="utf-8")
    (p / "Cargo.toml").write_text(CARGO_TOML, encoding="utf-8")
    (p / "pkg.json").write_text(PKG_JSON, encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb308@example.com"],
        ["git", "config", "user.name", "wb308"],
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
    # after any assets/ui edit or the browser loads yesterday's editor.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def open_file(page, slug, path, title, ftype="code"):
    """Open a file tab and wait for its editor to actually mount. Monaco boots
    through an AMD loader, so the mount is asynchronous — every assertion below
    waits on a predicate rather than reading right after the state change."""
    page.evaluate(
        "([project, path, title, ftype]) => "
        f"{SH}.openTab({{ project, path, title, ftype }})",
        [slug, path, title, ftype],
    )
    page.wait_for_function(
        "(p) => { const m = window.monaco && window.monaco.editor.getModels()"
        ".find(m => m.uri.path.endsWith(p)); return !!m && !!document.querySelector('.monaco-editor'); }",
        arg=path.split("/")[-1],
        timeout=30000,
    )


def language_of(page, filename):
    return page.evaluate(
        "(p) => window.monaco.editor.getModels().find(m => m.uri.path.endsWith(p)).getLanguageId()",
        filename,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb308_reg_")
    repo_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, repo_dir)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            # A bare `return` here would skip the exit gate below and report
            # success with ZERO browser assertions run.
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows (KNOWLEDGE.md).
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)

            # The AMD loader's global `define` must not have eaten the UMD
            # vendors loaded before it (index.html load-order invariant).
            umd = page.evaluate(
                "() => ({ marked: typeof window.marked, mermaid: typeof window.mermaid,"
                " dompurify: typeof window.DOMPurify, wunderbaum: typeof window.mar10,"
                " terminal: typeof window.Terminal })"
            )
            check(
                "the AMD loader did not swallow the UMD vendors",
                all(v != "undefined" for v in umd.values()),
                f"got={umd}",
            )

            # --- scenario 1: a .rs tab mounts Monaco, resolved as rust --------
            open_file(page, slug, "src/main.rs", "main.rs")
            mounted = page.evaluate("() => document.querySelectorAll('.code-viewer .monaco-editor').length")
            check("the source pane mounts a Monaco editor", mounted >= 1, f"got={mounted}")
            lang = language_of(page, "main.rs")
            check("src/main.rs resolves to the rust language", lang == "rust", f"got={lang!r}")

            # --- scenario 2: real syntax highlighting -------------------------
            # Count the `mtk<N>` COLOUR slots only: bracket-highlighting and
            # decoration classes ride along on mtk1 and would otherwise fake
            # three "distinct" classes while every token is still unstyled (the
            # grammar chunk loads lazily, after the first plaintext paint).
            MTK = (
                "Array.from(new Set(Array.from(document.querySelectorAll("
                "'.code-viewer .view-lines span[class*=mtk]')).flatMap(e => "
                "Array.from(e.classList).filter(c => /^mtk[0-9]+$/.test(c)))))"
            )
            page.wait_for_function(f"() => {MTK}.length >= 3", timeout=20000)
            slots = page.evaluate(f"() => {MTK}")
            check(
                "the rendered lines carry >= 3 distinct token colour slots",
                len(set(slots)) >= 3,
                f"got={sorted(set(slots))}",
            )
            # …and the colours are the theme's, not vs-dark's: `fn`/`let` are
            # `keyword`, which the wb theme paints #c98a7d.
            KW = (
                "(() => { const el = Array.from(document.querySelectorAll("
                "'.code-viewer .view-lines span[class*=mtk]')).find(e => e.textContent.trim() === 'fn');"
                " return el ? getComputedStyle(el).color : null; })()"
            )
            # The retokenize repaint can land a frame after the slot count rises,
            # so wait on the predicate rather than reading once (flake vector).
            kw = None
            try:
                page.wait_for_function(f"() => {KW} === 'rgb(201, 138, 125)'", timeout=10000)
                kw = page.evaluate(f"() => {KW}")
            except Exception as exc:
                kw = f"timeout: {exc.__class__.__name__}"
            check(
                "a rust keyword is painted by the wb theme (#c98a7d)",
                kw == "rgb(201, 138, 125)",
                f"got={kw!r}",
            )

            # --- scenario 6: the code surface is always black ------------------
            bg = page.evaluate(
                "() => getComputedStyle(document.querySelector"
                "('.code-viewer .monaco-editor .monaco-editor-background')).backgroundColor"
            )
            check("the editor ground is black", bg == "rgb(0, 0, 0)", f"got={bg!r}")
            # Assert the sheet is REACHABLE first: a swallowed cssRules error (or
            # a styles.css that failed to load) would otherwise read as "the rule
            # is gone" and pass against a page with no styles at all.
            css = page.evaluate(
                "() => { let seen = 0, cm = 0;"
                " for (const s of document.styleSheets) { let rules;"
                "   try { rules = Array.from(s.cssRules); } catch (e) { continue; }"
                "   for (const r of rules) { const sel = r.selectorText || '';"
                "     if (sel.includes('.viewer-body')) seen++;"
                "     if (sel.includes('.cm-s-wb')) cm++; } }"
                " return { seen, cm }; }"
            )
            check(
                "styles.css is reachable and no longer ships a .cm-s-wb rule",
                css["seen"] > 0 and css["cm"] == 0,
                f"got={css}",
            )

            page.screenshot(path=os.path.join(SHOT_DIR, "308-monaco-source-2026-07-25.png"))

            # --- scenario 3: Monaco's own find widget -------------------------
            page.click(".code-viewer [data-act='find']")
            # try/except, not a bare wait: a raise here would abort the run and
            # erase the Save and CodeMirror-removal evidence below.
            try:
                page.wait_for_function(
                    "() => { const w = document.querySelector('.code-viewer .monaco-editor .find-widget');"
                    " return !!w && w.classList.contains('visible'); }",
                    timeout=10000,
                )
                check("the Find button opens Monaco's find widget", True)
            except Exception as exc:
                check("the Find button opens Monaco's find widget", False, f"{exc.__class__.__name__}")
            page.keyboard.press("Escape")

            # --- scenario 4: Save still goes through the Write path -----------
            page.click(".code-viewer .view-lines")
            page.keyboard.press("Control+Home")
            page.keyboard.type("// edited by wb_monaco_308\n")
            edited = page.evaluate(
                "() => window.monaco.editor.getModels().find(m => m.uri.path.endsWith('main.rs')).getValue()"
            )
            check(
                "typing reaches the model",
                edited.startswith("// edited by wb_monaco_308"),
                f"head={edited[:40]!r}",
            )
            page.click(".code-viewer [data-act='save']")
            on_disk_path = Path(repo_dir, "src", "main.rs")
            deadline = time.time() + 10
            on_disk = ""
            while time.time() < deadline:
                # newline="" — the model's EOLs must reach disk verbatim.
                with io.open(on_disk_path, encoding="utf-8", newline="") as fh:
                    on_disk = fh.read()
                if on_disk == edited:
                    break
                time.sleep(0.2)
            check(
                "Save writes the edited bytes through the unchanged Write path",
                on_disk == edited,
                f"disk={len(on_disk)}B model={len(edited)}B",
            )

            # --- scenario 5: CodeMirror is gone -------------------------------
            cm_global = page.evaluate("() => typeof window.CodeMirror")
            check("window.CodeMirror is undefined", cm_global == "undefined", f"got={cm_global!r}")
            status = page.evaluate(
                "async () => (await fetch('vendor/codemirror/codemirror.js')).status"
            )
            check("the CodeMirror vendor path 404s", status == 404, f"got={status}")

            # --- scenario 7: .toml resolves onto the ini language -------------
            open_file(page, slug, "Cargo.toml", "Cargo.toml")
            toml_lang = language_of(page, "Cargo.toml")
            check("Cargo.toml resolves to the ini language", toml_lang == "ini", f"got={toml_lang!r}")

            # --- scenario 9: JSON still tokenizes ------------------------------
            # JSON is the ONE language with no basic-languages Monarch grammar:
            # its tokenizer is the `tokens` provider on jsonDefaults, so a mode
            # configuration that switches that off leaves .json as plaintext.
            open_file(page, slug, "pkg.json", "pkg.json")
            json_lang = language_of(page, "pkg.json")
            check("pkg.json resolves to the json language", json_lang == "json", f"got={json_lang!r}")
            JSON_MTK = (
                "Array.from(new Set(Array.from(document.querySelectorAll("
                "'.code-viewer .view-lines span[class*=mtk]')).flatMap(e => "
                "Array.from(e.classList).filter(c => /^mtk[0-9]+$/.test(c)))))"
            )
            json_slots = []
            try:
                page.wait_for_function(f"() => {JSON_MTK}.length >= 2", timeout=15000)
                json_slots = page.evaluate(f"() => {JSON_MTK}")
            except Exception as exc:
                json_slots = [f"timeout: {exc.__class__.__name__}"]
            check(
                "a .json tab is tokenized, not plaintext",
                len(set(json_slots)) >= 2,
                f"got={sorted(set(json_slots))}",
            )

            # --- scenario 10: the async boot gap -------------------------------
            # Every scenario above waits for the mount first. These two exercise
            # what the swap actually introduced: bytes and a close landing while
            # Monaco is still booting.
            baseline_models = page.evaluate("() => monaco.editor.getModels().length")
            page.evaluate(
                "([project, path]) => { const id = `file:${project}:${path}`;"
                " WBViewer.open({ id, project, path, ftype: 'code', content: 'before boot\\n' });"
                " WBViewer.externalChange(id, 'landed mid-boot\\n'); }",
                [slug, "src/gap.rs"],
            )
            page.wait_for_function(
                "() => { const m = monaco.editor.getModels().find(m => m.uri.path.endsWith('gap.rs'));"
                " return !!m; }",
                timeout=30000,
            )
            gap = page.evaluate(
                "() => monaco.editor.getModels().find(m => m.uri.path.endsWith('gap.rs')).getValue()"
            )
            check(
                "bytes arriving mid-boot reach the mounted model",
                gap == "landed mid-boot\n",
                f"got={gap!r}",
            )
            page.evaluate(f"() => WBViewer.close('file:{slug}:src/gap.rs')")
            after_close = page.evaluate("() => monaco.editor.getModels().length")
            check(
                "closing a tab disposes its model",
                after_close == baseline_models,
                f"baseline={baseline_models} after={after_close}",
            )
            # A tab closed WHILE Monaco boots must not mount an orphan editor.
            # Monaco is warm by now, so drive the same race through the record
            # identity guard: open, close, reopen the SAME id, and assert exactly
            # one model survives for that path.
            page.evaluate(
                "([project, path]) => { const id = `file:${project}:${path}`;"
                " WBViewer.open({ id, project, path, ftype: 'code', content: 'first\\n' });"
                " WBViewer.close(id);"
                " WBViewer.open({ id, project, path, ftype: 'code', content: 'second\\n' }); }",
                [slug, "src/race.rs"],
            )
            page.wait_for_function(
                "() => monaco.editor.getModels().filter(m => m.uri.path.endsWith('race.rs')).length >= 1",
                timeout=30000,
            )
            page.wait_for_timeout(500)
            race = page.evaluate(
                "() => monaco.editor.getModels().filter(m => m.uri.path.endsWith('race.rs'))"
                ".map(m => m.getValue())"
            )
            check(
                "close-then-reopen inside the boot window leaves exactly one model",
                race == ["second\n"],
                f"got={race!r}",
            )
            page.evaluate(f"() => WBViewer.close('file:{slug}:src/race.rs')")

            # --- scenario 11: the markdown raw-source editor -------------------
            page.evaluate(
                "([project, path]) => WBViewer.open({ id: `file:${project}:${path}`, project, path,"
                " ftype: 'markdown', content: '# heading\\n\\nbody text\\n' })",
                [slug, "NOTES.md"],
            )
            page.evaluate(f"() => WBViewer.setActive('file:{slug}:NOTES.md')")
            page.click(".md-viewer [data-act='toggle']")
            page.wait_for_function(
                "() => { const m = monaco.editor.getModels().find(m => m.uri.path.endsWith('NOTES.md'));"
                " return !!m && !!document.querySelector('.md-editor .monaco-editor'); }",
                timeout=30000,
            )
            md_lang = page.evaluate(
                "() => monaco.editor.getModels().find(m => m.uri.path.endsWith('NOTES.md')).getLanguageId()"
            )
            check("the markdown raw editor resolves markdown", md_lang == "markdown", f"got={md_lang!r}")
            page.evaluate(
                "() => monaco.editor.getModels().find(m => m.uri.path.endsWith('NOTES.md'))"
                ".setValue('# edited heading\\n\\nbody text\\n')"
            )
            page.click(".md-viewer [data-act='toggle']")
            page.wait_for_function(
                "() => document.querySelector('.md-viewer .md-body').textContent.includes('edited heading')",
                timeout=10000,
            )
            check("toggling back to preview keeps the edited bytes", True)
            page.evaluate(f"() => WBViewer.close('file:{slug}:NOTES.md')")

            # --- scenario 8: the file:// demo ---------------------------------
            demo = ctx.new_page()
            demo.goto("file:///" + os.path.join(UI_DIR, "index.html").replace("\\", "/"))
            demo.wait_for_selector("[x-data]", timeout=15000)
            demo.wait_for_function(f"() => {SH}.projects.length > 0", timeout=15000)
            demo.evaluate(
                f"() => {SH}.openTab"
                "({ project: Alpine.$data(document.querySelector('[x-data]')).projects[0].slug,"
                " path: 'demo.rs', title: 'demo.rs', ftype: 'code' })"
            )
            demo.wait_for_function(
                "() => !!document.querySelector('.code-viewer .monaco-editor')"
                " || !!document.querySelector('.code-viewer .code-fallback')",
                timeout=30000,
            )
            branch = demo.evaluate(
                "() => document.querySelector('.code-viewer .monaco-editor') ? 'monaco' : 'fallback'"
            )
            if branch == "monaco":
                ok = demo.evaluate(
                    "() => { const m = window.monaco.editor.getModels()"
                    ".find(m => m.uri.path.endsWith('demo.rs')); return !!m && m.getValue().length > 0; }"
                )
            else:
                ok = demo.evaluate(
                    "() => { const pre = document.querySelector('.code-viewer .code-fallback');"
                    " return !!pre && pre.textContent.length > 0; }"
                )
            check(f"the file:// demo pane carries the file's bytes ({branch})", ok)
            demo.close()

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # Pinned exactly: a `>=` gate lets a scenario vanish and still print success.
    ok = all(results) and len(results) == EXPECTED_CHECKS
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if len(results) != EXPECTED_CHECKS:
        print(f"EXPECTED {EXPECTED_CHECKS} checks, ran {len(results)}", flush=True)
    if ok:
        print("MONACO IS THE ONE EDITOR")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
