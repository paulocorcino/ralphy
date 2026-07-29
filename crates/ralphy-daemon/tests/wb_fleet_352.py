"""#352 browser acceptance: execution and environment-aware fleet surfaces."""

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
HELPER = REPO_ROOT / "target" / "debug" / (
    "session_test_child.exe" if os.name == "nt" else "session_test_child"
)
SHOT = REPO_ROOT / "docs" / "screenshots" / "352-local-fleet-awareness-2026-07-29.png"
LOCAL_PORT = 7452
PEER_PORT = 7453
LOCAL_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAY"
PEER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAZ"
SLUG = "ralphy-lab/shared-repo"
PEER_REF = f"{PEER_ID}/{SLUG}"
PEER_ENV = "WSL: Ubuntu-22.04"
SHELL = "Alpine.$data(document.querySelector('[x-data]'))"
results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def assert_port_free(port):
    sock = socket.socket()
    try:
        sock.bind(("127.0.0.1", port))
    except OSError as error:
        raise RuntimeError(f"port {port} is occupied: {error}") from error
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
    if proc is None or proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def seed_repo(prefix):
    root = Path(tempfile.mkdtemp(prefix=prefix)) / "shared-repo"
    root.mkdir()
    (root / "README.md").write_text(f"# {prefix}\n", encoding="utf-8")
    git(root, "init", "-b", "main")
    git(root, "config", "user.email", "wb352@example.com")
    git(root, "config", "user.name", "wb352")
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


def register(store, repo):
    subprocess.run(
        [str(EXE), "daemon", "add", str(repo)],
        env=dict(os.environ, RALPHY_DAEMON_DIR=str(store)),
        check=True,
        capture_output=True,
        encoding="utf-8",
    )


def daemon_env(store, empty_home, token=None, peer=False):
    empty_usage = tempfile.mkdtemp(prefix="wb352_usage_")
    env = dict(
        os.environ,
        RALPHY_DAEMON_DIR=str(store),
        RALPHY_DAEMON_AGENT_OVERRIDE=str(HELPER),
        RALPHY_USAGE_DIR=empty_usage,
        RALPHY_CLAUDE_PROJECTS_DIR=empty_usage,
        RALPHY_CODEX_DIR=empty_usage,
        RALPHY_OPENCODE_DB=os.path.join(empty_usage, "none.db"),
        RALPHY_KIMI_DIR=empty_usage,
        RALPHY_KIMI_CODE_DIR=empty_usage,
        RALPHY_COPILOT_DB=os.path.join(empty_usage, "copilot-none.db"),
        RALPHY_CURSOR_DIR=empty_usage,
        RALPHY_GEMINI_DIR=empty_usage,
    )
    if token:
        env["RALPHY_DAEMON_TOKEN"] = token
    if peer:
        env["WSL_DISTRO_NAME"] = "Ubuntu-22.04"
        env["PATH"] = ""
        env["USERPROFILE"] = str(empty_home)
        env["HOME"] = str(empty_home)
        env["LOCALAPPDATA"] = str(empty_home)
        env["ProgramFiles"] = str(empty_home)
        env["ProgramFiles(x86)"] = str(empty_home)
    else:
        env.pop("WSL_DISTRO_NAME", None)
    return env


def launch(store, port, empty_home, peer_store=None, token=None, peer=False):
    argv = [str(EXE), "daemon", "--port", str(port)]
    if peer_store:
        argv.extend(["--peer-store", str(peer_store)])
    return subprocess.Popen(
        argv,
        env=daemon_env(store, empty_home, token, peer),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def main():
    local_proc = None
    peer_proc = None
    try:
        assert_port_free(LOCAL_PORT)
        assert_port_free(PEER_PORT)
        subprocess.run(
            [
                "cargo",
                "build",
                "-p",
                "ralphy-cli",
                "--bin",
                "ralphy",
                "-p",
                "ralphy-daemon",
                "--bin",
                "session_test_child",
            ],
            cwd=REPO_ROOT,
            check=True,
        )

        local_store = Path(tempfile.mkdtemp(prefix="wb352_local_store_"))
        peer_store = Path(tempfile.mkdtemp(prefix="wb352_peer_store_"))
        empty_home = Path(tempfile.mkdtemp(prefix="wb352_empty_home_"))
        local_repo = seed_repo("wb352-local")
        peer_repo = seed_repo("wb352-peer")
        baptize(local_store, LOCAL_ID, "local-daemon")
        baptize(peer_store, PEER_ID, "peer-daemon")
        register(local_store, local_repo)
        register(peer_store, peer_repo)

        peer_proc = launch(
            peer_store,
            PEER_PORT,
            empty_home,
            peer_store=local_store,
            token="peer-token-352",
            peer=True,
        )
        if not wait_listening(PEER_PORT) or peer_proc.poll() is not None:
            raise RuntimeError("peer daemon did not start")
        local_proc = launch(local_store, LOCAL_PORT, empty_home)
        if not wait_listening(LOCAL_PORT) or local_proc.poll() is not None:
            raise RuntimeError("local daemon did not start")

        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(
                headless=True, args=["--disable-webgl", "--disable-gpu"]
            )
            page = browser.new_page(viewport={"width": 1500, "height": 950})
            thrown = []
            page.on("pageerror", lambda error: thrown.append(str(error)))
            page.goto(f"http://127.0.0.1:{LOCAL_PORT}/", wait_until="networkidle")
            page.wait_for_function(
                "() => document.querySelectorAll('li.project').length === 2",
                timeout=20000,
            )

            peer_row = page.locator("li.project.peer").first
            peer_row.locator(".project-head").click()
            page.wait_for_function(f"(ref) => {SHELL}.openSlug === ref", arg=PEER_REF)
            page.wait_for_function(
                f"() => {SHELL}.roster.some(r => r.id === 'opencode' && r.available === false)",
                timeout=10000,
            )

            page.locator("button[title='Runs']").click()
            page.wait_for_selector(".runs-actions", state="visible")
            controls = page.locator(".runs-actions .run-verb")
            check(
                "peer run, triage, and push controls are enabled",
                controls.count() == 3
                and all(not controls.nth(i).is_disabled() for i in range(controls.count())),
            )
            page.locator("button[title='Runs']").click()
            page.wait_for_selector("aside.runs", state="hidden")

            page.locator(".tab[data-tab='consoles'], .tab").filter(
                has_text="Consoles"
            ).first.click()
            page.get_by_role("button", name="New console").click()
            page.wait_for_selector(
                ".console-choice .dropdown-item[title='not installed here']",
                state="visible",
            )
            unavailable = page.locator(".console-choice").filter(
                has=page.locator(".dropdown-item[title='not installed here']")
            ).first
            check(
                "unavailable peer roster row states the reason and keeps try-anyway",
                unavailable.locator(".dropdown-item").is_disabled()
                and unavailable.locator(".dropdown-item").get_attribute("title")
                == "not installed here"
                and unavailable.locator(".row-try").is_visible(),
            )
            page.get_by_role("button", name="New console").click()

            page.evaluate(
                "(repo) => window.WBConsole.open({repo, plain: true})", SLUG
            )
            page.evaluate(
                "(repo) => window.WBConsole.open({repo, plain: true})", PEER_REF
            )
            page.wait_for_function(
                "() => document.querySelectorAll('.session-window').length === 2"
            )
            page.wait_for_function(
                "(env) => [...document.querySelectorAll('.session-title')].some(e => e.textContent.includes(env))",
                arg=PEER_ENV,
                timeout=15000,
            )
            titles = page.locator(".session-title").all_text_contents()
            check(
                "peer free-console title names its effective environment",
                any(PEER_ENV in title and PEER_REF in title for title in titles),
                f"titles={titles}",
            )

            page.wait_for_timeout(500)
            desk = page.evaluate("async () => await (await fetch('/api/desk')).json()")
            refs = [record.get("repo") for record in desk.get("windows", [])]
            check(
                "desk keeps same-slug local and peer windows distinct",
                SLUG in refs and PEER_REF in refs and refs.count(SLUG) == 1 and refs.count(PEER_REF) == 1,
                f"refs={refs}",
            )

            stop(peer_proc)
            peer_proc = None
            page.evaluate(f"async () => await {SHELL}.openUsage()")
            page.wait_for_function(
                "(env) => [...document.querySelectorAll('.usage-missing strong')].some(e => e.textContent.includes(env))",
                arg=PEER_ENV,
                timeout=10000,
            )
            missing_text = page.locator(".usage-missing").inner_text()
            check(
                "missing usage contribution names the peer environment",
                PEER_ENV in missing_text and "Missing contributions" in missing_text,
                missing_text,
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
        stop(local_proc)
        stop(peer_proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if len(results) != 6:
        print(f"[FAIL] expected 6 checks, ran {len(results)}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
