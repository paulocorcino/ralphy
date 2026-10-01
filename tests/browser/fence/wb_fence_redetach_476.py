"""A quick re-detach keeps its popup (issue #476) browser acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7483.

The fixture is the one of `wb_note_detach_draft.py`: a console `w-a` inside the
fence `f-det`.

R1 a re-attach and a detach in the same task: the second popup stays open,
   the fence stays detached, and the console is in the popup
R2 a `popup-gone`, a `popup-members` and a `popup-here` from an earlier popup
   of the fence change nothing: no re-attach, and the console's desk record stays
R4 an `origin-close` for an earlier popup does not close the current one
R5 the current popup's `popup-members` is still heard: a console closed there
   leaves the desk
R3 after the opener reloads, it adopts the popup's identity: a stale
   `popup-gone` is still ignored, and the popup's own close re-attaches

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: cargo build -p ralphy-cli --bin ralphy
     python tests/browser/fence/wb_fence_redetach_476.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile
import time
import traceback

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "notes"))
import wb_note_detach_draft as D  # noqa: E402  the fixture and page helpers
from playwright.sync_api import sync_playwright  # noqa: E402

T = D.T
PORT = 7483
T.PORT = PORT
T.BASE = D.BASE = BASE = f"http://127.0.0.1:{PORT}/"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


# A message on the lifecycle channel, as another document of this tab would
# send it. The channel does not deliver to the object that posts, but it does
# deliver to the shell's own channel object in the same page.
SEND = """(m) => { const tab = JSON.parse(sessionStorage.getItem('wb.detach.v1') || '{}').tab;
  const c = new BroadcastChannel('wb.detach.v1'); c.postMessage({ tab, ...m }); c.close(); }"""


def detached(page):
    return page.evaluate("() => WBConsole.isDetached('f-det')")


def w_a_on_desk():
    return any(w.get("id") == "w-a" for w in T.desk_raw().get("windows", []))


def main():
    d = tempfile.mkdtemp(prefix="wbredetach_")
    fx = T.make_fixture_repo()
    slug = T.register_fixture(d, fx)
    D.write_desk(d, slug)
    proc = T.launch(d)
    try:
        if not T.wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            b = p.chromium.launch()
            ctx = b.new_context(viewport=dict(D.VIEW))
            page = ctx.new_page()
            page.goto(BASE)
            D.boot(page)

            # R1
            first = D.detach(ctx, page, "f-det")
            first.wait_for_selector("#stage .session-window", timeout=15000)
            page.wait_for_timeout(1500)
            with ctx.expect_page(timeout=10000) as info:
                page.evaluate("() => { WBConsole.reattachFence('f-det'); WBConsole.detachFence('f-det'); }")
            second = info.value
            try:
                second.wait_for_selector("#stage .session-window", timeout=15000)
            except Exception:
                pass  # a popup the bug closed; the checks below say so
            page.wait_for_timeout(4000)
            check("R1 the first popup closed", first.is_closed())
            check("R1 the second popup is still open", not second.is_closed())
            check("R1 the fence is still detached", detached(page))
            if second.is_closed():
                check("R1 cannot continue: the second popup closed", False)
                return
            check(
                "R1 the console is in the second popup, not on the stage",
                not second.is_closed()
                and second.locator("#stage .session-window").count() == 1
                and page.locator("#stage .session-window").count() == 0,
            )

            # R2
            page.evaluate(SEND, {"type": "popup-members", "fenceId": "f-det", "pid": "stale", "members": []})
            page.evaluate(SEND, {"type": "popup-here", "fenceId": "f-det", "pid": "stale", "members": []})
            page.evaluate(SEND, {"type": "popup-gone", "fenceId": "f-det", "pid": "stale"})
            page.wait_for_timeout(500)
            check("R2 a stale popup-gone does not re-attach", detached(page) and not second.is_closed())
            check(
                "R2 a stale popup-members or popup-here keeps the console's record in this tab",
                page.evaluate("() => WBConsole.deskRecords().some((r) => r.id === 'w-a')"),
            )
            deadline = time.time() + 4
            while time.time() < deadline and w_a_on_desk():
                time.sleep(0.3)
            check("R2 ... and on the daemon's desk", w_a_on_desk())

            # R4
            page.evaluate(SEND, {"type": "origin-close", "fenceId": "f-det", "pid": "stale"})
            page.wait_for_timeout(1000)
            check("R4 a stale origin-close leaves the popup open", not second.is_closed())

            # R5: the current popup's own `popup-members` is heard. Closing
            # the console there removes its desk record.
            second.locator("#stage .session-window .session-close").first.click()
            T.confirm(second)
            deadline = time.time() + 8
            while time.time() < deadline and w_a_on_desk():
                time.sleep(0.3)
            check("R5 a console closed in the current popup leaves the desk", not w_a_on_desk())

            # R3
            D.reload_opener(page, "f-det")
            check("R3 the reloaded opener still holds the popup", detached(page) and not second.is_closed())
            page.evaluate(SEND, {"type": "popup-gone", "fenceId": "f-det", "pid": "stale"})
            page.wait_for_timeout(1500)
            check("R3 a stale popup-gone is ignored after the adoption", detached(page))
            second.close()
            deadline = time.time() + 4
            while time.time() < deadline and detached(page):
                time.sleep(0.2)
            check("R3 the popup's own close re-attaches the fence", not detached(page))
            b.close()
    except BaseException:
        # A crash is a failure, and its traceback is printed: the exit below
        # would otherwise replace it with the verdict of the checks so far.
        traceback.print_exc()
        results.append(False)
    finally:
        T.stop(proc)
        # In `finally`, so an early return still reports and still fails.
        passed = sum(results)
        print(f"\n{passed}/{len(results)} passed")
        sys.exit(0 if results and all(results) else 1)


if __name__ == "__main__":
    main()
