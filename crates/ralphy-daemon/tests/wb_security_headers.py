"""Security audit 2026-09-21 F3 browser acceptance: the CSP breaks nothing.

One Playwright pass over a REAL daemon proving the response headers ride every
answer and that the Content-Security-Policy — hash-allowed inline scripts,
`'unsafe-eval'` for Alpine, `blob:` workers for Monaco, `data:` images for the
QR — produces ZERO `securitypolicyviolation` events across the surfaces that
need each allowance.

Scenario 1  GET / carries CSP, X-Frame-Options, X-Content-Type-Options,
            Referrer-Policy; so does an API answer
Scenario 2  the shell boots (Alpine evaluates, the demo-seed inline gate runs)
            with no violation
Scenario 3  a .rs tab mounts Monaco — its language worker is a blob: worker
Scenario 4  a markdown tab with a mermaid block renders the SVG (inline styles)
Scenario 5  the Security modal enrols TOTP and shows the QR (`data:` image)
Scenario 6  detached.html and detached-fence.html boot their inline scripts
Scenario 7  opening a project dials /ws/tree — a refused ws: would be a violation

Boots a Localhost daemon on 7409 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/sec-audit-f3-csp-2026-09-21.png (the TOTP secret in it
is a scratch daemon's PENDING seed, discarded with its tempdir — never armed).
Run: python crates/ralphy-daemon/tests/wb_security_headers.py   (exit 0 = all pass)
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

PORT = 7409
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

MAIN_RS = "fn main() {\n    println!(\"csp\");\n}\n"
NOTES_MD = "# Notes\n\n```mermaid\ngraph TD; A-->B;\n```\n"

# Records every CSP violation the document sees, before any script runs.
RECORDER = """
window.__csp = [];
document.addEventListener('securitypolicyviolation', (e) => {
  window.__csp.push(e.violatedDirective + ' ' + (e.blockedURI || '') + ' @' + e.sourceFile + ':' + e.lineNumber);
});
"""

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
    empty = tempfile.mkdtemp(prefix="wbcsp_empty_")
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
    d = tempfile.mkdtemp(prefix="wbcsp_repo_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "src").mkdir()
    (p / "src" / "main.rs").write_text(MAIN_RS, encoding="utf-8")
    (p / "NOTES.md").write_text(NOTES_MD, encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbcsp@example.com"],
        ["git", "config", "user.name", "wbcsp"],
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


def headers_of(path):
    with urllib.request.urlopen(BASE.rstrip("/") + path, timeout=5) as r:
        return {k.lower(): v for k, v in r.headers.items()}


def violations(page):
    return page.evaluate("window.__csp")


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbcsp_reg_")
    fixture = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture)
    proc = launch(daemon_dir)
    try:
        check("daemon listening", wait_listening(BASE))

        # ── 1. the headers, on the shell and on the API ──────────────────
        for path in ("/", "/api/session"):
            h = headers_of(path)
            check(
                f"{path} carries the four headers",
                h.get("x-frame-options") == "DENY"
                and h.get("x-content-type-options") == "nosniff"
                and h.get("referrer-policy") == "no-referrer"
                and "frame-ancestors 'none'" in h.get("content-security-policy", ""),
                {k: v for k, v in h.items() if k in ("x-frame-options", "content-security-policy")},
            )

        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            ctx.add_init_script(RECORDER)
            page = ctx.new_page()
            console_csp = []
            page.on("console", lambda m: console_csp.append(m.text) if "Content Security Policy" in m.text else None)

            # ── 2. the shell boots ────────────────────────────────────────
            page.goto(BASE)
            page.wait_for_function("() => window.Alpine && document.body.classList.length >= 0", timeout=20000)
            page.wait_for_timeout(1500)
            check("shell boots with no CSP violation", violations(page) == [], violations(page))

            # ── 7. the sockets (connect-src ws:) — a refused `ws://` would land
            #      in the violation list every later check reads.
            page.evaluate(f"{SH}.openProject && {SH}.openProject('{slug}')")
            page.wait_for_timeout(2000)
            check("opening a project (its /ws/tree socket) with no CSP violation", violations(page) == [], violations(page))

            # ── 3. Monaco + its blob: worker ──────────────────────────────
            page.evaluate(
                "([project, path, title]) => " f"{SH}.openTab({{ project, path, title, ftype: 'code' }})",
                [slug, "src/main.rs", "main.rs"],
            )
            page.wait_for_function(
                "() => window.monaco && document.querySelector('.monaco-editor') && window.monaco.editor.getModels().length > 0",
                timeout=30000,
            )
            page.wait_for_timeout(2500)
            check("Monaco mounted with no CSP violation", violations(page) == [], violations(page))

            # ── 4. markdown + mermaid ─────────────────────────────────────
            page.evaluate(
                "([project, path, title]) => " f"{SH}.openTab({{ project, path, title, ftype: 'markdown' }})",
                [slug, "NOTES.md", "NOTES.md"],
            )
            page.wait_for_timeout(3000)
            has_svg = page.evaluate("() => !!document.querySelector('.mermaid svg')")
            check("mermaid rendered an SVG", has_svg, "" if has_svg else "no mermaid svg found (selector may differ)")
            check("markdown/mermaid with no CSP violation", violations(page) == [], violations(page))

            # ── 5. the QR (data: image) ───────────────────────────────────
            page.evaluate(f"{SH}.openSecurity()")
            page.wait_for_timeout(500)
            page.evaluate(f"{SH}.enrollTotp()")
            page.wait_for_function(f"() => {SH}.security.pendingEnroll === true", timeout=10000)
            page.wait_for_timeout(800)
            qr_ok = page.evaluate("() => !!document.querySelector('.totp-qr img, .totp-qr canvas, .totp-qr svg, .totp-qr table')")
            check("the TOTP QR rendered", qr_ok)
            check("QR with no CSP violation", violations(page) == [], violations(page))
            page.screenshot(path=os.path.join(SHOT_DIR, "sec-audit-f3-csp-2026-09-21.png"))
            page.evaluate(f"{SH}.cancelEnroll()")

            # ── 6. the popups' inline scripts are hash-allowed ────────────
            for popup in ("detached.html", "detached-fence.html"):
                p2 = ctx.new_page()
                p2.goto(BASE + popup)
                p2.wait_for_timeout(2500)
                v = violations(p2)
                check(f"{popup} boots with no CSP violation", v == [], v)
                p2.close()

            check("no CSP message reached the console", console_csp == [], console_csp[:3])
            browser.close()
    finally:
        stop(proc)

    ok = all(results)
    print(f"\n{sum(results)}/{len(results)} checks passed")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
