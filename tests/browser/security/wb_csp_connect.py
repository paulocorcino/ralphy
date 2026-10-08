"""The CSP `connect-src` names only the daemon's own origin, in every engine
and behind every tunnel the operator uses.

The daemon sends `connect-src 'self'`. Every workbench socket is built from
`location.host`, so `'self'` is the whole set. This script checks, in
Chromium, Firefox and WebKit, on three ways to reach the daemon:

  loopback     http://127.0.0.1:<port>
  ngrok        https://<random>.ngrok-free.app (a declared `--allowed-host`;
               ngrok keeps `Host` and `Origin`)
  dev tunnels  https://<id>-<port>.<region>.devtunnels.ms (dev tunnels
               rewrites `Host` and `Origin` to localhost)

that (1) the shell boots, (2) a workbench socket to the page's own origin
opens, (3) a socket to another host is refused, and (4) the page records no
other CSP violation.

The daemon runs over a SCRATCH `RALPHY_DAEMON_DIR`. A tunnel publishes it to
the internet, so the script arms TOTP and require-login in that scratch store
BEFORE any tunnel starts, and every browser logs in. The daemon and the
tunnels are stopped by their own subprocess handles, never by name.

Needs `ngrok` and `devtunnel` on PATH and logged in. `WB_CSP_TARGETS` picks
the targets (default `loopback,ngrok,devtunnel`).

Run: python tests/browser/security/wb_csp_connect.py   (exit 0 = all pass)
"""

import base64
import hashlib
import hmac
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7411
LOCAL = f"http://127.0.0.1:{PORT}"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
ENGINES = ("chromium", "firefox", "webkit")
TARGETS = os.environ.get("WB_CSP_TARGETS", "loopback,ngrok,devtunnel").split(",")

# Records every CSP violation and every socket open, before any page script runs.
RECORDER = """
window.__csp = [];
window.__open = [];
document.addEventListener('securitypolicyviolation', (e) => {
  window.__csp.push(e.violatedDirective + ' ' + (e.blockedURI || ''));
});
const Native = window.WebSocket;
window.WebSocket = function (url, protocols) {
  const ws = protocols === undefined ? new Native(url) : new Native(url, protocols);
  ws.addEventListener('open', () => window.__open.push(String(url)));
  return ws;
};
window.WebSocket.prototype = Native.prototype;
Object.assign(window.WebSocket, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 });
"""

# Tries a socket to `url`; resolves to how it ended. A CSP refusal is either a
# thrown SecurityError (Firefox, WebKit) or an error event plus a violation.
FOREIGN = """
(url) => new Promise((resolve) => {
  let ws;
  try { ws = new WebSocket(url); } catch (e) { resolve('threw ' + e.name); return; }
  const done = (how) => { try { ws.close(); } catch (_) {} resolve(how); };
  ws.addEventListener('open', () => done('open'));
  ws.addEventListener('error', () => done('error'));
  setTimeout(() => done('timeout'), 8000);
})
"""

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


# ── TOTP (RFC 6238, the parameters the enrol URI names) ─────────────────────
class Totp:
    def __init__(self, uri):
        q = urllib.parse.parse_qs(urllib.parse.urlparse(uri).query)
        secret = q["secret"][0].upper()
        self.key = base64.b32decode(secret + "=" * (-len(secret) % 8))
        self.digits = int(q.get("digits", ["6"])[0])
        self.period = int(q.get("period", ["30"])[0])
        self.algo = q.get("algorithm", ["SHA1"])[0].lower()
        self.last = None

    def at(self, step):
        mac = hmac.new(self.key, struct.pack(">Q", step), getattr(hashlib, self.algo)).digest()
        off = mac[-1] & 0x0F
        num = struct.unpack(">I", mac[off : off + 4])[0] & 0x7FFFFFFF
        return str(num % 10**self.digits).zfill(self.digits)

    def fresh(self):
        """A code for a step newer than the last one used. The daemon accepts
        one step ahead and refuses a replayed step, so this waits when needed."""
        while True:
            now = int(time.time()) // self.period
            step = now - 1 if self.last is None else self.last + 1
            step = max(step, now - 1)
            if step <= now + 1:
                self.last = step
                return self.at(step)
            time.sleep(1)


def post_form(base, path, data):
    req = urllib.request.Request(base + path, data=urllib.parse.urlencode(data).encode(), method="POST")
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read() or b"null")


def wait_listening(url, timeout=25, headers=None):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(urllib.request.Request(url, headers=headers or {}), timeout=3)
            return True
        except urllib.error.HTTPError:
            return True
        except Exception:
            time.sleep(0.5)
    return False


def stop(proc):
    if proc is None or proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def scratch_env(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wbcsp_empty_")
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
    d = tempfile.mkdtemp(prefix="wbcsp_repo_")
    with open(os.path.join(d, "README.md"), "w", encoding="utf-8") as f:
        f.write("# csp\n")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbcsp@example.com"],
        ["git", "config", "user.name", "wbcsp"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    return d


# ── tunnels ─────────────────────────────────────────────────────────────────
def start_ngrok():
    proc = subprocess.Popen(
        ["ngrok", "http", str(PORT), "--log", "stdout"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    deadline = time.time() + 30
    while time.time() < deadline:
        try:
            with urllib.request.urlopen("http://127.0.0.1:4040/api/tunnels", timeout=2) as r:
                for t in json.loads(r.read())["tunnels"]:
                    if t["public_url"].startswith("https://"):
                        return proc, t["public_url"]
        except Exception:
            pass
        time.sleep(0.5)
    stop(proc)
    return None, None


def create_devtunnel():
    """Creates the tunnel this run hosts; returns its id, or None. The id is
    what `delete_devtunnel` removes, so no other tunnel is ever touched. A
    tunnel that exists but whose JSON cannot be parsed is still returned when
    its id can be read from the text, so the caller deletes it."""
    out = subprocess.run(
        ["devtunnel", "create", "--allow-anonymous", "--json"], capture_output=True, encoding="utf-8", errors="replace"
    )
    text = (out.stdout or "") + (out.stderr or "")
    if out.returncode != 0:
        if re.search(r"rate limit|too many tunnels|limit exceeded", text, re.I):
            check("dev tunnel created", False, "the account's dev tunnel limit is the cause, not the CSP: " + text.strip()[:300])
        else:
            check("dev tunnel created", False, text.strip()[:300])
        return None
    try:
        # A welcome banner can come before the JSON.
        return json.loads(out.stdout[out.stdout.index("{") :])["tunnel"]["tunnelId"]
    except Exception as e:
        found = re.search(r'"tunnelId"\s*:\s*"([^"]+)"', text)
        check("dev tunnel created", False, f"no tunnel id in the output: {e!r} {text.strip()[:300]}")
        return found.group(1) if found else None


def add_devtunnel_port(tunnel_id):
    """`host` cannot add a port to an existing tunnel."""
    port = subprocess.run(["devtunnel", "port", "create", tunnel_id, "-p", str(PORT)], capture_output=True)
    ok = port.returncode == 0
    check("dev tunnel port added", ok, "" if ok else port.stdout.decode(errors="replace")[:300])
    return ok


def delete_devtunnel(tunnel_id):
    try:
        done = subprocess.run(["devtunnel", "delete", tunnel_id, "-f"], capture_output=True, timeout=60)
        ok, detail = done.returncode == 0, done.stdout.decode(errors="replace")[:300]
    except Exception as e:
        ok, detail = False, repr(e)
    check("dev tunnel deleted", ok, "" if ok else f"tunnel {tunnel_id} may be left behind: {detail}")


def start_devtunnel(tunnel_id):
    proc = subprocess.Popen(
        ["devtunnel", "host", tunnel_id],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        encoding="utf-8",
        errors="replace",
    )
    found = {}

    def read():
        for line in proc.stdout:
            if re.search(r"rate limit|too many tunnels|limit exceeded", line, re.I):
                found["limit"] = line.strip()
            m = re.search(r"https://\S+-%d\.\S+devtunnels\.ms/?" % PORT, line)
            if m and "url" not in found:
                found["url"] = m.group(0).rstrip("/")

    threading.Thread(target=read, daemon=True).start()
    deadline = time.time() + 45
    while time.time() < deadline and "url" not in found and "limit" not in found:
        time.sleep(0.5)
    if "url" not in found:
        stop(proc)
        if "limit" in found:
            check("dev tunnel hosted", False, "the account's dev tunnel limit is the cause, not the CSP: " + found["limit"])
        return None, None
    return proc, found["url"]


# Both providers put an anti-phishing page in front of a browser; these
# headers skip it.
SKIP_INTERSTITIAL = {"ngrok-skip-browser-warning": "1", "X-Tunnel-Skip-AntiPhishing-Page": "true"}


def run_engine(pw, engine, name, base, totp):
    browser = getattr(pw, engine).launch()
    try:
        ctx = browser.new_context(viewport={"width": 1300, "height": 850}, extra_http_headers=SKIP_INTERSTITIAL)
        ctx.add_init_script(RECORDER)
        page = ctx.new_page()
        tag = f"{engine} {name}"

        resp = ctx.request.post(base + "/api/login", form={"code": totp.fresh()})
        check(f"{tag}: login", resp.status == 200, f"status={resp.status}")

        page.goto(base + "/")
        page.wait_for_function("() => window.Alpine && document.querySelector('[x-data]')", timeout=30000)
        own = ("wss://" if base.startswith("https:") else "ws://") + urllib.parse.urlparse(base).netloc + "/"
        try:
            page.wait_for_function("(own) => window.__open.some(u => u.startsWith(own))", arg=own, timeout=20000)
        except Exception:
            pass  # the check below reports what opened
        opened = page.evaluate("window.__open")
        check(f"{tag}: a workbench socket to its own origin opens", any(u.startswith(own) for u in opened), opened)
        check(f"{tag}: no CSP violation from the workbench", page.evaluate("window.__csp") == [], page.evaluate("window.__csp"))

        # Another host, and the same host on another port (another origin).
        for foreign in ("wss://example.com/ws", f"ws://127.0.0.1:{PORT + 1}/ws"):
            before = len(page.evaluate("window.__csp"))
            how = page.evaluate(FOREIGN, foreign)
            v = page.evaluate("window.__csp")[before:]
            refused = how != "open" and (how.startswith("threw") or any("connect-src" in x for x in v))
            check(f"{tag}: a socket to {foreign} is refused", refused, f"{how} {v}")
    finally:
        browser.close()


def main():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") != "1":
        subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    daemon_dir = tempfile.mkdtemp(prefix="wbcsp_reg_")
    fixture = make_fixture_repo()
    reg = subprocess.run(
        [EXE, "daemon", "add", fixture],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        check=True,
        capture_output=True,
        encoding="utf-8",
    )
    check("fixture registered", reg.returncode == 0)

    tunnels = []
    devtunnel_id = None
    bases = []
    daemon = None
    try:
        if "loopback" in TARGETS:
            bases.append(("loopback", LOCAL))
        allowed = []
        if "ngrok" in TARGETS:
            proc, url = start_ngrok()
            check("ngrok tunnel started", url is not None, url or "")
            if url:
                tunnels.append(proc)
                bases.append(("ngrok", url))
                allowed += ["--allowed-host", urllib.parse.urlparse(url).hostname]
        daemon = subprocess.Popen(
            [EXE, "daemon", "--port", str(PORT), *allowed],
            env=scratch_env(daemon_dir),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        check("daemon listening", wait_listening(LOCAL + "/api/session"))

        # Arm login on the scratch store before any tunnel can reach it.
        uri = post_form(LOCAL, "/api/security/totp/enroll", {})["uri"]
        totp = Totp(uri)
        check("TOTP confirmed", post_form(LOCAL, "/api/security/totp/confirm", {"code": totp.fresh()})["confirmed"])
        check("require-login on", post_form(LOCAL, "/api/security/require-login", {"enable": "true"})["ok"])
        try:
            urllib.request.urlopen(LOCAL + "/api/projects", timeout=5)
            gated = False
        except urllib.error.HTTPError as e:
            gated = e.code in (401, 403)
        check("the scratch daemon now refuses a request with no session", gated)

        if "devtunnel" in TARGETS:
            # Held before the port call, so `finally` deletes it on any failure.
            devtunnel_id = create_devtunnel()
            ready = bool(devtunnel_id) and add_devtunnel_port(devtunnel_id)
            proc, url = start_devtunnel(devtunnel_id) if ready else (None, None)
            check("dev tunnel started", url is not None, url or "")
            if url:
                tunnels.append(proc)
                bases.append(("devtunnel", url))

        for name, base in bases:
            if name != "loopback":
                check(f"{name} reachable", wait_listening(base + "/api/session", 30, SKIP_INTERSTITIAL), base)
        with sync_playwright() as pw:
            for engine in ENGINES:
                for name, base in bases:
                    try:
                        run_engine(pw, engine, name, base, totp)
                    except Exception as e:
                        check(f"{engine} {name}: ran", False, repr(e)[:300])
    finally:
        for t in tunnels:
            stop(t)
        stop(daemon)
        if devtunnel_id:
            delete_devtunnel(devtunnel_id)

    print(f"\n{sum(results)}/{len(results)} checks passed")
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
