"""#320 browser acceptance: the operator publishes the branch.

One Playwright pass over a REAL daemon proving that the Push button in the
Changes panel's footer bar really publishes — the assertion is made against the
BARE remote's own refs, not against anything the UI says about itself — and that
the two refusals a mis-click can produce arrive as the core's prose instead of
as a silent no-op or a force-push.

Scenario a  a feature branch with no upstream is unpublished; clicking Push
            lands its exact commit on the remote and the row's counts settle
Scenario b  clicking Push again is a no-op that flashes nothing (already level)
Scenario c  on the remote's DEFAULT branch the click is refused, the flash says
            "default branch", and the remote's ref is byte-identical afterwards
Scenario d  a remote that moved on refuses with "pull first" and the other
            side's commit survives — nothing here force-pushes
Scenario e  the page throws nothing across all of it

Every "remote" is a LOCAL bare directory cloned by path — no network.

Boots a Localhost daemon on 7417 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/320-push-2026-07-26.png.
Run: python crates/ralphy-daemon/tests/wb_push_320.py   (exit 0 = all pass)
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

PORT = 7417
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_push_320.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
)
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def info(name, detail):
    print(f"[INFO] {name} {detail}", flush=True)


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
    empty = tempfile.mkdtemp(prefix="wb320_empty_")
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


def git_out(cwd, *args):
    r = subprocess.run(
        ["git", *args], cwd=cwd, check=False, capture_output=True, encoding="utf-8"
    )
    return r.stdout.strip() if r.returncode == 0 else None


def seed(d, tag):
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text(f"# {tag}\n\nThe #320 push fixture repo.\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb320@example.com")
    git(d, "config", "user.name", "wb320")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return p


def make_bare_remote():
    """A BARE remote: a non-bare one refuses an update to its own checked-out
    branch, which would make every push below fail for the wrong reason."""
    source = tempfile.mkdtemp(prefix="wb320_src_")
    seed(source, "source")
    bare = tempfile.mkdtemp(prefix="wb320_remote_")
    git(bare, "clone", "--quiet", "--bare", source, bare)
    return bare


def clone_of(remote, prefix):
    d = tempfile.mkdtemp(prefix=prefix)
    git(d, "clone", "--quiet", remote, d)
    git(d, "config", "user.email", "wb320@example.com")
    git(d, "config", "user.name", "wb320")
    return d


def commit_in(d, name, body):
    (Path(d) / name).write_text(body, encoding="utf-8")
    git(d, "add", "-A")
    git(d, "commit", "-m", f"add {name}")


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir],
        env=env,
        check=True,
        capture_output=True,
        encoding="utf-8",
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's sidebar.
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True
    )


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# The Changes view (#317) as the one `x-show`-gated surface everything below
# waits on, exactly as the #316 script does.
OPEN_VIEW = (
    "(() => { const v = document.querySelector('.changes-view');"
    " return v && v.offsetParent !== null ? v : null; })()"
)


def open_changes(page, slug):
    # The slug rides as an ARGUMENT, never interpolated into the source: a slug
    # carrying Windows backslashes would be swallowed as escapes by a literal.
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    page.evaluate(
        "() => { const v = document.querySelector('.changes-view');"
        " if (!v || v.offsetParent === null)"
        "   document.querySelector('nav.rail button[title=\"Changes\"]').click(); }"
    )
    page.wait_for_function(
        f"() => {{ const v = {OPEN_VIEW}; if (!v) return false;"
        " const b = v.querySelector('[data-act=\"push\"]');"
        " return !!b && b.offsetParent !== null; }",
        timeout=15000,
    )


def flash(page):
    return page.evaluate(f"() => {SH}.runsActionMsg || ''")


def click_push_and_settle(page):
    page.evaluate(f"() => {{ {SH}.runsActionMsg = ''; }}")
    page.click('[data-act="push"]:visible')
    # The click spawns a child process; there is no completion event to await,
    # so this settles on the flash OR on a quiet window — and the assertions
    # downstream are made against the remote's refs, not against the wait.
    deadline = time.time() + 20
    while time.time() < deadline:
        if flash(page):
            break
        time.sleep(0.25)
    page.wait_for_timeout(800)
    return flash(page)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb320_reg_")
    remote = make_bare_remote()
    ours = clone_of(remote, "wb320_ours_")
    theirs = clone_of(remote, "wb320_theirs_")
    slug_ours = register_fixture(daemon_dir, ours)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)

            # --- scenario a: publishing an unpublished branch -----------------
            git(ours, "checkout", "--quiet", "-b", "feat/publish-me")
            commit_in(ours, "work.txt", "work\n")
            local_sha = git_out(ours, "rev-parse", "HEAD")
            check(
                "the branch is unpublished before the click",
                git_out(remote, "rev-parse", "--verify", "--quiet", "refs/heads/feat/publish-me")
                is None,
            )
            open_changes(page, slug_ours)
            msg = click_push_and_settle(page)
            landed = git_out(remote, "rev-parse", "--verify", "--quiet", "refs/heads/feat/publish-me")
            check(
                "clicking Push landed the branch on the remote",
                landed is not None,
                f"remote ref={landed}",
            )
            check(
                "…carrying exactly the local commit",
                landed == local_sha,
                f"remote={landed} local={local_sha}",
            )
            check("…and flashed no refusal", msg == "", f"flash={msg!r}")
            check(
                "…and the push set an upstream",
                git_out(ours, "rev-parse", "--abbrev-ref", "@{upstream}")
                == "origin/feat/publish-me",
                f"upstream={git_out(ours, 'rev-parse', '--abbrev-ref', '@{upstream}')}",
            )

            # --- scenario b: a second click has nothing to publish ------------
            before = git_out(remote, "rev-parse", "refs/heads/feat/publish-me")
            msg = click_push_and_settle(page)
            check(
                "a second Push is a quiet no-op, not a refusal",
                msg == "",
                f"flash={msg!r}",
            )
            check(
                "…and moved nothing on the remote",
                git_out(remote, "rev-parse", "refs/heads/feat/publish-me") == before,
            )

            # --- scenario c: the protected ref --------------------------------
            git(ours, "checkout", "--quiet", "main")
            commit_in(ours, "on-main.txt", "main work\n")
            main_before = git_out(remote, "rev-parse", "refs/heads/main")
            page.evaluate(f"(s) => {SH}.loadSync(s)", arg=slug_ours)
            page.wait_for_timeout(1200)
            msg = click_push_and_settle(page)
            check(
                "pushing the remote's default branch is refused",
                "default branch" in msg,
                f"flash={msg!r}",
            )
            check(
                "…and the remote's main is byte-identical afterwards",
                git_out(remote, "rev-parse", "refs/heads/main") == main_before,
                f"before={main_before} after={git_out(remote, 'rev-parse', 'refs/heads/main')}",
            )

            # --- scenario d: a remote that moved on ---------------------------
            # The other side publishes a commit ours has never seen…
            git(theirs, "fetch", "--quiet", "origin")
            git(theirs, "checkout", "--quiet", "-b", "feat/publish-me", "origin/feat/publish-me")
            commit_in(theirs, "theirs.txt", "theirs\n")
            git(theirs, "push", "--quiet", "origin", "feat/publish-me")
            theirs_sha = git_out(remote, "rev-parse", "refs/heads/feat/publish-me")
            # …and ours commits on top of the stale tip.
            git(ours, "checkout", "--quiet", "feat/publish-me")
            commit_in(ours, "ours-2.txt", "ours\n")
            page.evaluate(f"(s) => {SH}.loadSync(s)", arg=slug_ours)
            page.wait_for_timeout(1200)
            msg = click_push_and_settle(page)
            check(
                "a remote that moved on is refused with its own advice",
                "pull first" in msg,
                f"flash={msg!r}",
            )
            check(
                "…and the other side's commit is still the remote tip (no force-push)",
                git_out(remote, "rev-parse", "refs/heads/feat/publish-me") == theirs_sha,
                f"tip={git_out(remote, 'rev-parse', 'refs/heads/feat/publish-me')}",
            )

            page.screenshot(path=os.path.join(SHOT_DIR, "320-push-2026-07-26.png"))
            check("…and the panel threw nothing", not thrown, f"pageerrors={thrown}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    info("fixtures", f"remote={remote} ours={ours} theirs={theirs}")
    passed = sum(1 for r in results if r)
    print(f"\n{passed}/{len(results)} checks passed")
    if passed != len(results):
        sys.exit(1)
    print("PUSH LIVE")


if __name__ == "__main__":
    main()
