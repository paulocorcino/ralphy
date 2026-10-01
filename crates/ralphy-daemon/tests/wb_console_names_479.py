"""Console names (#479, ADR-0066, PRD #482 UI §§1–8 and §11) browser acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7479.

The fixture repo has an `origin` of `owner/fincal`, so its slug's last segment
is `fincal`. The fixture `desk.toml` is written BEFORE the daemon starts, with
NO names except one:
  w-1  shell, owner/fincal
  w-2  shell, no repo (`~`)
  w-3  placeholder agent (claude), owner/fincal
  w-4  shell, owner/fincal, locked
  w-5  shell, owner/fincal, inside the locked fence f-held
  w-6  shell, owner/fincal, `consoleName = "backend"`
  w-7  shell, owner/fincal, inside the fence f-away

Scenario 11  the first load names the desk in desk order; a second browser
             shows the same names; 11b: an upload without a name and with a
             newer `ts` leaves the stored name
Scenario 1   the title is `<name> (<label>) · <slug>`, no environment; a home
             console has no slug; a new shell and a new home shell take the
             lowest free number; an agent console's worktree button comes
             before the slug
Scenario 2   a placeholder shows its name and label
Scenario 4t  the title tooltip: the full ref, then the environment
Scenario 3   a single click raises and does not edit; a drag from the name moves
Scenario 4   double-click edits (whole name selected); Enter saves; Escape, a
             press elsewhere and a focus loss cancel; no maximize
Scenario 5   a rename works on a placeholder, a locked console, a console in a
             locked fence, a maximized console and a column
Scenario 6   trim; empty gives the default name; maxlength 40; duplicates
Scenario 9   Restart keeps the name
Scenario 10  the column list and the Go-to row use the one label; no repo head;
             the filter matches name, label, repo and fence
Scenario 12  a narrow bar cuts the slug first, then the label, then the name; the worktree
             button keeps its size
Scenario 8   in a detached fence the name is read-only; on return the desk's
             name is shown, not the snapshot's
Scenario 7   a rename in one browser reaches another on its next GET, and a
             later drag there does not undo it
Scenario 9b  the name survives a daemon restart

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_console_names_479.py   (exit 0 = all pass)
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7479
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
VIEW = {"width": 2400, "height": 1000}
SLUG = "owner/fincal"
FLOOR = 53  # every check above the floor check; pinned after the first green run

F_HELD = {"left": 20, "top": 320, "width": 480, "height": 340}
F_AWAY = {"left": 540, "top": 320, "width": 480, "height": 340}
WINDOWS = [
    # id, repo, agent, kind, rect, extra toml
    ("w-1", SLUG, "console", "console", {"left": 20, "top": 20, "width": 420, "height": 260}, ""),
    ("w-2", "~", "console", "console", {"left": 460, "top": 20, "width": 420, "height": 260}, ""),
    ("w-3", SLUG, "claude", "agent", {"left": 900, "top": 20, "width": 420, "height": 260}, ""),
    ("w-4", SLUG, "console", "console", {"left": 1340, "top": 20, "width": 420, "height": 260}, "locked = true\n"),
    ("w-5", SLUG, "console", "console", {"left": 40, "top": 370, "width": 420, "height": 260}, ""),
    ("w-6", SLUG, "console", "console", {"left": 1340, "top": 320, "width": 420, "height": 260},
     'consoleName = "backend"\n'),
    ("w-7", SLUG, "console", "console", {"left": 560, "top": 370, "width": 420, "height": 260}, ""),
]
FIRST_NAMES = {
    "w-1": "fincal #1",
    "w-2": "home #1",
    "w-3": "fincal #2",
    "w-4": "fincal #3",
    "w-5": "fincal #4",
    "w-6": "backend",
    "w-7": "fincal #5",
}

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def shot(page, name):
    page.screenshot(path=os.path.join(SHOT_DIR, f"479-console-names-{name}-2026-09-27.png"))


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
    empty = tempfile.mkdtemp(prefix="wbname_empty_")
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
    d = tempfile.mkdtemp(prefix="wbname_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe console names fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbname@example.com"],
        ["git", "config", "user.name", "wbname"],
        ["git", "remote", "add", "origin", f"https://github.com/{SLUG}.git"],
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


def rect_toml(r):
    return "rect = { left = %(left)s, top = %(top)s, width = %(width)s, height = %(height)s }" % r


def write_fixture_desk(daemon_dir, slug):
    out = ""
    for ts, (wid, repo, agent, kind, rect, extra) in enumerate(WINDOWS, start=100):
        out += (
            "[[windows]]\n"
            f'id = "{wid}"\n'
            f'repo = "{slug if repo == SLUG else repo}"\n'
            f'agent = "{agent}"\n'
            f'kind = "{kind}"\n'
            "max = false\n"
            f"{extra}"
            f"ts = {ts}\n"
            f"{rect_toml(rect)}\n\n"
        )
    for ts, (fid, name, rect, locked) in enumerate(
        [("f-held", "held", F_HELD, True), ("f-away", "away", F_AWAY, False)], start=200
    ):
        out += (
            "[[fences]]\n"
            f'id = "{fid}"\n'
            f'name = "{name}"\n'
            + ("locked = true\n" if locked else "")
            + f"ts = {ts}\n"
            f"{rect_toml(rect)}\n\n"
        )
    Path(daemon_dir, "desk.toml").write_bytes(out.encode("utf-8"))


def build():
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # the previous build's console.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def http(method, path, body=None):
    data = None
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=5) as r:
        return r.status, r.read().decode()


def desk_windows():
    return {r["id"]: r for r in json.loads(http("GET", "api/desk")[1])["windows"]}


def poll_desk(predicate, timeout=8):
    """A desk write is debounced-then-HTTP: poll, never one fixed sleep."""
    deadline = time.time() + timeout
    got = None
    while time.time() < deadline:
        try:
            got = desk_windows()
        except Exception:
            got = None
        if got and predicate(got):
            return got
        time.sleep(0.3)
    return got


HELPERS = """() => {
  window.__W = (id) => [...document.querySelectorAll('.session-window')].find((w) => w._deskId === id) || null;
  window.__name = (id) => __W(id)?.querySelector('.session-name')?.textContent ?? null;
  window.__title = (id) => (__W(id)?.querySelector('.session-title')?.textContent || '').replace(/\\s+/g, ' ').trim();
  window.__tip = (id) => __W(id)?.querySelector('.session-title')?.title ?? null;
  window.__input = (id) => __W(id)?.querySelector('.session-name-input') || null;
  window.__names = () => Object.fromEntries([...document.querySelectorAll('.session-window')]
    .map((w) => [w._deskId, w.querySelector('.session-name')?.textContent ?? null]));
  window.__center = (el) => { const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };
}"""


def install(page):
    page.evaluate(HELPERS)


def open_page(ctx, errors, want):
    page = ctx.new_page()
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    page.wait_for_function(
        "(n) => { const ws = [...document.querySelectorAll('.session-window')];"
        " return ws.length >= n && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        arg=want,
        timeout=30000,
    )
    page.wait_for_timeout(800)
    install(page)
    return page


def name_center(page, id):
    # Raised first, as the operator's first press would: the consoles opened in
    # scenario 1 cascade over the fixture windows.
    return page.evaluate(
        "(id) => { const w = __W(id); w.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));"
        " const s = w.querySelector('.session-name'); s.scrollIntoView({block: 'nearest', inline: 'nearest'});"
        " return __center(s); }",
        id,
    )


def dbl_name(page, id):
    c = name_center(page, id)
    page.mouse.dblclick(c["x"], c["y"])
    page.wait_for_function("(id) => !!__input(id)", arg=id, timeout=3000)


def rename(page, id, text):
    dbl_name(page, id)
    page.keyboard.press("Control+A")
    if text:
        page.keyboard.type(text)
    else:
        page.keyboard.press("Delete")
    page.keyboard.press("Enter")
    page.wait_for_function("(id) => !__input(id)", arg=id, timeout=3000)
    return page.evaluate("(id) => __name(id)", id)


def new_window(page, js):
    before = set(page.evaluate("() => [...document.querySelectorAll('.session-window')].map((w) => w._deskId)"))
    page.evaluate(js)
    page.wait_for_function(
        "(n) => document.querySelectorAll('.session-window').length > n", arg=len(before), timeout=15000
    )
    page.wait_for_timeout(600)
    after = page.evaluate("() => [...document.querySelectorAll('.session-window')].map((w) => w._deskId)")
    return [i for i in after if i not in before][0]


def main():
    build()
    os.makedirs(SHOT_DIR, exist_ok=True)
    daemon_dir = tempfile.mkdtemp(prefix="wbname_daemon_")
    fixture = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture)
    check("the fixture registers as owner/fincal", slug == SLUG, slug)
    write_fixture_desk(daemon_dir, slug)
    proc = launch(daemon_dir)
    errors = []
    names_before_restart = {}
    try:
        if not wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch()
            ctx = browser.new_context(viewport=dict(VIEW))
            page = open_page(ctx, errors, len(WINDOWS))

            # 11: the first load names the desk in desk order ---------------
            names = page.evaluate("() => __names()")
            check("11 the first load names the desk in desk order; a named record keeps its name",
                  all(names.get(k) == v for k, v in FIRST_NAMES.items()), str(names))
            check("11 a restored home console is `home #<n>`, never `~`", names.get("w-2") == "home #1",
                  str(names.get("w-2")))
            desk = poll_desk(lambda d: all(d.get(k, {}).get("consoleName") == v for k, v in FIRST_NAMES.items()
                                           if k != "w-3"), timeout=10)
            check("11 the names reach the stored desk with the next flush",
                  desk and all(desk[k].get("consoleName") == v for k, v in FIRST_NAMES.items() if k != "w-3"),
                  str({k: (desk or {}).get(k, {}).get("consoleName") for k in FIRST_NAMES}))
            ctx_b = browser.new_context(viewport=dict(VIEW))
            page_b = open_page(ctx_b, errors, len(WINDOWS))
            names_b = page_b.evaluate("() => __names()")
            check("11 a second browser shows the same names", names_b == names, f"{names_b} vs {names}")
            ctx_b.close()
            # 11b: a record from a shell that predates the name.
            raw = desk_windows()["w-4"]
            raw.pop("consoleName", None)
            raw["ts"] = int(time.time() * 1000) + 1
            status, _ = http("PUT", "api/desk", {"windows": [raw], "fences": [], "removed": {"windows": []}})
            got = desk_windows()["w-4"]
            check("11b an upload without a name and a newer ts keeps the stored name",
                  status == 200 and got.get("consoleName") == "fincal #3" and got["ts"] == raw["ts"],
                  f"status={status} got={got.get('consoleName')} ts={got['ts']}")

            # 1: the title -------------------------------------------------
            t1 = page.evaluate("() => __title('w-1')")
            check("1 a shell's title is `<name> (console) · <slug>`, no environment",
                  t1 == f"fincal #1 (console) · {SLUG}", repr(t1))
            n_new = new_window(page, f"() => window.WBConsole.open({{ repo: '{SLUG}', plain: true }})")
            page.wait_for_function("(id) => !!__W(id)._presentation?.environment", arg=n_new, timeout=15000)
            t_new = page.evaluate("(id) => __title(id)", n_new)
            check("1 a new shell takes the lowest free number", t_new == f"fincal #6 (console) · {SLUG}", repr(t_new))
            h_new = new_window(page, "() => window.WBConsole.open({ plain: true })")
            t_home = page.evaluate("(id) => __title(id)", h_new)
            check("1 a new console with no repo is `home #<n>`, with no slug", t_home == "home #2 (console)", repr(t_home))
            a_new = None
            claude = subprocess.run(["claude", "--version"], capture_output=True, text=True, shell=os.name == "nt")
            if claude.returncode == 0:
                a_new = new_window(page, f"() => window.WBConsole.open({{ repo: '{SLUG}', agent: 'claude' }})")
                page.wait_for_function("(id) => !!__W(id).querySelector('.session-checkout')", arg=a_new,
                                       timeout=20000)
                t_agent = page.evaluate("(id) => __title(id)", a_new)
                tail = page.evaluate("(id) => __W(id).querySelector('.session-checkout').nextElementSibling?.className",
                                     a_new)
                check("1 an agent console reads `<name> (claude) · primary · <slug>`, the slug after the button",
                      t_agent == f"fincal #7 (claude) · primary · {SLUG}" and tail == "session-repo",
                      f"{t_agent!r} after-button={tail!r}")
            else:
                check("1 agent sub-check: claude CLI present", False, claude.stderr.strip())
            shot(page, "title")

            # 2: a placeholder --------------------------------------------
            t3 = page.evaluate("() => ({ t: __title('w-3'), ph: __W('w-3').classList.contains('placeholder') })")
            check("2 a placeholder shows `<name> (<agent>)`", t3["ph"] and t3["t"] == f"fincal #2 (claude) · {SLUG}", str(t3))

            # 4t: the tooltip ---------------------------------------------
            tip1 = page.evaluate("() => __tip('w-1')").split("\n")
            check("4t the title tooltip is the full ref, then the environment",
                  len(tip1) == 2 and tip1[0] == SLUG and tip1[1] and tip1[1] != SLUG, str(tip1))
            tip2 = page.evaluate("() => __tip('w-2')").split("\n")
            check("4t a home console's tooltip leaves the empty ref out", len(tip2) == 1 and tip2[0] != "~", str(tip2))

            # 3: single click, drag ---------------------------------------
            page.evaluate("() => __W('w-6').classList.contains('focused') || null")
            c = name_center(page, "w-6")
            page.mouse.click(c["x"], c["y"])
            page.wait_for_timeout(300)
            s3 = page.evaluate("() => ({ focused: __W('w-6').classList.contains('focused'), input: !!__input('w-6') })")
            check("3 a single click raises the window and opens no edit", s3["focused"] and not s3["input"], str(s3))
            left0 = page.evaluate("() => __W('w-6').getBoundingClientRect().left")
            page.mouse.move(c["x"], c["y"])
            page.mouse.down()
            page.mouse.move(c["x"] - 30, c["y"] + 10, steps=4)
            page.mouse.move(c["x"] - 60, c["y"] + 20, steps=4)
            page.mouse.up()
            page.wait_for_timeout(300)
            left1 = page.evaluate("() => __W('w-6').getBoundingClientRect().left")
            check("3 a drag that starts on the name moves the window", abs((left0 - left1) - 60) <= 2,
                  f"{left0} -> {left1}")

            # 4: double-click edits ---------------------------------------
            dbl_name(page, "w-1")
            s4 = page.evaluate(
                "() => { const i = __input('w-1'); return { label: i.getAttribute('aria-label'), max: i.getAttribute('maxlength'),"
                " start: i.selectionStart, end: i.selectionEnd, len: i.value.length, focus: document.activeElement === i,"
                " maximized: __W('w-1').classList.contains('maximized') }; }"
            )
            check("4 a double-click opens the name with the whole name selected",
                  s4["label"] == "Console name" and s4["max"] == "40" and s4["start"] == 0 and s4["end"] == s4["len"]
                  and s4["focus"], str(s4))
            check("4 a double-click on the name does not maximize", not s4["maximized"], str(s4))
            shot(page, "rename")
            page.keyboard.type("renamed")
            page.keyboard.press("Enter")
            page.wait_for_timeout(200)
            check("4 Enter saves", page.evaluate("() => __name('w-1')") == "renamed")
            d = poll_desk(lambda d: d["w-1"].get("consoleName") == "renamed")
            check("4 the rename reaches the desk", d and d["w-1"].get("consoleName") == "renamed")
            for how in ("escape", "stage", "blur"):
                dbl_name(page, "w-1")
                page.keyboard.type("zzz")
                if how == "escape":
                    page.keyboard.press("Escape")
                elif how == "stage":
                    page.mouse.click(1900, 900)
                else:
                    page.evaluate("() => __input('w-1').blur()")
                page.wait_for_timeout(200)
                s = page.evaluate("() => ({ name: __name('w-1'), input: !!__input('w-1'),"
                                  " max: __W('w-1').classList.contains('maximized') })")
                check(f"4 {how} cancels", s["name"] == "renamed" and not s["input"] and not s["max"], str(s))

            # 6: trim, empty, maxlength, duplicate ------------------------
            check("6 spaces are trimmed", rename(page, "w-1", "  x  ") == "x")
            check("6 an empty name gets the default name again", rename(page, "w-1", "") == "fincal #1")
            dbl_name(page, "w-1")
            page.keyboard.press("Control+A")
            page.keyboard.type("a" * 45)
            typed = page.evaluate("() => __input('w-1').value.length")
            page.keyboard.press("Escape")
            check("6 the input holds at most 40 characters", typed == 40, str(typed))
            check("6 a duplicate name is allowed", rename(page, "w-1", "backend") == "backend")
            rename(page, "w-1", "fincal #1")

            # 5: every kind of console -------------------------------------
            check("5 a rename works on a placeholder", rename(page, "w-3", "ph") == "ph")
            check("5 a rename works on a locked console", rename(page, "w-4", "held-self") == "held-self")
            check("5 a rename works in a locked fence", rename(page, "w-5", "held-fence") == "held-fence")
            page.evaluate("() => __W('w-6').querySelector('.session-max').click()")
            page.wait_for_timeout(400)
            check("5 a rename works on a maximized console", rename(page, "w-6", "big") == "big")
            s5 = page.evaluate("() => __W('w-6').classList.contains('maximized')")
            check("5 the maximized console stays maximized", s5)

            # 10: the column list and Go-to -------------------------------
            page.evaluate("() => __W('w-6').querySelector('.session-column').click()")
            page.wait_for_function(f"() => {SH}.columnMenu === true", timeout=5000)
            page.wait_for_timeout(200)
            s10 = page.evaluate(
                "() => ({ repoHeads: document.querySelectorAll('.column-menu .column-group-repo').length,"
                " heads: [...document.querySelectorAll('.column-menu .column-group-head')]"
                "   .filter((h) => getComputedStyle(h).display !== 'none').map((h) => h.textContent.trim()),"
                " rows: Object.fromEntries([...document.querySelectorAll('.column-menu .column-item')]"
                "   .map((b) => [b.dataset.id, b.querySelector('.row-name').textContent])) })"
            )
            check("10 no group head prints a repo; fence heads keep the fence name",
                  s10["repoHeads"] == 0 and sorted(s10["heads"]) == ["away", "held"], str(s10))
            check("10 a column row reads `<name> (<label>)`",
                  s10["rows"].get("w-1") == "fincal #1 (console)" and s10["rows"].get("w-3") == "ph (claude)"
                  and s10["rows"].get("w-2") == "home #1 (console)", str(s10["rows"]))
            shot(page, "columns")

            def filtered(q):
                return page.evaluate(
                    f"(q) => {{ {SH}.columnFilter = q; return {SH}.columnView().flatMap((g) => g.rows.map((r) => r.id)); }}",
                    q,
                )

            check("10 the filter matches the console name", filtered("held-self") == ["w-4"], str(filtered("held-self")))
            check("10 the filter matches the label", "w-3" in filtered("claude") and "w-1" not in filtered("claude"))
            check("10 the filter matches the repo", "w-1" in filtered("owner/fincal") and "w-2" not in filtered("owner/fincal"))
            check("10 the filter matches the fence name", filtered("away") == ["w-7"], str(filtered("away")))
            page.evaluate(f"() => {{ {SH}.columnFilter = ''; }}")
            page.locator(".column-menu .column-item[data-id='w-2']").click()
            page.wait_for_timeout(600)
            col = page.evaluate("() => __W('w-2').classList.contains('column')")
            check("10 w-2 opens as a column", col)
            check("5 a rename works on a column", rename(page, "w-2", "col") == "col")
            page.evaluate("() => __W('w-6').querySelector('.session-max').click()")
            page.wait_for_timeout(400)
            page.evaluate("() => __W('w-2').classList.contains('maximized') && __W('w-2').querySelector('.session-max').click()")
            page.wait_for_timeout(400)
            page.evaluate(f"() => {SH}.toggleWindowMenu()")
            page.wait_for_timeout(300)
            g = page.evaluate(
                "() => ({ hidden: getComputedStyle(document.querySelector('.window-menu').closest('.menu-wrap')).display,"
                " rows: [...document.querySelectorAll('.window-item')].map((b) => { const s = b.querySelector('span');"
                "   return { text: s.textContent, title: s.title }; }),"
                " titles: [...document.querySelectorAll('.session-window')].map((w) => ({"
                "   text: w.querySelector('.session-name').textContent + ' ' + w.querySelector('.session-label').textContent,"
                "   title: w.querySelector('.session-title').title })) })"
            )
            check("10 the Go-to menu stays hidden", g["hidden"] == "none", g["hidden"])
            check("10 each Go-to row says what its title says, with the title's tooltip",
                  g["rows"] == g["titles"] and len(g["rows"]) >= len(WINDOWS), json.dumps(g)[:600])
            page.evaluate(f"() => {{ {SH}.windowMenu = false; }}")

            # 9: Restart keeps the name -----------------------------------
            page.evaluate("() => __W('w-1').querySelector('.session-restart').click()")
            page.wait_for_selector(".wb-confirm .btn.danger, .wb-confirm .btn.accent", timeout=5000)
            old = page.evaluate("() => { window.__old = __W('w-1'); return true; }")
            page.locator(".wb-confirm .btn.danger, .wb-confirm .btn.accent").click()
            page.wait_for_function("() => __W('w-1') && __W('w-1') !== window.__old && !__W('w-1').classList.contains('placeholder')",
                                   timeout=20000)
            page.wait_for_timeout(500)
            check("9 Restart keeps the name", old and page.evaluate("() => __name('w-1')") == "fincal #1",
                  page.evaluate("() => __name('w-1')"))

            # 12: the narrow bar ------------------------------------------
            target = a_new or "w-1"
            page.evaluate("(id) => { const w = __W(id); w.style.minWidth = '0px'; w.style.left = '1500px'; w.style.top = '700px'; }", target)
            samples = []
            for width in range(700, 150, -10):
                samples.append(page.evaluate(
                    "([id, width]) => { const w = __W(id); w.style.width = width + 'px';"
                    " const cut = (s) => { const e = w.querySelector(s); return e.scrollWidth > e.clientWidth + 1; };"
                    " const ck = w.querySelector('.session-checkout');"
                    " return { width, repo: cut('.session-repo'), label: cut('.session-label'), name: cut('.session-name'),"
                    "   ck: ck ? ck.getBoundingClientRect().width : null,"
                    "   ell: getComputedStyle(w.querySelector('.session-label')).textOverflow }; }",
                    [target, width],
                ))
            first_repo = next((s for s in samples if s["repo"]), None)
            first_label = next((s for s in samples if s["label"]), None)
            first_name = next((s for s in samples if s["name"]), None)
            check("12 the slug is cut first",
                  first_repo is not None and not first_repo["label"] and not first_repo["name"], str(first_repo))
            check("12 then the label, with an ellipsis",
                  first_label is not None and not first_label["name"] and first_label["ell"] == "ellipsis",
                  str(first_label))
            check("12 then the name is cut", first_name is not None and first_name["width"] < first_label["width"],
                  f"{first_name} after {first_label}")
            if a_new:
                widths = {round(s["ck"], 1) for s in samples if s["ck"] is not None}
                check("12 the worktree button keeps its size", len(widths) == 1, str(widths))
            page.evaluate("(id) => { __W(id).style.width = '600px'; }", target)
            shot_w = (first_label or {}).get("width", 300) - 20
            page.evaluate("([id, w]) => __W(id).style.width = w + 'px'", [target, shot_w])
            shot(page, "narrow")
            page.evaluate("(id) => __W(id).style.width = '600px'", target)

            # 8: a detached fence -----------------------------------------
            with page.expect_popup(timeout=15000) as info:
                page.evaluate("() => document.querySelector(\"[data-fence-id='f-away'] .fence-detach\").click()")
            popup = info.value
            popup.on("pageerror", lambda e: errors.append("popup: " + str(e)))
            popup.wait_for_load_state()
            popup.wait_for_function("() => document.querySelectorAll('.session-window').length === 1", timeout=30000)
            popup.wait_for_timeout(600)
            pn = popup.evaluate(
                "() => { const s = document.querySelector('.session-window .session-name'); const r = s.getBoundingClientRect();"
                " return { text: s.textContent, x: r.left + r.width / 2, y: r.top + r.height / 2 }; }"
            )
            check("8 the detached console shows its name", pn["text"] == "fincal #5", str(pn))
            popup.mouse.dblclick(pn["x"], pn["y"])
            popup.wait_for_timeout(300)
            p8 = popup.evaluate("() => ({ input: !!document.querySelector('.session-name-input'),"
                                " max: document.querySelector('.session-window').classList.contains('maximized') })")
            check("8 in a detached fence a double-click on the name does nothing", not p8["input"] and not p8["max"],
                  str(p8))
            shot(popup, "detached")
            raw = desk_windows()["w-7"]
            raw["consoleName"] = "away-renamed"
            raw["ts"] = int(time.time() * 1000) + 1
            http("PUT", "api/desk", {"windows": [raw], "fences": [], "removed": {"windows": []}})
            page.evaluate("() => window.WBConsole.afterLogin()")
            page.wait_for_timeout(500)
            page.evaluate("() => window.WBConsole.reattachFence('f-away')")
            page.wait_for_function("() => !!__W('w-7')", timeout=20000)
            page.wait_for_timeout(500)
            check("8 back on the stage the console shows the desk's name, not the snapshot's",
                  page.evaluate("() => __name('w-7')") == "away-renamed", page.evaluate("() => __name('w-7')"))

            # 7: two browsers ---------------------------------------------
            ctx_b = browser.new_context(viewport=dict(VIEW))
            page_b = open_page(ctx_b, errors, len(WINDOWS))
            check("7 a new page loads the current names", page_b.evaluate("() => __name('w-2')") == "col")
            rename(page, "w-2", "from-a")
            poll_desk(lambda d: d["w-2"].get("consoleName") == "from-a")
            page_b.evaluate("() => window.WBConsole.afterLogin()")
            page_b.wait_for_function("() => __name('w-2') === 'from-a'", timeout=5000)
            check("7 the other page shows the rename after its next GET", page_b.evaluate("() => __name('w-2')") == "from-a")
            c = page_b.evaluate("() => __center(__W('w-6').querySelector('.session-titlebar'))")
            page_b.mouse.move(c["x"] + 40, c["y"])
            page_b.mouse.down()
            page_b.mouse.move(c["x"] + 10, c["y"] + 20, steps=4)
            page_b.mouse.move(c["x"] - 20, c["y"] + 40, steps=4)
            page_b.mouse.up()
            page_b.wait_for_timeout(2500)
            d = desk_windows()
            check("7 a later drag on the other page does not bring the old name back",
                  d["w-2"].get("consoleName") == "from-a", str(d["w-2"].get("consoleName")))
            ctx_b.close()

            names_before_restart = page.evaluate("() => __names()")
            check("no page errors", not errors, str(errors[:3]))
            browser.close()
    finally:
        stop(proc)

    # 9b: a daemon restart --------------------------------------------
    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check("daemon listening after restart", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch()
            ctx = browser.new_context(viewport=dict(VIEW))
            page = open_page(ctx, errors, len(WINDOWS))
            after = page.evaluate("() => __names()")
            same = all(after.get(k) == v for k, v in names_before_restart.items() if k in after)
            check("9b the names survive a daemon restart", same and len(after) >= len(WINDOWS),
                  f"{after} vs {names_before_restart}")
            browser.close()
    finally:
        stop(proc)


if __name__ == "__main__":
    main()
    passed = sum(results)
    print(f"{passed}/{len(results)} passed", flush=True)
    if FLOOR is not None:
        check("check floor", len(results) == FLOOR, f"{len(results)} != {FLOOR}")
    sys.exit(0 if results and all(results) else 1)
