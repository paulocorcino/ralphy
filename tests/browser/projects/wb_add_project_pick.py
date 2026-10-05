"""Add a project: a double click and a touch on the folder list.

Scenario a  a double click on a folder opens that folder, not a folder of the
            list that the first click loaded
Scenario b  a double click on `..` goes up one folder, not two
Scenario c  a touch on a folder opens it and does not focus the field (on a
            phone, the focus opens the keyboard and iOS zooms the page)
Scenario d  a mouse click on a folder focuses the field

Boots a Localhost daemon on 7509 over a SCRATCH `RALPHY_DAEMON_DIR`. The
daemon is stopped by its own subprocess handle, NEVER by name.

Writes .ralphy/screenshots/add-project-pick.png.
Run: python tests/browser/projects/wb_add_project_pick.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wb_add_project_501 as base501  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7509
BASE = f"http://127.0.0.1:{PORT}/"
base501.PORT = PORT
DLG = base501.DLG
SHOT = os.path.join(base501.SHOT_DIR, "add-project-pick.png")

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def listed(page, folder):
    """Type `folder` and wait until the list shows its rows."""
    page.fill("#add-project-folder", folder)
    page.wait_for_function(
        f"(t) => {DLG}.addProject.listedText === t && document.querySelectorAll('.add-project-item').length > 0",
        arg=folder,
        timeout=10000,
    )


def row(name):
    return f".add-project-item:has(.add-project-name:text-is('{name}'))"


def settled(page):
    """Wait until the newest listing answers the text in the field."""
    page.wait_for_function(
        f"() => {{ const s = {DLG}.addProject; return s.listedText === s.text && !s.loading; }}",
        timeout=10000,
    )
    return page.input_value("#add-project-folder")


def main():
    os.makedirs(base501.SHOT_DIR, exist_ok=True)
    base501.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbpick_reg_")
    disk = Path(tempfile.mkdtemp(prefix="wbpick_disk_"))
    base501.seed(disk, "alpha")
    # A folder with the same name inside alpha: a second click that lands on
    # the reloaded list would open alpha/alpha.
    (disk / "alpha" / "alpha").mkdir()
    (disk / "beta").mkdir()
    sep = os.sep
    top = str(disk) + sep
    alpha = top + "alpha" + sep

    proc = base501.launch(daemon_dir)
    try:
        if not base501.wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            thrown = []

            # --- a, b, d: a mouse -------------------------------------------
            page = browser.new_context(viewport={"width": 1440, "height": 900}).new_page()
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            base501.open_dialog(page)

            listed(page, top)
            page.dblclick(row("alpha"))
            got = settled(page)
            check("a: a double click opens the folder", got == alpha, repr(got))
            check("a: …and the button names it", base501.wait_label(page, "Add project", True), str(base501.primary(page)))

            listed(page, alpha)
            page.dblclick(row(".."))
            got = settled(page)
            check("b: a double click on .. goes up one folder", got == top, repr(got))

            listed(page, top)
            page.evaluate("() => document.activeElement?.blur()")
            page.click(row("beta"))
            settled(page)
            focused = page.evaluate("() => document.activeElement?.id")
            check("d: a mouse click focuses the field", focused == "add-project-folder", repr(focused))

            # --- c: a touch -------------------------------------------------
            phone = browser.new_context(viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True).new_page()
            phone.on("pageerror", lambda e: thrown.append(str(e)))
            phone.goto(BASE)
            phone.wait_for_selector("[x-data]", timeout=8000)
            base501.open_dialog(phone)
            listed(phone, top)
            phone.evaluate("() => document.activeElement?.blur()")
            phone.tap(row("alpha"))
            got = settled(phone)
            check("c: a touch opens the folder", got == alpha, repr(got))
            focused = phone.evaluate("() => document.activeElement?.id || document.activeElement?.tagName")
            check("c: …and does not focus the field", focused != "add-project-folder", repr(focused))
            phone.screenshot(path=SHOT)
            print(f"[INFO] screenshot {SHOT}", flush=True)

            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        base501.stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite.
    check_floor = 8
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
