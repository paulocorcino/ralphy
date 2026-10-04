"""Settings → Desk history: upload, restore, download, and a page that missed it.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so
the operator's own desk is untouched. The windows are agent placeholders (no
session), so no child is spawned.

Scenario   desk A (one window at left 100) is open on pages 1, 2 and 3.
           Page 3 stops handling pushes, like a phone that sleeps.
           1. Page 1 uploads a file with desk B (one window at left 600 and a
              fence). Pages 1 and 2 reload by themselves and show B.
           2. The list shows the versions: Uploaded, Before a restore, Changed.
           3. Page 1 downloads a version: the file is a desk version.
           4. Page 1 restores "Before a restore". Pages 1 and 2 show A again.
              Page 3 missed both restores: it never reloaded, and it still
              holds the generation from before the upload.
           5. Page 3 drags its window 60 px. It reloads instead of writing,
              and the window on the daemon is still at left 100.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes .ralphy/screenshots/desk-history-2026-10-04.png.
Run: python tests/browser/desk/wb_desk_history.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile

from playwright.sync_api import sync_playwright

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wb_desk_double_launch as base  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

SHOT = "desk-history-2026-10-04.png"
MARK = "() => { window.__beforeReload = true; }"
RELOADED = "() => window.__beforeReload === undefined"


def desk():
    _, body = base.http("GET", "api/desk")
    return json.loads(body)


def history():
    _, body = base.http("GET", "api/desk/history")
    return json.loads(body)


def shown(page):
    return page.evaluate(
        """() => [...document.querySelectorAll('.session-window')]
            .map((w) => [w._deskId, parseFloat(w.style.left)])"""
    )


def open_page(browser):
    ctx = browser.new_context(viewport={"width": 1400, "height": 900}, accept_downloads=True)
    page = ctx.new_page()
    page.goto(base.BASE)
    base.open_consoles_tab(page)
    page.wait_for_timeout(2500)
    page.evaluate(MARK)
    return page


def open_history(page):
    page.locator('button[title="Settings"]').click()
    page.locator(".settings-navitem", has_text="Desk history").click()
    page.wait_for_selector(".desk-history-row", timeout=5000)


def confirm_restore(page):
    page.locator(".confirm-modal button", has_text="Restore").click()


def wait_reloaded(page):
    page.wait_for_function(RELOADED, timeout=10000)
    page.wait_for_timeout(2500)
    page.evaluate(MARK)


def main():
    os.makedirs(base.SHOT_DIR, exist_ok=True)
    base.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbhistory_reg_")
    slug = base.register_fixture(daemon_dir, base.make_fixture_repo())
    proc = subprocess.Popen(
        [base.EXE, "daemon", "--port", str(base.PORT)],
        env=base.daemon_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        if not base.wait_listening(base.BASE):
            base.check("the daemon listens", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            a = base.record("w-a", slug, "agent", "claude", 100.0, 1)
            status, _ = base.put_desk([a])
            base.check("desk A is stored", status == 200)
            base.check(
                "the first change is a version",
                [v["reason"] for v in history()] == ["change"],
                f"got={history()}",
            )

            p1, p2, p3 = open_page(browser), open_page(browser), open_page(browser)
            for n, page in ((1, p1), (2, p2), (3, p3)):
                base.check(f"page {n} shows desk A", shown(page) == [["w-a", 100.0]], f"got={shown(page)}")
            p3.evaluate(f"() => {{ {base.SH}.onPresencePush = () => {{}}; }}")

            # 1. Upload desk B from page 1.
            b = base.record("w-b", slug, "agent", "claude", 600.0, 1)
            fence = {
                "id": "f-b",
                "name": "backend",
                "rect": {"left": 560.0, "top": 10.0, "width": 520.0, "height": 340.0},
                "ts": 1,
            }
            file_b = {
                "kind": "ralphy-desk-version",
                "id": 1,
                "startedAt": 1,
                "savedAt": 1,
                "reason": "change",
                "desk": {"windows": [b], "fences": [fence]},
            }
            path_b = os.path.join(daemon_dir, "desk-b.json")
            with open(path_b, "w", encoding="utf-8") as fh:
                json.dump(file_b, fh)
            open_history(p1)
            p1.set_input_files("#desk-upload-input", path_b)
            confirm_restore(p1)
            wait_reloaded(p1)
            wait_reloaded(p2)
            for n, page in ((1, p1), (2, p2)):
                base.check(f"page {n} shows desk B", shown(page) == [["w-b", 600.0]], f"got={shown(page)}")
                base.check(f"page {n} shows the fence", page.locator(".fence").count() == 1)
            base.check("the desk has a generation", desk().get("generation", 0) > 0)

            # 2. The list.
            open_history(p1)
            helps = p1.locator(".desk-history-row .set-help").all_inner_texts()
            base.check(
                "the list shows the versions, newest first",
                [h.split(" · ")[0] for h in helps] == ["Uploaded", "Before a restore", "Changed"],
                f"got={helps}",
            )

            # 3. Download the newest version.
            with p1.expect_download() as dl:
                p1.locator(".desk-history-row").first.locator("button", has_text="Download").click()
            saved = json.loads(open(dl.value.path(), encoding="utf-8").read())
            base.check(
                "the download is a desk version",
                saved.get("kind") == "ralphy-desk-version"
                and [w["id"] for w in saved["desk"]["windows"]] == ["w-b"],
                f"got={dl.value.suggested_filename}",
            )

            # 4. Restore "Before a restore".
            p1.locator(".desk-history-row", has_text="Before a restore").locator(
                "button", has_text="Restore"
            ).click()
            confirm_restore(p1)
            wait_reloaded(p1)
            wait_reloaded(p2)
            for n, page in ((1, p1), (2, p2)):
                base.check(f"page {n} shows desk A again", shown(page) == [["w-a", 100.0]], f"got={shown(page)}")
            base.check("page 3 missed the push", p3.evaluate("() => window.__beforeReload === true"))
            base.check("page 3 still shows the desk it loaded", shown(p3) == [["w-a", 100.0]], f"got={shown(p3)}")

            # 5. Page 3 drags: it reloads, and writes nothing.
            p3.evaluate("() => { const w = document.querySelector('.session-window'); w.scrollIntoView(); }")
            box = p3.locator(".session-window .session-titlebar").first.bounding_box()
            p3.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            p3.mouse.down()
            p3.mouse.move(box["x"] + box["width"] / 2 + 60, box["y"] + box["height"] / 2 + 40, steps=4)
            p3.mouse.up()
            wait_reloaded(p3)
            ids = [(w["id"], w["rect"]["left"]) for w in desk()["windows"]]
            base.check("the drag on page 3 was not written", ids == [("w-a", 100.0)], f"got={ids}")
            base.check("page 3 now shows desk A", shown(p3) == [["w-a", 100.0]], f"got={shown(p3)}")

            open_history(p1)
            p1.screenshot(path=os.path.join(base.SHOT_DIR, SHOT))
            browser.close()
    finally:
        base.stop(proc)

    passed = sum(base.results)
    print(f"\n{passed}/{len(base.results)} checks passed")
    sys.exit(0 if base.results and all(base.results) else 1)


if __name__ == "__main__":
    main()
