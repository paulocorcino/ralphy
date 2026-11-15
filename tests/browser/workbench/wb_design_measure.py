"""Measure the workbench design system in a real browser.

Boots a daemon on port 7493 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon is never touched, and stops it by its own handle,
never by name. Opens the workbench in four modes (desktop, 150% zoom,
tablet with touch, phone) and writes, per mode, the computed styles of each
component, the contrast of its text, the focus walk with Tab, the touch
target sizes, and a screenshot of each view.

Run from the repo root (it uses target/debug/ralphy.exe; build it first):
    python tests/browser/workbench/wb_design_measure.py [out-dir] [modes]
`out-dir` defaults to .ralphy/design-measure/<time>; `modes` is a comma
list (desktop,zoom150,tablet,phone). Compare two runs with
    python tests/browser/workbench/wb_design_measure_diff.py old.json new.json
The method and the numbers it produced are in docs/design-system/audit.md.
"""
import json, os, socket, subprocess, sys, tempfile, time
from pathlib import Path
from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")
REPO = str(Path(__file__).resolve().parents[3])
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(REPO, ".ralphy", "design-measure", time.strftime("%Y%m%d-%H%M%S"))
(OUT / "shots").mkdir(parents=True, exist_ok=True)
PORT = 7493
BASE = f"http://127.0.0.1:{PORT}/"
TARGET = os.path.join(REPO, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe")
CHILD = os.path.join(TARGET, "session_test_child.exe")
JS = (Path(__file__).parent / "wb_design_measure.js").read_text(encoding="utf-8")


def env(d):
    e = tempfile.mkdtemp(prefix="dsp_empty_")
    return dict(os.environ, RALPHY_DAEMON_DIR=d, RALPHY_DAEMON_AGENT_OVERRIDE=CHILD,
                RALPHY_USAGE_DIR=e, RALPHY_CLAUDE_PROJECTS_DIR=e, RALPHY_CODEX_DIR=e,
                RALPHY_OPENCODE_DB=os.path.join(e, "none.db"), RALPHY_KIMI_DIR=e,
                RALPHY_KIMI_CODE_DIR=e)


def fixture():
    d = tempfile.mkdtemp(prefix="dsp_fix_")
    (Path(d) / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (Path(d) / "README.md").write_text("# fixture\n\nSome text.\n", encoding="utf-8")
    (Path(d) / "src").mkdir()
    (Path(d) / "src" / "a.txt").write_text("a\n", encoding="utf-8")
    for a in (["git", "init", "-b", "main"], ["git", "config", "user.email", "d@e.x"],
              ["git", "config", "user.name", "d"], ["git", "add", "-A"],
              ["git", "commit", "-m", "f"]):
        subprocess.run(a, cwd=d, check=True, capture_output=True)
    return d


def listening():
    s = socket.socket(); s.settimeout(0.5)
    try:
        return s.connect_ex(("127.0.0.1", PORT)) == 0
    finally:
        s.close()


VIEWS = {
    "desktop": dict(viewport={"width": 1440, "height": 900}),
    "zoom150": dict(viewport={"width": 960, "height": 600}),
    "tablet": dict(viewport={"width": 1024, "height": 768}, has_touch=True),
    "phone": dict(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True,
                  device_scale_factor=3),
}


def settle(p, ms=400):
    p.wait_for_timeout(ms)


def run_view(b, name, opts, slug, data):
    ctx = b.new_context(**opts)
    p = ctx.new_page()
    errors = []
    p.on("pageerror", lambda e: errors.append(str(e)))
    p.goto(BASE)
    p.wait_for_function("() => window.Alpine && Alpine.store('connection').authed", timeout=20000)
    p.evaluate("() => { " + JS + " }")
    settle(p, 1200)
    out = {}
    shot = lambda tag: p.screenshot(path=str(OUT / "shots" / f"{name}-{tag}.png"))
    out["layout"] = p.evaluate("() => DSP.layout()")
    out["home"] = p.evaluate("() => DSP.all()")
    shot("home")
    p.evaluate("(s) => Alpine.store('projects').setOpen(s)", slug)
    settle(p, 3000)
    p.evaluate("(s) => Alpine.store('tabs').openTab({ project: s, path: 'README.md', title: 'README.md', ftype: 'markdown', content: '# fixture\\n\\nSome text.\\n' })", slug)
    settle(p, 1500)
    p.evaluate("() => Alpine.store('tabs').activate('consoles')")
    settle(p, 500)
    out["project"] = p.evaluate("() => DSP.all()")
    out["targets"] = p.evaluate("() => DSP.targets()")
    shot("project")
    if name == "desktop" or name == "phone":
        out["focus_page"] = DSP_tab(p, 80)
    # the Files search, for the small segmented control
    try:
        p.evaluate("() => window.dispatchEvent(new CustomEvent('workbench:file-search-open'))")
        settle(p, 800)
        out["files_search"] = p.evaluate("() => DSP.all('.files-search')")
        shot("files-search")
        p.keyboard.press("Escape")
    except Exception as e:
        out["files_search"] = {"crash": repr(e)}
    # menus
    p.evaluate("() => Alpine.$data(document.querySelector('[x-data=\"wbConsoleMenus\"]')).agentMenu = true")
    settle(p)
    out["menu"] = p.evaluate("() => DSP.all('.dropdown')")
    shot("menu")
    if name == "desktop":
        out["focus_menu"] = DSP_tab(p, 12, "() => document.querySelector('.console-menu .dropdown-item')?.focus()")
    p.keyboard.press("Escape")
    p.evaluate("() => Alpine.$data(document.querySelector('[x-data=\"wbConsoleMenus\"]')).agentMenu = false")
    # context menu on a tab
    tab = p.locator(".tab").nth(1)
    if tab.count() and tab.is_visible():
        tab.click(button="right")
        settle(p)
        out["ctxmenu"] = p.evaluate("() => DSP.all('#ctxmenu')")
        shot("ctxmenu")
        p.mouse.click(5, 5)
    # one console, to measure the session window
    try:
        p.evaluate("() => Alpine.$data(document.querySelector('[x-data=\"wbConsoleMenus\"]')).agentMenu = true")
        settle(p)
        p.locator('.console-menu .dropdown-item:not([disabled])').last.click()
        p.wait_for_selector('.session-window', timeout=15000)
        settle(p, 2000)
        out["console"] = p.evaluate("() => DSP.all('.session-window')")
        out["console_win"] = p.evaluate("() => DSP.win()")
        shot("console")
    except Exception as e:
        out["console"] = {"crash": repr(e)}
    # toast and flash
    p.evaluate("""() => { const el = document.createElement('div'); el.className = 'wb-toast';
      el.innerHTML = '<span class="wb-toast-text">Note deleted</span><button class="wb-toast-action" type="button">Undo</button><button class="wb-toast-close" type="button" title="Dismiss">×</button>';
      document.body.append(el); }""")
    settle(p, 200)
    out["toast"] = p.evaluate("() => DSP.all('.wb-toast')")
    shot("toast")
    p.evaluate("() => document.querySelector('.wb-toast')?.remove()")
    for ev, tag in [("workbench:settings-open", "settings"), ("workbench:security-open", "security"),
                    ("workbench:hosts-open", "hosts"), ("workbench:add-project-open", "addproject")]:
        p.evaluate("(e) => window.dispatchEvent(new CustomEvent(e))", ev)
        settle(p, 900)
        out["modal_" + tag] = p.evaluate("() => DSP.all('.modal-scrim:not([style*=\"display: none\"])')")
        shot(tag)
        if name in ("desktop", "phone"):
            out["focus_" + tag] = DSP_tab(p, 30)
        p.keyboard.press("Escape")
        settle(p, 500)
    # confirm (danger) and prompt
    p.evaluate("() => { Alpine.store('modals').askConfirm({ title: 'Delete a.txt?', message: 'The file goes to the trash.', confirmLabel: 'Delete', danger: true }); }")
    settle(p)
    out["modal_confirm"] = p.evaluate("() => DSP.all('.confirm-modal')")
    shot("confirm")
    out["focus_confirm"] = DSP_tab(p, 6)
    p.keyboard.press("Escape")
    settle(p)
    out["errors"] = errors
    data[name] = out
    ctx.close()


def DSP_tab(p, n, start=None):
    if start:
        p.evaluate(start)
    else:
        p.evaluate("() => document.activeElement?.blur?.()")
    p.evaluate("() => DSP.focusStart()")
    for _ in range(n):
        p.keyboard.press("Tab")
        p.evaluate("() => DSP.focusRecord()")
    return p.evaluate("() => DSP.focusReport()")


def main():
    if listening():
        print("port busy"); sys.exit(1)
    d = tempfile.mkdtemp(prefix="dsp_store_")
    fx = fixture()
    r = subprocess.run([EXE, "daemon", "add", fx], env=dict(os.environ, RALPHY_DAEMON_DIR=d),
                       check=True, capture_output=True, encoding="utf-8")
    slug = r.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()
    proc = subprocess.Popen([EXE, "daemon", "--port", str(PORT)], env=env(d),
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    data = {}
    try:
        for _ in range(60):
            if listening():
                break
            time.sleep(0.5)
        with sync_playwright() as pw:
            b = pw.chromium.launch()
            for name, opts in VIEWS.items():
                if len(sys.argv) > 2 and name not in sys.argv[2].split(","):
                    continue
                try:
                    run_view(b, name, opts, slug, data)
                except Exception as e:
                    data[name] = {"crash": repr(e)}
                print("view", name, "done", flush=True)
            b.close()
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
    (OUT / "measure.json").write_text(json.dumps(data, indent=1), encoding="utf-8")
    print("wrote", OUT / "measure.json")


main()
