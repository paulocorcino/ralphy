"""Browser check: on a phone, a toolbar menu stays inside the canvas.

The Consoles, Fence and Note menus open under their toolbar buttons. The
canvas clips what leaves it, so a menu that runs past the canvas's left edge
is cut at the rail. On two phone widths, each menu must lie between the rail
and the right edge of the viewport, and each of its rows must be whole.

On a desktop viewport the Consoles menu still hangs from its own button.

A real daemon on a scratch `RALPHY_DAEMON_DIR`, stopped by its own subprocess
handle, never by name.

Writes .ralphy/screenshots/toolbar-menu-phone-<width>.png.
Run: python tests/browser/workbench/wb_toolbar_menu_phone.py   (exit 0 = all pass)
"""

import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7752
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")

PHONES = [{"width": 390, "height": 844}, {"width": 430, "height": 932}]
DESKTOP = {"width": 1280, "height": 800}
MENUS = [("Consoles", ".console-menu"), ("Fence", ".fence-menu"), ("Note", ".note-menu")]

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


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    # The UI assets are embedded in the binary: an old build shows old styles.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wbmenu_empty_")
    env = dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )


# The box of the open menu, the box of each row, and the canvas's left edge.
MEASURE = """
(sel) => {
  const menu = document.querySelector(sel);
  if (!menu || menu.offsetParent === null) return null;
  const box = (el) => { const r = el.getBoundingClientRect(); return { left: r.left, right: r.right }; };
  const rows = [...menu.querySelectorAll('button, .dropdown-head')].filter((el) => el.offsetParent !== null);
  return {
    menu: box(menu),
    rows: rows.map(box),
    canvasLeft: document.querySelector('.canvas').getBoundingClientRect().left,
    viewport: document.documentElement.clientWidth,
    button: box(menu.parentElement.querySelector('.btn')),
  };
}
"""


def open_menu(page, label):
    page.locator(f".canvas-tools .btn[aria-label='{label}']").click()
    page.wait_for_timeout(250)


def close_menus(page):
    page.keyboard.press("Escape")
    page.wait_for_timeout(150)


def check_phone(browser, viewport):
    width = viewport["width"]
    ctx = browser.new_context(viewport=viewport, has_touch=True, is_mobile=True)
    page = ctx.new_page()
    page.goto(BASE, wait_until="domcontentloaded")
    page.wait_for_timeout(1200)
    for label, selector in MENUS:
        open_menu(page, label)
        m = page.evaluate(MEASURE, selector)
        if m is None:
            check(f"{width}: the {label} menu opens", False)
            continue
        if label == "Consoles":
            page.screenshot(path=os.path.join(SHOT_DIR, f"toolbar-menu-phone-{width}.png"))
        check(
            f"{width}: the {label} menu starts right of the canvas's left edge",
            m["menu"]["left"] >= m["canvasLeft"],
            f"menu left {m['menu']['left']:.1f}, canvas left {m['canvasLeft']:.1f}",
        )
        check(
            f"{width}: the {label} menu ends inside the viewport",
            m["menu"]["right"] <= m["viewport"],
            f"menu right {m['menu']['right']:.1f}, viewport {m['viewport']}",
        )
        inside = all(r["left"] >= m["menu"]["left"] and r["right"] <= m["menu"]["right"] for r in m["rows"])
        check(f"{width}: every row of the {label} menu is inside the menu", inside and len(m["rows"]) > 0)
        close_menus(page)
    ctx.close()


def check_desktop(browser):
    ctx = browser.new_context(viewport=DESKTOP)
    page = ctx.new_page()
    page.goto(BASE, wait_until="domcontentloaded")
    page.wait_for_timeout(1200)
    open_menu(page, "Consoles")
    m = page.evaluate(MEASURE, ".console-menu")
    check(
        "desktop: the Consoles menu ends at the right edge of its own button",
        m is not None and abs(m["menu"]["right"] - m["button"]["right"]) < 1,
        "" if m is None else f"menu right {m['menu']['right']:.1f}, button right {m['button']['right']:.1f}",
    )
    ctx.close()


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    if port_already_listening(PORT):
        check(f"port {PORT} free before launch", False, "a listener is already bound")
        sys.exit(1)
    build()
    proc = launch(tempfile.mkdtemp(prefix="wbmenu_reg_"))
    try:
        if not wait_listening(BASE):
            check("daemon listening", False, BASE)
            sys.exit(1)
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            for viewport in PHONES:
                check_phone(browser, viewport)
            check_desktop(browser)
            browser.close()
    finally:
        stop(proc)
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
