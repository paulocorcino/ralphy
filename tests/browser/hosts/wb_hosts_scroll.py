"""Browser check: the Hosts dialog with many hosts.

One Playwright pass over a REAL daemon on a SCRATCH `RALPHY_DAEMON_DIR`. The
list of hosts is replaced in the page with 20 fake rows, because only the
layout is under test here (wb_hosts_497.py covers the real verbs).

Scenario 1  the dialog stays inside the window, the list scrolls, and the tabs
            and the Close button stay in place when it scrolls
Scenario 2  at the top only the bottom shadow is painted; at the bottom only
            the top shadow (pixels just inside each edge of the list)
Scenario 3  Remove on the last row scrolls its question into view
Scenario 4  with 3 hosts the list does not scroll
Scenario 5  no page errors were thrown

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
is also the orchestrator on this host).

Needs `cargo build -p ralphy-cli --bin ralphy` first (the UI is embedded).
Writes .ralphy/screenshots/2026-10-02-hosts-scroll-*.png.
Run: python tests/browser/hosts/wb_hosts_scroll.py   (exit 0 = all pass)
"""

import os
import subprocess
import sys
import tempfile
import time
import urllib.request

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7498
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXT = ".exe" if os.name == "nt" else ""
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy" + EXT)
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")

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


def shot(page, name):
    os.makedirs(SHOT_DIR, exist_ok=True)
    page.screenshot(path=os.path.join(SHOT_DIR, f"2026-10-02-hosts-scroll-{name}.png"))


# Replaces the peers with `n` tunnel peers and opens the dialog on its list.
OPEN_WITH = """(n) => {
  const app = Alpine.$data(document.querySelector('.hosts-dialog'));
  const oses = ["Linux", "macOS", "Windows"];
  const hosts = Array.from({ length: n }, (_, i) => ({
    daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5F" + String(i).padStart(2, "0"),
    name: "vps-ralphy" + (i + 1),
    environment: oses[i % 3],
    destination: "ralphy" + (i + 1) + "@10.1.1." + (i + 4),
    state: i % 4 === 3 ? "unreachable" : "reachable",
    tunnel: {},
  }));
  app.sshHosts = () => hosts;
  app.addHost.open = false;
  window.dispatchEvent(new CustomEvent('workbench:hosts-open'));
}"""

LIST_GEOMETRY = """() => {
  const list = document.querySelector('[role=dialog][aria-label="Hosts"] .host-list');
  const modal = document.querySelector('[role=dialog][aria-label="Hosts"]');
  const tabs = document.querySelector('[role=dialog][aria-label="Hosts"] .host-tabs').getBoundingClientRect();
  const close = document.querySelector('[role=dialog][aria-label="Hosts"] .modal-foot').getBoundingClientRect();
  const m = modal.getBoundingClientRect();
  return {
    scrollHeight: list.scrollHeight, clientHeight: list.clientHeight, scrollTop: list.scrollTop,
    modalTop: m.top, modalBottom: m.bottom, winH: window.innerHeight,
    tabsTop: tabs.top, closeTop: close.top,
  };
}"""


def main():
    daemon_dir = tempfile.mkdtemp(prefix="wbscroll_")
    subprocess.run(
        [EXE, "daemon", "setup", "--name", "local", "--avatar", "1"],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        check=True,
        capture_output=True,
    )
    empty = tempfile.mkdtemp(prefix="wbscroll_empty_")
    proc = subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=dict(
            os.environ,
            RALPHY_DAEMON_DIR=daemon_dir,
            RALPHY_USAGE_DIR=empty,
            RALPHY_CLAUDE_PROJECTS_DIR=empty,
            RALPHY_CODEX_DIR=empty,
            RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
            RALPHY_KIMI_DIR=empty,
            RALPHY_KIMI_CODE_DIR=empty,
        ),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    errors = []
    try:
        if not wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_page(viewport={"width": 1100, "height": 700})
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            page.wait_for_function("() => window.Alpine && Alpine.$data(document.body).authed")

            # Scenario 1
            page.evaluate(OPEN_WITH, 20)
            page.wait_for_selector("[role=dialog][aria-label=Hosts] .host-item >> nth=19")
            g0 = page.evaluate(LIST_GEOMETRY)
            check("dialog inside the window", g0["modalTop"] >= 0 and g0["modalBottom"] <= g0["winH"], str(g0))
            check("the list scrolls", g0["scrollHeight"] > g0["clientHeight"] + 50, str(g0))
            shot(page, "top")
            page.eval_on_selector("[role=dialog][aria-label=Hosts] .host-list", "l => { l.scrollTop = l.scrollHeight; }")
            page.wait_for_timeout(100)
            g1 = page.evaluate(LIST_GEOMETRY)
            check("tabs stay in place", abs(g1["tabsTop"] - g0["tabsTop"]) < 0.5, f"{g0['tabsTop']} -> {g1['tabsTop']}")
            check("Close stays in place", abs(g1["closeTop"] - g0["closeTop"]) < 0.5, f"{g0['closeTop']} -> {g1['closeTop']}")
            shot(page, "bottom")

            # Scenario 2: luminance of one pixel row just inside each edge,
            # between two rows' left edge and the name (empty background).
            def edge_luma(at_top):
                box = page.eval_on_selector(
                    "[role=dialog][aria-label=Hosts] .host-list",
                    "l => { const r = l.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; }",
                )
                x = box[0] + box[2] * 0.5
                y = box[1] + 2 if at_top else box[1] + box[3] - 3
                png = page.screenshot(clip={"x": x, "y": y, "width": 1, "height": 1})
                from io import BytesIO

                from PIL import Image

                r, gg, b = Image.open(BytesIO(png)).convert("RGB").getpixel((0, 0))
                return 0.299 * r + 0.587 * gg + 0.114 * b

            page.eval_on_selector("[role=dialog][aria-label=Hosts] .host-list", "l => { l.scrollTop = 0; }")
            page.wait_for_timeout(100)
            top_at_top, bottom_at_top = edge_luma(True), edge_luma(False)
            page.eval_on_selector("[role=dialog][aria-label=Hosts] .host-list", "l => { l.scrollTop = l.scrollHeight; }")
            page.wait_for_timeout(100)
            top_at_bottom, bottom_at_bottom = edge_luma(True), edge_luma(False)
            check(
                "shadow only where rows are hidden",
                bottom_at_top < top_at_top - 3 and top_at_bottom < bottom_at_bottom - 3,
                f"at top: top={top_at_top:.1f} bottom={bottom_at_top:.1f}; "
                f"at bottom: top={top_at_bottom:.1f} bottom={bottom_at_bottom:.1f}",
            )

            # Scenario 3
            page.eval_on_selector("[role=dialog][aria-label=Hosts] .host-list", "l => { l.scrollTop = 0; }")
            page.wait_for_timeout(100)
            page.locator("[role=dialog][aria-label=Hosts] .host-item").nth(19).get_by_role("button", name="Remove").click()
            page.wait_for_timeout(400)
            inside = page.evaluate(
                """() => {
                  const l = document.querySelector('[role=dialog][aria-label="Hosts"] .host-list').getBoundingClientRect();
                  const q = [...document.querySelectorAll('[role=dialog][aria-label="Hosts"] .host-remove')]
                    .find((e) => e.offsetParent).querySelector('.btn.danger').getBoundingClientRect();
                  return q.top >= l.top && q.bottom <= l.bottom + 0.5;
                }"""
            )
            check("Remove question scrolled into view", inside)
            shot(page, "remove")

            # Scenario 4
            page.evaluate(OPEN_WITH, 3)
            page.wait_for_timeout(200)
            g3 = page.evaluate(LIST_GEOMETRY)
            check("3 hosts: no scrolling", g3["scrollHeight"] <= g3["clientHeight"], str(g3))
            shot(page, "three")

            browser.close()
    finally:
        proc.terminate()
        proc.wait(timeout=10)
    # Scenario 5
    check("no page errors", not errors, "; ".join(errors))


if __name__ == "__main__":
    main()
    sys.exit(0 if results and all(results) else 1)
