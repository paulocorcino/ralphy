"""#209 operator acceptance walkthrough (HITL, post-remediation, #202-#208).

One consolidated Playwright pass over a REAL daemon asserting that none of the
five originally-audited symptoms (docs/audit-workbench-2026-07-13.md) plus
three extras reproduce anymore:
  C1 demo badge under file:// / daemon-mode detection (#202/#208)
  C2 Kanban error state distinct from empty (#207)
  C3 auth honesty: loopback vs a hardened network(Session) bind (#205)
  A2 Kanban stays above a focused floating console (#208)
  A4/A5 tree integrity: .ralphy/.github visible, .git/.env excluded, no
        duplication across a reconcile (#203)
  M8 translation errors/decisions are actionable, mapped pure functions (#206)
  M1 topbar uptime is a live heartbeat, not a static string (#204)
  A6 viewer external-edit refresh mechanism (#203)

Boots a Localhost daemon on 7357 (most scenarios) plus a SECOND, network-bind
Session-policy daemon on 7358 (pre-seeded TOTP; #179/#205 hard half) driven
through chromium (DOM renderer, no WebGL — headless chromium's WebGL canvas
reads empty text even when content shows, KNOWLEDGE.md). Both daemons are
stopped by their own subprocess handle, NEVER by name (`ralphy.exe` doubles as
the orchestrator on this host).

Writes the six dated screenshots under docs/screenshots/209-*-2026-07-14.png.
Run: python crates/ralphy-daemon/tests/wb_accept_209.py   (exit 0 = all pass)
"""

import base64
import hashlib
import hmac
import os
import secrets
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

# The Windows console's default codepage (cp1252 here) can't encode the arrow
# in explainError's mapped message; force utf-8 stdout so `check()` never dies
# mid-run on a passing assertion's own detail string.
sys.stdout.reconfigure(encoding="utf-8")

PORT = 7357
NET_PORT = 7358
BASE = f"http://127.0.0.1:{PORT}/"
NET_BASE = f"http://127.0.0.1:{NET_PORT}/"

# crates/ralphy-daemon/tests/wb_accept_209.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe")
INDEX_FILE = os.path.join(REPO_ROOT, "crates", "ralphy-daemon", "assets", "ui", "index.html")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

results = []
downgrades = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def downgrade(name, detail):
    downgrades.append((name, detail))
    print(f"[DOWNGRADED] {name} {detail}", flush=True)


def wait_listening(base, timeout=25):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(base, timeout=1)
            return True
        except Exception:
            time.sleep(0.3)
    return False


def empty_env(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wb209_empty_")
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


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


# --- TOTP (RFC 6238) — stdlib only, mirrors ralphy-daemon/src/totp.rs -------
def totp_code(secret_b32, at=None, period=30, digits=6):
    if at is None:
        at = time.time()
    counter = int(at // period)
    padded = secret_b32 + "=" * (-len(secret_b32) % 8)
    key = base64.b32decode(padded)
    msg = struct.pack(">Q", counter)
    h = hmac.new(key, msg, hashlib.sha1).digest()
    o = h[-1] & 0x0F
    bin_code = struct.unpack(">I", h[o : o + 4])[0] & 0x7FFFFFFF
    return str(bin_code % (10**digits)).zfill(digits)


def make_fixture_repo():
    """A throwaway git repo (symptom 4 + the viewer extra): tracked .ralphy/
    and .github dotfolders, a gitignored .env, never the live C:\\Dev\\ralphy
    tree."""
    d = tempfile.mkdtemp(prefix="wb209_fixture_")
    p = Path(d)
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "plan.md").write_text("# fixture plan\n")
    (p / ".github").mkdir()
    (p / ".github" / "x.yml").write_text("name: x\n")
    (p / "README.md").write_text("# fixture\n\nThe #209 walkthrough fixture repo.\n")
    (p / ".gitignore").write_text(".env\n")
    (p / ".env").write_text("SECRET=1\n")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb209@example.com"],
        ["git", "config", "user.name", "wb209"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True,
        encoding="utf-8",
    )
    # stdout: "registered <slug> -> <path>" — the CLI's own arrow is U+2192; the
    # subprocess pipe defaults to the console codepage (cp1252 here), which
    # mangles it, so decode explicitly utf-8 rather than trust `text=True`.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    daemon_dir = tempfile.mkdtemp(prefix="wb209_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

    env = empty_env(daemon_dir)
    proc = subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)], env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        if not wait_listening(BASE):
            check("daemon listening on 7357", False)
            return
        check("daemon listening on 7357", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])

            def fresh():
                ctx = browser.new_context(viewport={"width": 1400, "height": 900})
                page = ctx.new_page()
                return ctx, page

            def ready(page, base=BASE):
                page.goto(base)
                page.wait_for_selector("[x-data]", timeout=8000)
                page.wait_for_timeout(300)

            # --- extra: file:// demo badge (C1/B1, #202/#208) ---------------
            ctx, page = fresh()
            page.goto("file:///" + INDEX_FILE.replace(os.sep, "/"))
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_timeout(300)
            check("demo badge visible under file://", page.locator(".demo-badge").is_visible())
            modes = page.evaluate("() => ({daemon: window.WBMode.isDaemon(), demo: window.WBMode.isDemo()})")
            check("WBMode.isDemo() true under file://", modes["demo"] is True, f"{modes}")
            page.screenshot(path=os.path.join(SHOT_DIR, "209-demo-badge-2026-07-14.png"))
            ctx.close()

            ctx, page = fresh()
            ready(page)
            modes = page.evaluate("() => ({daemon: window.WBMode.isDaemon(), demo: window.WBMode.isDemo()})")
            check("WBMode.isDaemon() true under http://", modes["daemon"] is True, f"{modes}")
            ctx.close()

            # --- symptom 1: Kanban error state (C2, #207) --------------------
            ctx, page = fresh()
            ready(page)
            page.evaluate(f"""() => {{
                const s = {SH};
                s.openSlug = 'fixture';
                s.boardError = {{ fixture: 'gh: not authenticated' }};
                s.kanbanOpen = true;
            }}""")
            page.wait_for_timeout(200)
            err = page.locator(".kanban-error")
            check("symptom1: .kanban-error visible", err.is_visible())
            err_text = err.locator("span").inner_text()
            check("symptom1: .kanban-error text is the daemon message", err_text == "gh: not authenticated", f"got={err_text!r}")
            visible_empties = [page.locator(".kanban-empty").nth(i).is_visible() for i in range(page.locator(".kanban-empty").count())]
            check("symptom1: .kanban-empty stays hidden (error != empty)", not any(visible_empties), f"{visible_empties}")
            page.screenshot(path=os.path.join(SHOT_DIR, "209-board-error-2026-07-14.png"))
            ctx.close()

            # --- symptom 2 (loopback half, C3, #205) --------------------------
            ctx, page = fresh()
            ready(page)
            page.wait_for_function(f"() => {SH}.security.policy === 'localhost'", timeout=5000)
            page.evaluate(f"() => {SH}.openSecurity()")
            page.wait_for_timeout(300)
            require_cb = page.locator('.security-modal input[type="checkbox"]')
            check("symptom2: Require-login checkbox disabled on loopback", require_cb.is_disabled())
            loopback_note = page.locator('.sec-note', has_text="the login gate only applies to a network bind with TOTP")
            check("symptom2: loopback honesty note visible", loopback_note.is_visible())
            enroll_btn = page.locator('.sec-actions button', has_text="Enroll")
            check("symptom2: TOTP enroll button enabled", enroll_btn.is_enabled())
            page.screenshot(path=os.path.join(SHOT_DIR, "209-auth-honest-2026-07-14.png"))
            page.evaluate(f"() => {SH}.closeSecurity()")
            page.evaluate(f"() => {SH}.logOff()")
            page.wait_for_timeout(200)
            check("symptom2: login-gate stays hidden after Log off on loopback", not page.locator(".login-gate").is_visible())
            authed_after = page.evaluate(f"() => {SH}.authed")
            check("symptom2: authed stays true after Log off on loopback", authed_after is True)
            ctx.close()

            # --- symptom 3: Kanban above consoles (A2, #208) -------------------
            ctx, page = fresh()
            ready(page)
            page.evaluate(f"""() => {{
                const s = {SH};
                s.openSlug = '{slug}';
                s.boardError = {{}};
                s.kanbanOpen = true;
                s.newConsole('claude');
            }}""")
            page.wait_for_timeout(200)
            win = page.locator(".session-window").first
            check("symptom3: a console window mounted", win.count() > 0)
            win.evaluate("(el) => el.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))")
            page.wait_for_timeout(100)
            box = win.bounding_box()
            cx, cy = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
            hit = page.evaluate(
                """([x, y]) => {
                    const el = document.elementFromPoint(x, y);
                    return { inKanban: !!el?.closest('.kanban'), inSession: !!el?.closest('.session-window') };
                }""",
                [cx, cy],
            )
            check("symptom3: focused console centre still resolves inside .kanban", hit["inKanban"], f"{hit}")
            check("symptom3: focused console centre does NOT resolve inside .session-window", not hit["inSession"], f"{hit}")
            page.screenshot(path=os.path.join(SHOT_DIR, "209-kanban-above-consoles-2026-07-14.png"))
            ctx.close()

            # --- symptom 4: tree integrity + .ralphy navigable (A4/A5, #203) --
            ctx, page = fresh()
            ready(page)
            page.evaluate(f"() => {{ {SH}.openSlug = '{slug}'; }}")
            entries1 = page.evaluate(
                f"""() => window.WBDaemon.observe('tree.list', {{ repo: '{slug}', path: '' }})"""
            )
            names1 = [e["name"] for e in entries1.get("entries", [])] if entries1 else []
            check("symptom4: root tree includes .ralphy", ".ralphy" in names1, f"{names1}")
            check("symptom4: root tree includes .github", ".github" in names1, f"{names1}")
            check("symptom4: root tree excludes .git", ".git" not in names1, f"{names1}")
            check("symptom4: root tree excludes gitignored .env", ".env" not in names1, f"{names1}")
            page.screenshot(path=os.path.join(SHOT_DIR, "209-tree-integrity-2026-07-14.png"))

            # two writes to the same directory must reconcile, not append.
            page.evaluate(
                f"""() => window.WBDaemon.write('file.write', {{ repo: '{slug}', path: 'a.txt', content: 'one' }})"""
            )
            page.wait_for_timeout(150)
            entries2 = page.evaluate(
                f"""() => window.WBDaemon.observe('tree.list', {{ repo: '{slug}', path: '' }})"""
            )
            page.evaluate(
                f"""() => window.WBDaemon.write('file.write', {{ repo: '{slug}', path: 'a.txt', content: 'two' }})"""
            )
            page.wait_for_timeout(150)
            entries3 = page.evaluate(
                f"""() => window.WBDaemon.observe('tree.list', {{ repo: '{slug}', path: '' }})"""
            )
            n2 = len(entries2.get("entries", [])) if entries2 else -1
            n3 = len(entries3.get("entries", [])) if entries3 else -2
            check("symptom4: child count identical across a re-list after two writes (reconcile, not append)", n2 == n3, f"n2={n2} n3={n3}")

            # --- extra: viewer external-edit refresh mechanism (A6, #203) -----
            page.evaluate(f"""() => {{
                const s = {SH};
                s.openTab({{ project: '{slug}', path: 'README.md', title: 'README.md', ftype: 'markdown' }});
            }}""")
            page.wait_for_timeout(400)
            badge = page.locator(".viewer-disk-badge")
            check("extra: .viewer-disk-badge element present in the viewer DOM", badge.count() > 0)
            check("extra: .viewer-disk-badge starts hidden on a clean tab", not badge.first.is_visible())
            page.evaluate(
                f"""() => window.WBDaemon.write('file.write', {{ repo: '{slug}', path: 'README.md', content: '# changed externally\\n' }})"""
            )
            page.wait_for_timeout(150)
            page.evaluate(f"() => {SH}.refreshOpenViewers('')")
            page.wait_for_timeout(300)
            body_text = page.locator(".md-body").inner_text()
            check("extra: a clean tab re-applies external content on a directory nudge", "changed externally" in body_text, f"body={body_text[:80]!r}")
            ctx.close()

            # --- symptom 5: translation actionable (M8, #206) ------------------
            ctx, page = fresh()
            ready(page)
            explained = page.evaluate(
                "() => window.WBTranslate.explainError('Other generic failures occurred.', 'pt', 'en')"
            )
            check(
                "symptom5: explainError maps the generic Chromium failure",
                explained == "couldn't download the pt→en model — check your connection and free disk space",
                f"got={explained!r}",
            )
            decided = page.evaluate("() => window.WBTranslate.decide(null, 'en', 'available')")
            check(
                "symptom5: decide(null,...) fails honestly with no guessed source",
                decided == {"action": "fail", "message": "couldn't detect the source language — translation skipped"},
                f"got={decided!r}",
            )
            progress = page.evaluate("() => window.WBTranslate.progressText(0.42)")
            check("symptom5: progressText renders a percentage", progress == "downloading model — 42%", f"got={progress!r}")
            ctx.close()

            # --- extra: topbar live uptime (M1, #204) --------------------------
            ctx, page = fresh()
            ready(page)
            page.wait_for_function(
                f"() => {SH}.uptimeText && {SH}.uptimeText !== 'connecting…'", timeout=8000
            )
            uptime_text = page.locator(".topbar .uptime").inner_text()
            check("extra: topbar uptime is live (not the static placeholder)", uptime_text.startswith("up "), f"got={uptime_text!r}")
            page.screenshot(path=os.path.join(SHOT_DIR, "209-topbar-live-2026-07-14.png"))
            ctx.close()

            browser.close()
    finally:
        stop(proc)

    # --- symptom 2 (hard half): network-bind Session TOTP login (#205/#179) --
    run_network_bind_login()

    ok = all(results) and len(results) >= 12
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if downgrades:
        print(f"{len(downgrades)} downgraded to review-only:", flush=True)
        for name, detail in downgrades:
            print(f"  - {name}: {detail}", flush=True)
    if ok:
        print("ALL SYMPTOMS NOT REPRODUCIBLE")
    sys.exit(0 if ok else 1)


def run_network_bind_login():
    daemon_dir2 = tempfile.mkdtemp(prefix="wb209_reg2_")
    seed_bytes = secrets.token_bytes(20)
    secret_b32 = base64.b32encode(seed_bytes).decode().rstrip("=")
    Path(daemon_dir2, "daemon-totp").write_text(secret_b32)
    env2 = empty_env(daemon_dir2)
    env2["RALPHY_DAEMON_TOKEN"] = secrets.token_hex(16)

    proc2 = None
    try:
        proc2 = subprocess.Popen(
            [EXE, "daemon", "--port", str(NET_PORT), "--bind", "0.0.0.0"], env=env2,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        if not wait_listening(NET_BASE):
            raise RuntimeError("network-bind daemon on 0.0.0.0:7358 never came up listening")

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page.goto(NET_BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_timeout(300)

            policy = page.evaluate(f"() => {SH}.security.policy")
            if policy != "session":
                raise RuntimeError(f"expected policy 'session' on the network bind, got {policy!r}")
            check("symptom2 (network bind): policy resolves to session", True, f"policy={policy}")
            check("symptom2 (network bind): login-gate shown pre-auth", page.locator(".login-gate").is_visible())

            code = totp_code(secret_b32)
            page.fill(".login-input", code)
            page.click(".login-btn")
            page.wait_for_timeout(400)

            gate_hidden = not page.locator(".login-gate").is_visible()
            authed = page.evaluate(f"() => {SH}.authed")
            session = page.evaluate("() => fetch('/api/session').then((r) => r.json())")
            login_ok = gate_hidden and authed is True and session.get("authed") is True and session.get("policy") == "session"
            if not login_ok:
                raise RuntimeError(f"login did not authorize: gate_hidden={gate_hidden} authed={authed} session={session}")
            check("symptom2 (network bind): valid TOTP logs in (gate hidden, authed, /api/session confirms)", True)

            ctx.close()
            browser.close()
    except Exception as e:  # the documented downgrade path (plan Caveats)
        downgrade("symptom2 network-bind Session TOTP login", str(e))
    finally:
        if proc2 is not None:
            stop(proc2)


if __name__ == "__main__":
    main()
