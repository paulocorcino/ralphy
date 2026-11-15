"""The detached file window shows its file, browser acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7484.

The fixture: a repository with one file, `README.md`, open in a tab.

  menu        the pane's Detach button (the operator's path) opens the window
  descriptor  `detachFile` is called with the descriptor that button sends
  tab         diagnosis only, no check: `detachFile` with the tab record, as a
              scripted check once did; the record has no `label` and no `bom`

Each check waits up to 5 s for the file text in the window and fails on a page
error. The script prints the window's URL, what `#viewers` holds, the console
lines and the messages between the two windows.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: cargo build -p ralphy-cli --bin ralphy
     python tests/browser/files/wb_detached_file.py menu|descriptor|tab
"""

import json
import os
import sys
import tempfile
import traceback

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "columns"))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

PORT = 7484
T.PORT = PORT
BASE = f"http://127.0.0.1:{PORT}/"
TEXT = "The columns fixture repo."
PANE = '.viewer[data-tab-id$=":README.md"]'
NAMES = {
    "menu": "detached file from the tab menu shows its text",
    "descriptor": "detached file from detachFile shows its text",
}

# Every message a window receives, written to its console for the log below.
LOG_MESSAGES = """window.addEventListener('message', (e) => {
  const d = e.data;
  console.log('[message] ' + JSON.stringify({ type: d && d.type, keys: d && d.desc ? Object.keys(d.desc) : null }));
});"""

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def open_readme(page, slug):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.wait_for_function("() => !!window.Alpine && !!Alpine.store('tabs')", timeout=10000)
    page.evaluate(
        "(s) => Alpine.store('tabs').openLink({ project: s, path: 'README.md', checkout: null })", slug
    )
    page.wait_for_function("(sel) => !!document.querySelector(sel + ' .md-body')", arg=PANE, timeout=15000)


def detach(ctx, page, how):
    with ctx.expect_page() as popup:
        if how == "menu":
            page.locator(f'{PANE} [data-act="detach"]').click()
        elif how == "descriptor":
            page.evaluate(
                "(sel) => Alpine.store('tabs').detachFile("
                "WBViewer.descOf(document.querySelector(sel).dataset.tabId))",
                PANE,
            )
        else:
            page.evaluate(
                "(sel) => { const id = document.querySelector(sel).dataset.tabId;"
                " const tab = Alpine.store('tabs').tabs.find((t) => t.id === id);"
                " Alpine.store('tabs').detachFile(JSON.parse(JSON.stringify(tab))); }",
                PANE,
            )
    return popup.value


def watch(page, who, errors):
    page.on("console", lambda m: print(f"  {who} console: {m.text}", flush=True))
    page.on("pageerror", lambda e: errors.append(f"{who}: {e}"))


def run(how, slug):
    errors = []
    with sync_playwright() as p:
        b = p.chromium.launch()
        ctx = b.new_context(viewport={"width": 1400, "height": 900})
        ctx.add_init_script(LOG_MESSAGES)
        page = ctx.new_page()
        watch(page, "opener", errors)
        page.goto(BASE)
        open_readme(page, slug)
        popup = detach(ctx, page, how)
        watch(popup, "popup", errors)
        try:
            popup.wait_for_function("(t) => document.querySelector('#viewers').innerText.includes(t)", arg=TEXT, timeout=5000)
            shown = True
        except Exception:
            shown = False
        viewers = popup.evaluate("() => document.querySelector('#viewers')?.innerText ?? null")
        print(f"  popup url: {popup.url}")
        print(f"  #viewers: {json.dumps((viewers or '')[:200])}")
        print(f"  page errors: {errors}")
        if how in NAMES:
            check(NAMES[how], shown and not errors, f"shown={shown} errors={len(errors)}")
        else:
            print(f"  tab record: shown={shown}")
        b.close()


def main():
    how = sys.argv[1] if len(sys.argv) > 1 else ""
    if how not in ("menu", "descriptor", "tab"):
        print("usage: wb_detached_file.py menu|descriptor|tab")
        sys.exit(2)
    d = tempfile.mkdtemp(prefix="wbdetfile_")
    fx = T.make_fixture_repo()
    slug = T.register_fixture(d, fx)
    proc = T.launch(d)
    try:
        if not T.wait_listening(BASE):
            check("daemon listening", False)
            return
        run(how, slug)
    except BaseException:
        traceback.print_exc()
        results.append(False)
    finally:
        T.stop(proc)
        if how != "tab":
            print(f"\n{sum(results)}/{len(results)} passed")
            sys.exit(0 if results and all(results) else 1)


if __name__ == "__main__":
    main()
