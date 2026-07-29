"""#350 browser acceptance: federated repo workbench surfaces."""

import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[3]
EXE = REPO_ROOT / "target" / "debug" / ("ralphy.exe" if os.name == "nt" else "ralphy")
SHOT = REPO_ROOT / "docs" / "screenshots" / "350-federated-workbench-2026-07-29.png"
A_PORT = 7442
B_PORT = 7443
A_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
B_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
SLUG = "ralphy-lab/shared-repo"
PEER_REF = f"{B_ID}/{SLUG}"
SH = "Alpine.$data(document.querySelector('[x-data]'))"
results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def assert_port_free(port):
    sock = socket.socket()
    try:
        sock.bind(("127.0.0.1", port))
    except OSError as error:
        raise RuntimeError(f"port {port} is already occupied: {error}") from error
    finally:
        sock.close()


def wait_listening(port, timeout=25):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=1)
            return True
        except Exception:
            time.sleep(0.25)
    return False


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def seed_repo(prefix, content):
    root = Path(tempfile.mkdtemp(prefix=prefix)) / "shared-repo"
    root.mkdir()
    (root / "note.txt").write_text(content, encoding="utf-8")
    git(root, "init", "-b", "main")
    git(root, "config", "user.email", "wb350@example.com")
    git(root, "config", "user.name", "wb350")
    git(root, "remote", "add", "origin", f"https://github.com/{SLUG}.git")
    git(root, "add", "-A")
    git(root, "commit", "-m", "fixture")
    return root


def baptize(store, daemon_id, name):
    store.mkdir(parents=True, exist_ok=True)
    (store / "daemon.toml").write_text(
        f'id = "{daemon_id}"\nname = "{name}"\navatar = "🐙"\n',
        encoding="utf-8",
    )


def sparse_env(store, token=None):
    empty = tempfile.mkdtemp(prefix="wb350_empty_")
    env = dict(
        os.environ,
        RALPHY_DAEMON_DIR=str(store),
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )
    if token:
        env["RALPHY_DAEMON_TOKEN"] = token
    return env


def register(store, repo):
    subprocess.run(
        [str(EXE), "daemon", "add", str(repo)],
        env=dict(os.environ, RALPHY_DAEMON_DIR=str(store)),
        check=True,
        capture_output=True,
        encoding="utf-8",
    )


def launch(store, port, peer_store=None, token=None):
    argv = [str(EXE), "daemon", "--port", str(port)]
    if peer_store:
        argv.extend(["--peer-store", str(peer_store)])
    return subprocess.Popen(
        argv,
        env=sparse_env(store, token),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def open_row(page, peer):
    selector = "li.project.peer" if peer else "li.project:not(.peer)"
    page.locator(selector).first.locator(".project-head").click()


def wait_tree(page, title):
    page.wait_for_function(
        f"(t) => !!{SH}._tree && !!{SH}._tree.findFirst(n => n.title === t)",
        arg=title,
        timeout=20000,
    )


def open_file(page, title):
    wait_tree(page, title)
    page.evaluate(
        f"(t) => {SH}.openFile({SH}._tree.findFirst(n => n.title === t))",
        arg=title,
    )


def main():
    try:
        assert_port_free(A_PORT)
        assert_port_free(B_PORT)
    except RuntimeError as error:
        print(f"[FAIL] {error}", flush=True)
        sys.exit(1)

    subprocess.run(
        ["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"],
        cwd=REPO_ROOT,
        check=True,
    )
    a_store = Path(tempfile.mkdtemp(prefix="wb350_a_"))
    b_store = Path(tempfile.mkdtemp(prefix="wb350_b_"))
    a_repo = seed_repo("wb350_local_", "local-side")
    b_repo = seed_repo("wb350_peer_", "peer-side")
    baptize(a_store, A_ID, "local-daemon")
    baptize(b_store, B_ID, "peer-daemon")
    register(a_store, a_repo)
    register(b_store, b_repo)

    b_proc = launch(b_store, B_PORT, peer_store=a_store, token="peer-tok")
    a_proc = None
    try:
        if not wait_listening(B_PORT) or b_proc.poll() is not None:
            raise RuntimeError(f"daemon B did not start on port {B_PORT}")
        a_proc = launch(a_store, A_PORT)
        if not wait_listening(A_PORT) or a_proc.poll() is not None:
            raise RuntimeError(f"daemon A did not start on port {A_PORT}")

        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(
                headless=True, args=["--disable-webgl", "--disable-gpu"]
            )
            page = browser.new_page(viewport={"width": 1500, "height": 950})
            thrown = []
            page.on("pageerror", lambda error: thrown.append(str(error)))
            page.goto(f"http://127.0.0.1:{A_PORT}/", wait_until="domcontentloaded")
            page.wait_for_function(
                "() => document.querySelectorAll('li.project').length === 2",
                timeout=20000,
            )

            peer_row = page.locator("li.project.peer").first
            check(
                "peer environment group and repo row render",
                peer_row.is_visible()
                and peer_row.locator(".project-slug").get_attribute("title") == SLUG
                and "peer-daemon" in page.locator(".env-daemon").all_text_contents(),
            )

            open_row(page, True)
            page.wait_for_function(f"(ref) => {SH}.openSlug === ref", arg=PEER_REF)
            wait_tree(page, "note.txt")
            check(
                "clicking the peer row opens its file tree",
                page.evaluate(f"() => {SH}.openSlug") == PEER_REF,
            )

            open_file(page, "note.txt")
            page.wait_for_function(
                "() => window.monaco && monaco.editor.getModels().some(m => m.getValue() === 'peer-side')",
                timeout=20000,
            )
            check(
                "peer file bytes render in the editor",
                page.evaluate(
                    "() => monaco.editor.getModels().some(m => m.getValue() === 'peer-side')"
                ),
            )

            (b_repo / "changed.txt").write_text("changed on peer", encoding="utf-8")
            page.evaluate(f"() => {SH}.showSideView('changes')")
            page.wait_for_function(
                f"(ref) => ({SH}.changesUnstaged[ref] || []).some(c => c.path === 'changed.txt')",
                arg=PEER_REF,
                timeout=20000,
            )
            check(
                "Changes lists a file modified in the peer repo",
                page.evaluate(
                    f"(ref) => ({SH}.changesUnstaged[ref] || []).some(c => c.path === 'changed.txt')",
                    PEER_REF,
                ),
            )

            (b_repo / "fresh.txt").write_text("watch-created", encoding="utf-8")
            wait_tree(page, "fresh.txt")
            check(
                "peer create refreshes the open tree without reload",
                page.evaluate(
                    f"() => !!{SH}._tree.findFirst(n => n.title === 'fresh.txt')"
                ),
            )

            page.reload(wait_until="domcontentloaded")
            page.wait_for_function(
                "() => document.querySelectorAll('li.project').length === 2",
                timeout=20000,
            )
            page.locator("li.project:not(.peer)").first.locator(".project-head").click()
            page.wait_for_function(f"(slug) => {SH}.openSlug === slug", arg=SLUG)
            wait_tree(page, "note.txt")
            open_file(page, "note.txt")
            page.wait_for_function(
                "() => monaco.editor.getModels().some(m => m.getValue() === 'local-side')",
                timeout=20000,
            )
            check(
                "local collision still opens and reads local bytes",
                page.evaluate(
                    "() => monaco.editor.getModels().some(m => m.getValue() === 'local-side')"
                ),
            )

            SHOT.parent.mkdir(parents=True, exist_ok=True)
            page.screenshot(path=str(SHOT))
            print(f"[INFO] screenshot {SHOT}", flush=True)
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    except Exception as error:
        print(f"[FAIL] browser acceptance crashed: {error}", flush=True)
        results.append(False)
    finally:
        if a_proc is not None:
            stop(a_proc)
        stop(b_proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if len(results) != 7:
        print(f"[FAIL] expected 7 checks, ran {len(results)}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
