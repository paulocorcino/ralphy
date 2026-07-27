"""#332 browser acceptance: the sidebar Projects list.

One Playwright pass over a REAL daemon proving the four fixes: the rows paint
their icons with no click, a remoteless repo is labelled by its directory rather
than by its `path-<hash>` slug, a long name truncates instead of wrapping, and
the branch chip has moved to the Files bar — leaving the name the width it used
to spend on a four-character branch.

Scenario 1  on FIRST paint, with no click anywhere: every project row's chevron
            is a real `<svg>` and no `data-lucide` placeholder survives in the
            list. This is the whole reason `loadRepos` converts its own icons.
Scenario 2  the remoteless rows read their DIRECTORY name while their title is
            still the `path-<hash>` slug (the identity is untouched), and the
            GitHub-backed row is unchanged
Scenario 3  the directory label is searchable — typing what the row prints must
            not return an empty list
Scenario 4  the long name stays on ONE line and truncation is actually engaged
            (`scrollWidth > clientWidth`), not merely a short name fitting
Scenario 5  the name is wide: over 150px, where the 48% branch-chip cap used to
            leave it around 40
Scenario 6  the column has ONE left edge: the section head, the search box and
            the project rows share a padding
Scenario 7  the chip is gone from the row (whose title still names the branch,
            because the filter matches on it) and lives in the Files bar of the
            OPEN project — rendered in its own case, not the bar's uppercase —
            and still opens the branch picker
Scenario 8  scenarios 4 and 5 again at a 160px sidebar, since `--side-w` is a
            fixed track no viewport width reflows

Boots a Localhost daemon on 7422 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Every geometry assertion is gated on `clientWidth > 0`, repeated inside the
assertion: a measurement of a zero-width element passes a "fits" test vacuously
(CONTEXT.md, the vacuous-geometry trap).

Writes docs/screenshots/332-projects-list-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_projects_332.py   (exit 0 = all pass)
"""

import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7422
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_projects_332.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

# The directory names ARE the fixture: scenario 2 asserts the label is the
# basename, so these must be chosen, not mkdtemp's random suffix.
DIR_LOCAL = "my-local-repo"
DIR_LONG = "a-deliberately-long-local-directory-name"
BRANCH_LONG = "feat/Mixed-Case-Branch"

# 160px, not 320: the default `--side-w` is 300, so any "too narrow" assertion
# at 320 passes vacuously. The 0.2s grid transition wants ~400ms of settle.
SQUEEZED_SIDE_W = "160px"

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


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def empty_env(daemon_dir):
    """A scratch registry + empty vendor stores: the operator's own daemon dir
    (and its login policy) is never touched, and the usage scan finds nothing."""
    empty = tempfile.mkdtemp(prefix="wb332_empty_")
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


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def seed(parent_prefix, name, branch="main", origin=None):
    """A committed git repo at a CHOSEN directory name. Without `origin` the
    daemon keys it `path-<hash>` (ADR-0008 D7) — which is exactly the case
    scenario 2 is about, so it needs no stubbing."""
    d = Path(tempfile.mkdtemp(prefix=parent_prefix)) / name
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "README.md").write_text(f"# {name}\n\nThe #332 sidebar fixture repo.\n", encoding="utf-8")
    git(d, "init", "-b", branch)
    git(d, "config", "user.email", "wb332@example.com")
    git(d, "config", "user.name", "wb332")
    if origin:
        git(d, "remote", "add", "origin", origin)
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return str(d)


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's sidebar.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# One project row, by slug. The row is found through `.project-slug`'s `title`
# (the full slug) because the visible label is the UPPERCASED name — which is
# precisely what this issue changed, so the locator must not depend on it.
ROW_EXPR = (
    "(s) => { const r = Array.from(document.querySelectorAll('li.project'))"
    "  .find(e => { const n = e.querySelector('.project-slug');"
    "              return n && n.getAttribute('title') === s; });"
    "  if (!r) return null; const n = r.querySelector('.project-slug');"
    "  const b = n.getBoundingClientRect();"
    "  const head = r.querySelector('.project-head');"
    "  return { label: n.textContent.trim(), title: n.getAttribute('title'),"
    "    rowTitle: head ? head.getAttribute('title') : null,"
    # Gated AND reported: a zero-width name satisfies "fits on one line"
    # without ever having been laid out.
    "    laid: n.offsetParent !== null && n.clientWidth > 0,"
    "    width: n.clientWidth, scrollWidth: n.scrollWidth, height: b.height,"
    "    ellipsis: getComputedStyle(n).textOverflow,"
    "    chipsInRow: head ? head.querySelectorAll('.branch-chip').length : -1,"
    "    lucideLeft: r.querySelectorAll('[data-lucide]').length,"
    "    chevronSvg: r.querySelectorAll('.chevron svg').length }; }"
)


def row(page, slug):
    return page.evaluate(ROW_EXPR, arg=slug)


def visible_slugs(page):
    return page.evaluate(
        "() => Array.from(document.querySelectorAll('li.project'))"
        "  .filter(e => e.offsetParent !== null)"
        "  .map(e => { const n = e.querySelector('.project-slug');"
        "              return n ? n.getAttribute('title') : null; })"
    )


def set_side_w(page, value):
    if value:
        page.evaluate("(w) => document.documentElement.style.setProperty('--side-w', w)", arg=value)
    else:
        page.evaluate("() => document.documentElement.style.removeProperty('--side-w')")
    page.wait_for_timeout(400)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb332_reg_")
    dir_a = seed("wb332_a_", "widget", origin="https://github.com/acme/widget.git")
    dir_b = seed("wb332_b_", DIR_LOCAL)
    dir_c = seed("wb332_c_", DIR_LONG, branch=BRANCH_LONG)
    slug_a = register_fixture(daemon_dir, dir_a)
    slug_b = register_fixture(daemon_dir, dir_b)
    slug_c = register_fixture(daemon_dir, dir_c)

    check(
        "the two remoteless fixtures really are path-hashed",
        slug_b.startswith("path-") and slug_c.startswith("path-") and slug_a == "acme/widget",
        "a={} b={} c={}".format(slug_a, slug_b, slug_c),
    )

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 3", timeout=15000)
            # The rows exist; give Alpine's x-for one frame to place them.
            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('li.project'))"
                "  .filter(e => e.offsetParent !== null).length === 3",
                timeout=15000,
            )

            # --- scenario 1: icons on first paint, with NO click --------------
            # Nothing above this point clicked anything. `toggle()` re-scans the
            # document, so a single click before here would mask the defect.
            # `i[data-lucide]`, not `[data-lucide]`: lucide COPIES the source
            # element's attributes onto the `<svg>` it swaps in, so a converted
            # icon still answers the bare attribute selector. Only a surviving
            # `<i>` is an unpainted icon.
            icons = page.evaluate(
                "() => ({ placeholders: document.querySelectorAll('li.project i[data-lucide]').length,"
                "  chevrons: document.querySelectorAll('li.project .chevron svg').length,"
                "  chips: document.querySelectorAll('li.project .branch-chip svg').length })"
            )
            check(
                "every row's icons are drawn before any click",
                icons["chevrons"] == 3 and icons["chips"] == 3 and icons["placeholders"] == 0,
                "got={}".format(icons),
            )

            # --- scenario 2: the remoteless label -----------------------------
            r_a, r_b, r_c = row(page, slug_a), row(page, slug_b), row(page, slug_c)
            label_b = r_b["label"] if r_b else None
            title_b = r_b["title"] if r_b else None
            check(
                "a remoteless row is labelled by its directory",
                label_b == DIR_LOCAL.upper(),
                "got={} want={}".format(label_b, DIR_LOCAL.upper()),
            )
            check(
                "…while its title is still the path-hash identity",
                bool(title_b) and title_b.startswith("path-"),
                "got={}".format(title_b),
            )
            check(
                "a GitHub-backed row is unchanged",
                bool(r_a) and r_a["label"] == "WIDGET" and r_a["title"] == "acme/widget",
                "got={}".format(r_a and (r_a["label"], r_a["title"])),
            )

            # --- scenario 3: the label is searchable --------------------------
            page.evaluate(f"(q) => {{ {SH}.projectQuery = q; }}", arg="my-local")
            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('li.project'))"
                "  .filter(e => e.offsetParent !== null).length === 1",
                timeout=10000,
            )
            shown = visible_slugs(page)
            check(
                "typing the visible label finds exactly that row",
                shown == [slug_b],
                "got={}".format(shown),
            )
            page.evaluate(f"() => {{ {SH}.projectQuery = ''; }}")
            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('li.project'))"
                "  .filter(e => e.offsetParent !== null).length === 3",
                timeout=10000,
            )
            # Filtering tears the `x-for` down and rebuilds it, which is the same
            # unpainted-icon defect arriving by a second route: a loader-side fix
            # alone leaves the list blank after one keystroke.
            page.wait_for_timeout(150)
            after = page.evaluate(
                "() => ({ placeholders: document.querySelectorAll('li.project i[data-lucide]').length,"
                "  chevrons: document.querySelectorAll('li.project .chevron svg').length,"
                "  chips: document.querySelectorAll('li.project .branch-chip svg').length })"
            )
            check(
                "the icons survive a filter re-render",
                after["chevrons"] == 3 and after["chips"] == 3 and after["placeholders"] == 0,
                "got={}".format(after),
            )

            # --- scenarios 4 + 5: one line, truncated, and WIDE ---------------
            r_c = row(page, slug_c)
            check(
                "the long name stays on one line and truncates",
                bool(r_c)
                and r_c["laid"]
                and r_c["width"] > 0
                and r_c["height"] <= 24
                and r_c["scrollWidth"] > r_c["width"]
                and r_c["ellipsis"] == "ellipsis",
                "got={}".format(r_c),
            )
            check(
                "the name reclaims the width the branch chip used to take",
                bool(r_c) and r_c["laid"] and r_c["width"] > 150,
                "width={} (the 48% chip cap used to leave ~40)".format(r_c and r_c["width"]),
            )

            # --- scenario 6: one left edge for the column ---------------------
            edges = page.evaluate(
                "() => { const px = (sel, prop) => { const e = document.querySelector(sel);"
                "   if (!e || e.offsetParent === null || e.clientWidth === 0) return null;"
                "   return getComputedStyle(e)[prop]; };"
                " return { head: px('.projects-view .side-head', 'paddingLeft'),"
                "   search: px('.side-search', 'marginLeft'),"
                "   row: px('.project-head', 'paddingLeft') }; }"
            )
            check(
                "the section head, the search box and the rows share one left edge",
                all(edges.values()) and len(set(edges.values())) == 1,
                "got={}".format(edges),
            )

            # --- scenario 7: the chip's new home ------------------------------
            check(
                "the collapsed row carries no chip but still names its branch",
                bool(r_c) and r_c["chipsInRow"] == 0 and BRANCH_LONG in (r_c["rowTitle"] or ""),
                "chips={} title={}".format(
                    r_c and r_c["chipsInRow"], r_c and r_c["rowTitle"]
                ),
            )
            # The slug rides as an ARGUMENT, never interpolated: a repo
            # registered from a Windows path carries backslashes a string
            # literal would swallow as escapes (#316).
            page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug_c)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug_c, timeout=15000)
            page.wait_for_function(
                "() => { const c = document.querySelector('li.project.open .files-sec .branch-chip');"
                "  return !!c && c.offsetParent !== null && c.clientWidth > 0; }",
                timeout=15000,
            )
            chip = page.evaluate(
                "() => { const c = document.querySelector('li.project.open .files-sec .branch-chip');"
                "  const n = c.querySelector('.branch-chip-name');"
                "  return { laid: c.offsetParent !== null && c.clientWidth > 0,"
                "    name: n.textContent.trim(), transform: getComputedStyle(n).textTransform,"
                "    glyphs: c.querySelectorAll('svg').length,"
                "    inside: c.getBoundingClientRect().right"
                "      <= document.querySelector('.side').getBoundingClientRect().right + 0.5 }; }"
            )
            check(
                "the open project's branch reads in its own case, inside the bar",
                chip["laid"]
                and chip["name"] == BRANCH_LONG
                and chip["transform"] == "none"
                and chip["glyphs"] == 1
                and chip["inside"],
                "got={}".format(chip),
            )
            page.evaluate("() => document.querySelector('li.project.open .files-sec .branch-chip').click()")
            page.wait_for_function(f"() => {SH}.branchOpen === true", timeout=10000)
            picked = page.evaluate(f"() => {SH}.branchModal.slug")
            check(
                "clicking it still opens the picker on that project",
                picked == slug_c,
                "got={}".format(picked),
            )
            page.evaluate(f"() => {{ {SH}.branchOpen = false; }}")
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug_c)
            page.wait_for_function(f"() => {SH}.openSlug === null", timeout=10000)

            # --- scenario 8: the same under a squeezed sidebar ----------------
            set_side_w(page, SQUEEZED_SIDE_W)
            side_w = page.evaluate("() => document.querySelector('.side').getBoundingClientRect().width")
            r_c_sq = row(page, slug_c)
            check(
                "at a 160px sidebar the name still truncates on one line",
                side_w < 200
                and r_c_sq
                and r_c_sq["laid"]
                and r_c_sq["width"] > 0
                and r_c_sq["height"] <= 24
                and r_c_sq["scrollWidth"] > r_c_sq["width"],
                "side={} got={}".format(side_w, r_c_sq),
            )
            set_side_w(page, None)

            shot = os.path.join(SHOT_DIR, "332-projects-list-2026-07-27.png")
            page.screenshot(path=shot)
            print(f"[INFO] screenshot {shot}", flush=True)

            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
