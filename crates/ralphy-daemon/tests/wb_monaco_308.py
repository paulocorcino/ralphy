"""#308 browser acceptance: Monaco is the workbench's one editor.

One Playwright pass over a REAL daemon proving the source pane runs on Monaco
— resolving languages from the path, highlighting, finding, saving through the
unchanged Write path — and that CodeMirror is gone from the served tree.

Scenario 1  a .rs tab mounts .monaco-editor, its model language is `rust`
Scenario 2  the rendered lines carry >= 3 distinct `mtk*` token classes
Scenario 3  the Find button opens Monaco's own find widget
Scenario 4  typing + Save writes the edited bytes to the real file on disk
Scenario 5  window.CodeMirror is undefined and its vendor path 404s
Scenario 6  the editor ground is ADR-0035's --log-bg (rgb(26, 22, 19))
Scenario 7  Cargo.toml resolves to the `ini` language
Scenario 8  the file:// demo mounts Monaco OR degrades to .code-fallback

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
            kw = page.evaluate(
                "() => { const el = Array.from(document.querySelectorAll("
                "'.code-viewer .view-lines span[class*=mtk]')).find(e => e.textContent.trim() === 'fn');"
                " return el ? getComputedStyle(el).color : null; }"
            )
            check(
                "a rust keyword is painted by the wb theme (#c98a7d)",
                kw == "rgb(201, 138, 125)",
                f"got={kw!r}",
            )

            # --- scenario 6: ADR-0035's ground colour -------------------------
            bg = page.evaluate(
                "() => getComputedStyle(document.querySelector"
                "('.code-viewer .monaco-editor .monaco-editor-background')).backgroundColor"
            )
            check("the editor ground is ADR-0035's --log-bg", bg == "rgb(26, 22, 19)", f"got={bg!r}")
            cm_css = page.evaluate(
                "() => Array.from(document.styleSheets).some(s => { try {"
                " return Array.from(s.cssRules).some(r => (r.selectorText || '').includes('.cm-s-wb'));"
                " } catch (e) { return false; } })"
            )
            check("styles.css no longer ships a .cm-s-wb rule", not cm_css)

            page.screenshot(path=os.path.join(SHOT_DIR, "308-monaco-source-2026-07-25.png"))

            # --- scenario 3: Monaco's own find widget -------------------------
            page.click(".code-viewer [data-act='find']")
            page.wait_for_function(
                "() => { const w = document.querySelector('.code-viewer .monaco-editor .find-widget');"
                " return !!w && w.classList.contains('visible'); }",
                timeout=10000,
            )
            check("the Find button opens Monaco's find widget", True)
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
            check(f"the file:// demo reaches a readable pane ({branch})", True)
            demo.close()

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 13
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("MONACO IS THE ONE EDITOR")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
