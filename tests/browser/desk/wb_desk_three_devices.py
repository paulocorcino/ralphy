"""Three devices on one desk: a change made on one is never undone by another.

ADR-0050 amendment 2026-10-04 (the desk upload carries changes, not the desk).
Each scenario runs a REAL daemon on a fresh scratch `RALPHY_DAEMON_DIR` and
opens one browser context per device, so the operator's own desk is
untouched. A device "goes stale" by going offline: it keeps acting, its
changes queue, and they are sent when it comes back.

Scenario 1   the measured loss: the PC moves a window, the phone's socket
             reconnects. The move stays, and the phone shows it.
Scenario 2   the phone renames a window while offline, after the PC moved it.
             Both the move and the name stay.
Scenario 3   the PC closes a placeholder; the phone, offline, drags it. The
             record does not come back, and the phone's window goes.
Scenario 4   the tablet selects a worktree; the phone, offline, drags a window.
             The tablet's selection stays.
Scenario 5   the PC moves a fence; the phone, offline, renames it. Both stay.
Scenario 6   a drag on the phone reaches the tablet live, but not the PC's
             window that is under the operator's hand; a maximize does not
             reach the other devices.
Scenario 7   the daemon applies the phone's batch and the reply is lost; the PC
             moves the window; the phone resends. The PC's move stays.

The daemon is stopped by its own subprocess handle, NEVER by name.
Run: python tests/browser/desk/wb_desk_three_devices.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile
import urllib.request

from playwright.sync_api import sync_playwright

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wb_desk_double_launch as base  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7433
BASE = f"http://127.0.0.1:{PORT}/"
SH = base.SH

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def http(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"} if body is not None else {}
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=5) as r:
        return r.status, r.read().decode()


def put_changes(changes):
    return http("PUT", "api/desk", {"seq": 1, "changes": changes})


def desk():
    return json.loads(http("GET", "api/desk")[1])


def window_record(rid, slug, kind="console", agent="console", left=100.0):
    return {
        "id": rid,
        "repo": slug,
        "agent": agent,
        "kind": kind,
        "rect": {"left": left, "top": 60.0, "width": 420.0, "height": 260.0},
        "max": False,
        "sessionId": None,
    }


def record_of(rid):
    return next((w for w in desk()["windows"] if w["id"] == rid), None)


class Daemon:
    def __init__(self):
        self.dir = tempfile.mkdtemp(prefix="wbthree_")
        self.slug = base.register_fixture(self.dir, base.make_fixture_repo())
        self.proc = subprocess.Popen(
            [base.EXE, "daemon", "--port", str(PORT)],
            env=base.daemon_env(self.dir),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    def __enter__(self):
        if not base.wait_listening(BASE):
            raise RuntimeError("the scratch daemon did not listen")
        return self

    def __exit__(self, *exc):
        base.stop(self.proc)


def device(browser, width=1400, height=900):
    ctx = browser.new_context(viewport={"width": width, "height": height})
    page = ctx.new_page()
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
    return ctx, page


WIN = "(id) => [...document.querySelectorAll('.session-window')].find((w) => w._deskId === id)"


def left_of(page, rid):
    return page.evaluate(f"(id) => {{ const w = ({WIN})(id); return w ? parseFloat(w.style.left) : null; }}", rid)


def has_window(page, rid):
    return page.evaluate(f"(id) => !!({WIN})(id)", rid)


def box_in(page, rid, selector):
    return page.evaluate(
        f"""([id, sel]) => {{
            const w = ({WIN})(id);
            const r = w && w.querySelector(sel).getBoundingClientRect();
            return r && {{ x: r.x, y: r.y, width: r.width, height: r.height }};
        }}""",
        [rid, selector],
    )


def press(page, box, dx=40):
    page.mouse.move(box["x"] + dx, box["y"] + box["height"] / 2)
    page.mouse.down()


def drag_by(page, box, dx, dy=0, release=True):
    press(page, box)
    page.mouse.move(box["x"] + 40 + dx, box["y"] + box["height"] / 2 + dy, steps=8)
    if release:
        page.mouse.up()


def drag_window(page, rid, dx, dy=0, release=True):
    drag_by(page, box_in(page, rid, ".session-titlebar"), dx, dy, release)


def near(a, b, tol=2.0):
    return a is not None and b is not None and abs(a - b) <= tol


def scenario_1(browser):
    print("\n--- scenario 1: a reconnect does not undo a move ---")
    with Daemon() as d:
        put_changes([{"op": "create", "type": "window", "record": window_record("w-1", d.slug)}])
        phone_ctx, phone = device(browser)
        pc_ctx, pc = device(browser)
        pc.wait_for_timeout(4000)
        drag_window(pc, "w-1", 300)
        pc.wait_for_timeout(1500)
        moved = left_of(pc, "w-1")
        check("the PC's move reaches the daemon", near(record_of("w-1")["rect"]["left"], moved), f"pc={moved}")
        check("the phone shows the move without a reload", near(left_of(phone, "w-1"), moved), f"phone={left_of(phone, 'w-1')}")
        phone.evaluate(f"() => ({WIN})('w-1')._term.ws.close()")
        phone.wait_for_timeout(5000)
        check(
            "after the phone reconnects the move stays",
            near(record_of("w-1")["rect"]["left"], moved),
            f"daemon={record_of('w-1')['rect']['left']} moved={moved}",
        )
        phone_ctx.close()
        pc_ctx.close()


def scenario_2(browser):
    print("\n--- scenario 2: an offline rename keeps another device's move ---")
    with Daemon() as d:
        put_changes([{"op": "create", "type": "window", "record": window_record("w-1", d.slug)}])
        phone_ctx, phone = device(browser)
        pc_ctx, pc = device(browser)
        pc.wait_for_timeout(4000)
        phone_ctx.set_offline(True)
        drag_window(pc, "w-1", 300)
        pc.wait_for_timeout(1500)
        moved = left_of(pc, "w-1")
        check("the phone did not see the move", near(left_of(phone, "w-1"), 100.0), f"phone={left_of(phone, 'w-1')}")
        name = box_in(phone, "w-1", ".session-name")
        phone.mouse.dblclick(name["x"] + 5, name["y"] + name["height"] / 2)
        phone.keyboard.press("Control+A")
        phone.keyboard.type("phone-name")
        phone.keyboard.press("Enter")
        phone.wait_for_timeout(800)
        phone_ctx.set_offline(False)
        phone.wait_for_timeout(3000)
        rec = record_of("w-1")
        check("the PC's move stays", near(rec["rect"]["left"], moved), f"daemon={rec['rect']['left']} moved={moved}")
        check("the phone's name is stored", rec.get("consoleName") == "phone-name", f"got={rec.get('consoleName')}")
        check("the phone shows the move now", near(left_of(phone, "w-1"), moved), f"phone={left_of(phone, 'w-1')}")
        pc.wait_for_timeout(1000)
        pc_name = pc.evaluate(f"() => ({WIN})('w-1')._deskConsoleName")
        check("the PC shows the name without a reload", pc_name == "phone-name", f"got={pc_name}")
        phone_ctx.close()
        pc_ctx.close()


def scenario_3(browser):
    print("\n--- scenario 3: a closed placeholder does not come back ---")
    with Daemon() as d:
        put_changes(
            [{"op": "create", "type": "window", "record": window_record("w-ph", d.slug, "agent", "claude")}]
        )
        phone_ctx, phone = device(browser)
        pc_ctx, pc = device(browser)
        pc.wait_for_timeout(4000)
        check("both devices show the placeholder", has_window(pc, "w-ph") and has_window(phone, "w-ph"))
        phone_ctx.set_offline(True)
        close = box_in(pc, "w-ph", ".session-close")
        pc.mouse.click(close["x"] + close["width"] / 2, close["y"] + close["height"] / 2)
        pc.locator(".wb-confirm .btn.danger").click()
        pc.wait_for_timeout(1500)
        check("the PC's close removes the record", record_of("w-ph") is None)
        drag_window(phone, "w-ph", 200)
        phone.wait_for_timeout(500)
        phone_ctx.set_offline(False)
        phone.wait_for_timeout(3000)
        check("the phone's drag does not bring the record back", record_of("w-ph") is None, json.dumps(desk()["windows"]))
        check("the placeholder leaves the phone", not has_window(phone, "w-ph"))
        phone_ctx.close()
        pc_ctx.close()


def scenario_4(browser):
    print("\n--- scenario 4: a worktree selection survives another device's writes ---")
    with Daemon() as d:
        put_changes(
            [
                {"op": "create", "type": "window", "record": window_record("w-1", d.slug)},
                {"op": "checkout", "repo": d.slug, "name": "wt-a"},
            ]
        )
        phone_ctx, phone = device(browser)
        tablet_ctx, tablet = device(browser, 1024, 768)
        tablet.wait_for_timeout(4000)
        phone_ctx.set_offline(True)
        tablet.evaluate(f"(slug) => {SH}.setCheckout(slug, 'wt-b')", d.slug)
        tablet.wait_for_timeout(1500)
        check("the tablet's selection is stored", desk().get("checkouts", {}).get(d.slug) == "wt-b", json.dumps(desk().get("checkouts")))
        drag_window(phone, "w-1", 150)
        phone.wait_for_timeout(500)
        phone_ctx.set_offline(False)
        phone.wait_for_timeout(3000)
        check(
            "the phone's write keeps the tablet's selection",
            desk().get("checkouts", {}).get(d.slug) == "wt-b",
            json.dumps(desk().get("checkouts")),
        )
        check("the phone's drag is stored", near(record_of("w-1")["rect"]["left"], left_of(phone, "w-1")))
        got = phone.evaluate(f"(slug) => window.WBConsole.checkoutOf(slug)", d.slug)
        check("the phone shows the tablet's selection", got == "wt-b", f"got={got}")
        phone_ctx.close()
        tablet_ctx.close()


def scenario_5(browser):
    print("\n--- scenario 5: a fence moved on one device and renamed on another keeps both ---")
    with Daemon() as d:
        fence = {"id": "f-1", "name": "Fence 1", "rect": {"left": 40.0, "top": 400.0, "width": 520.0, "height": 300.0}}
        put_changes([{"op": "create", "type": "fence", "record": fence}])
        phone_ctx, phone = device(browser)
        pc_ctx, pc = device(browser)
        pc.wait_for_timeout(3000)
        phone_ctx.set_offline(True)
        grab = pc.evaluate(
            "() => { const r = document.querySelector('.fence .fence-grab').getBoundingClientRect();"
            " return { x: r.x, y: r.y, width: r.width, height: r.height }; }"
        )
        pc.mouse.move(grab["x"] + grab["width"] / 2, grab["y"] + grab["height"] / 2)
        pc.mouse.down()
        pc.mouse.move(grab["x"] + grab["width"] / 2 + 250, grab["y"] + grab["height"] / 2, steps=8)
        pc.mouse.up()
        pc.wait_for_timeout(1500)
        moved = desk()["fences"][0]["rect"]["left"]
        check("the PC's fence move is stored", moved > 200, f"left={moved}")
        name = phone.locator(".fence .fence-name")
        name.dblclick()
        phone.keyboard.press("Control+A")
        phone.keyboard.type("phone-fence")
        phone.keyboard.press("Enter")
        phone.wait_for_timeout(500)
        phone_ctx.set_offline(False)
        phone.wait_for_timeout(3000)
        f = desk()["fences"][0]
        check("the fence keeps the PC's move", f["rect"]["left"] == moved, f"left={f['rect']['left']}")
        check("the fence takes the phone's name", f["name"] == "phone-fence", f"name={f['name']}")
        pc.wait_for_timeout(1000)
        check(
            "the PC shows the name without a reload",
            pc.locator(".fence .fence-name").input_value() == "phone-fence",
        )
        phone_ctx.close()
        pc_ctx.close()


def scenario_6(browser):
    print("\n--- scenario 6: positions converge live, a drag in progress is not moved ---")
    with Daemon() as d:
        put_changes([{"op": "create", "type": "window", "record": window_record("w-1", d.slug)}])
        pc_ctx, pc = device(browser)
        phone_ctx, phone = device(browser)
        tablet_ctx, tablet = device(browser, 1024, 768)
        tablet.wait_for_timeout(4000)
        drag_window(pc, "w-1", 200, release=False)
        held = left_of(pc, "w-1")
        drag_window(phone, "w-1", 500)
        phone.wait_for_timeout(2000)
        theirs = left_of(phone, "w-1")
        check("the tablet shows the phone's drag live", near(left_of(tablet, "w-1"), theirs), f"tablet={left_of(tablet, 'w-1')} phone={theirs}")
        check("the PC's window under the operator's hand is not moved", near(left_of(pc, "w-1"), held), f"pc={left_of(pc, 'w-1')} held={held}")
        pc.mouse.up()
        pc.wait_for_timeout(2000)
        check("the last drag to reach the daemon wins", near(record_of("w-1")["rect"]["left"], held), f"daemon={record_of('w-1')['rect']['left']}")
        check("the phone shows it live", near(left_of(phone, "w-1"), held), f"phone={left_of(phone, 'w-1')}")
        phone.evaluate(f"() => ({WIN})('w-1').querySelector('.session-max').click()")
        phone.wait_for_timeout(1500)
        check("the phone's maximize is stored", record_of("w-1")["max"] is True)
        maxed = pc.evaluate(f"() => ({WIN})('w-1').classList.contains('maximized')")
        check("…and does not maximize the PC's window", maxed is False)
        pc_ctx.close()
        phone_ctx.close()
        tablet_ctx.close()


def scenario_7(browser):
    print("\n--- scenario 7: a resend after a lost reply does not undo a later change ---")
    with Daemon() as d:
        put_changes([{"op": "create", "type": "window", "record": window_record("w-1", d.slug)}])
        phone_ctx, phone = device(browser)
        pc_ctx, pc = device(browser)
        pc.wait_for_timeout(4000)
        lost = {"first": True}

        def deliver_then_lose(route):
            req = route.request
            if req.method == "PUT" and lost["first"]:
                lost["first"] = False
                route.fetch()  # the daemon applies the batch...
                route.abort()  # ...and the page never hears the reply
                return
            route.continue_()

        phone.route(lambda url: "/api/desk?tab=" in url, deliver_then_lose)
        drag_window(phone, "w-1", 300)
        phone.wait_for_timeout(700)
        check("the phone's batch reached the daemon", not lost["first"])
        phone_ctx.set_offline(True)
        pc.wait_for_timeout(800)
        drag_window(pc, "w-1", -150)
        pc.wait_for_timeout(1500)
        pcs = left_of(pc, "w-1")
        check("the PC's later move is stored", near(record_of("w-1")["rect"]["left"], pcs), f"pc={pcs}")
        phone_ctx.set_offline(False)
        phone.wait_for_timeout(4000)
        check(
            "the phone's resend does not undo the PC's move",
            near(record_of("w-1")["rect"]["left"], pcs),
            f"daemon={record_of('w-1')['rect']['left']} pc={pcs}",
        )
        check("the phone shows the PC's move", near(left_of(phone, "w-1"), pcs), f"phone={left_of(phone, 'w-1')}")
        phone_ctx.close()
        pc_ctx.close()


def main():
    base.build()
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
        for scenario in (scenario_1, scenario_2, scenario_3, scenario_4, scenario_5, scenario_6, scenario_7):
            try:
                scenario(browser)
            except Exception as e:  # a crashed scenario is a failed one; the next still runs
                check(f"{scenario.__name__} ran to the end", False, repr(e))
        browser.close()
    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if results and all(results) else 1)


if __name__ == "__main__":
    main()
