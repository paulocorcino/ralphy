"""Probe: an HTML file with scripts, served from a route of the daemon's own
host with a `sandbox allow-scripts` policy, in three engines.

Usage: python script_probe.py <folder that holds html-preview-test.html>
"""

import json
import mimetypes
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")
ROOT = Path(sys.argv[1]).resolve()
PORT = 7466
SELF = f"http://127.0.0.1:{PORT}"

# The policy of crates/ralphy-daemon/src/routes/headers.rs, without the hashes.
PAGE_CSP = (
    "default-src 'self'; script-src 'self' 'unsafe-eval'; "
    "style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; "
    "font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:; "
    "object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
)


def preview_csp(kind):
    prefix = f"{SELF}/preview/{kind}/"
    if kind == "loose":
        return "sandbox allow-scripts; frame-ancestors 'self'"
    images = f"img-src data: {prefix}" + (" https:" if kind == "web" else "")
    scripts = f"script-src 'unsafe-inline' 'unsafe-eval' {prefix}" + (
        " https://cdn.jsdelivr.net" if kind == "web" else ""
    )
    return (
        f"sandbox allow-scripts; default-src 'none'; {scripts}; "
        f"style-src 'unsafe-inline' {prefix}; {images}; font-src {prefix}; "
        "connect-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'"
    )


ATTACK = """<!doctype html><title>attack</title><pre id="out">{}</pre><script>
const out = {};
const note = (k, v) => { out[k] = String(v); document.getElementById('out').textContent = JSON.stringify(out); };
const attempt = (k, fn) => { try { note(k, fn()); } catch (e) { note(k, 'THROWS ' + e.name); } };
attempt('origin', () => self.origin);
attempt('cookie', () => JSON.stringify(document.cookie));
attempt('localStorage', () => { localStorage.setItem('a', 'b'); return 'works'; });
attempt('parent-dom', () => (top === self ? 'is top' : 'READ ' + parent.document.title));
attempt('window.open', () => (window.open('/api/hit/popup') ? 'OPENED' : 'null'));
if (top !== self) attempt('top-navigation', () => { top.location.href = '/api/hit/top-nav'; return 'no throw'; });
const send = (k, url, init) => fetch(url, init).then(
  (r) => note(k, 'sent, reply ' + r.type + ' ' + r.status), (e) => note(k, 'REFUSED ' + e.name));
send('fetch-get', '/api/hit/fetch-get', { credentials: 'include', mode: 'no-cors' });
send('fetch-post', '/api/hit/fetch-post', { method: 'POST', credentials: 'include', mode: 'no-cors',
  headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: 'enable=true' });
send('fetch-other-token', '/preview/other/secret.js', { credentials: 'include', mode: 'no-cors' });
attempt('xhr', () => { const x = new XMLHttpRequest(); x.open('POST', '/api/hit/xhr'); x.withCredentials = true;
  x.onload = () => note('xhr', 'sent'); x.onerror = () => note('xhr', 'REFUSED or no reply'); x.send('a=b'); return 'pending'; });
attempt('beacon', () => navigator.sendBeacon('/api/hit/beacon', 'a=b'));
attempt('websocket', () => { const w = new WebSocket('ws://' + location.host + '/api/hit/ws');
  w.onerror = () => note('websocket', 'error event'); return 'pending'; });
const el = (tag, attrs, k) => { const e = document.createElement(tag); Object.assign(e, attrs);
  e.onload = () => note(k, 'loaded'); e.onerror = () => note(k, 'error event'); document.body.append(e); note(k, 'pending'); };
el('img', { src: '/api/hit/img' }, 'img');
el('script', { src: '/api/hit/script.js' }, 'script-src');
el('script', { src: '/preview/__KIND__/../../api/hit/dotdot.js' }, 'script-dotdot');
el('link', { rel: 'stylesheet', href: '/api/hit/css' }, 'stylesheet');
import('/api/hit/import.js').then(() => note('import', 'loaded'), (e) => note('import', 'REFUSED ' + e.name));
</script>"""

ATTACK_NAV = """<!doctype html><title>attack-nav</title><p>nav</p><script>
try { const f = document.createElement('form'); f.method = 'post'; f.action = '/api/hit/form';
  document.body.append(f); f.submit(); } catch (e) {}
setTimeout(() => { location.href = '/api/hit/nav'; }, 500);
</script>"""

hits = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, body, ctype, csp, cookie=False):
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Security-Policy", csp)
        if cookie:
            self.send_header("Set-Cookie", "sess=strict; SameSite=Strict; HttpOnly; Path=/")
        self.end_headers()
        self.wfile.write(body)

    def handle_any(self):
        path = self.path.split("?")[0]
        parts = path.strip("/").split("/")
        if path.startswith("/api/hit/"):
            hits.append(
                {
                    "name": parts[2].split(".")[0],
                    "method": self.command,
                    "cookie": "sess=strict" in (self.headers.get("Cookie") or ""),
                    "origin": self.headers.get("Origin"),
                }
            )
            ctype = "text/javascript" if path.endswith(".js") else "text/plain"
            self.reply(b"/* hit */", ctype, PAGE_CSP)
        elif parts[0] == "host":
            frame = f"/preview/{parts[1]}/{'/'.join(parts[2:])}"
            body = (
                "<!doctype html><title>workbench</title><body style='margin:0'>"
                f"<iframe id=f style='width:1100px;height:1500px;border:0' src='{frame}'></iframe>"
            ).encode()
            self.reply(body, "text/html; charset=utf-8", PAGE_CSP, cookie=True)
        elif parts[0] == "preview":
            kind, rel = parts[1], "/".join(parts[2:])
            csp = preview_csp(kind if kind in ("tight", "web", "loose") else "tight")
            if rel == "attack.html":
                self.reply(ATTACK.replace("__KIND__", kind).encode(), "text/html; charset=utf-8", csp)
            elif rel == "attack-nav.html":
                self.reply(ATTACK_NAV.encode(), "text/html; charset=utf-8", csp)
            else:
                target = (ROOT / rel).resolve()
                if kind == "other" or ROOT not in target.parents or not target.is_file():
                    hits.append({"name": "outside-token", "method": self.command, "cookie": False, "origin": None})
                    self.reply(b"", "text/plain", csp)
                    return
                ctype = mimetypes.guess_type(target.name)[0] or "application/octet-stream"
                if target.suffix == ".js":
                    ctype = "text/javascript"
                self.reply(target.read_bytes(), ctype, csp)
        else:
            self.reply(b"", "text/plain", PAGE_CSP)

    do_GET = do_POST = handle_any


def frame_of(page):
    return next(f for f in page.frames if f is not page.main_frame)


def show_hits(label):
    seen = {}
    for h in hits:
        seen[h["name"]] = f"{h['method']} cookie={'YES' if h['cookie'] else 'no'} origin={h['origin']}"
    print(f"    reached the daemon ({label}): " + (json.dumps(seen) if seen else "nothing"))


def statuses(page, kind):
    hits.clear()
    page.goto(f"{SELF}/host/{kind}/html-preview-test.html", wait_until="domcontentloaded")
    page.wait_for_timeout(3000)
    frame = frame_of(page)
    try:
        frame.locator("button[onclick]").click(timeout=3000)
    except Exception as exc:
        print("    click E:", str(exc).splitlines()[0][:80])
    page.wait_for_timeout(300)
    got = frame.evaluate(
        "() => Object.fromEntries([...document.querySelectorAll('[data-status]')]"
        ".map(e => [e.dataset.status, e.textContent.trim().slice(0, 110)]))"
    )
    added = frame.evaluate(
        "() => Object.fromEntries([...document.querySelectorAll('[id^=slot-js] img')]"
        ".map(i => [i.alt, i.naturalWidth]))"
    )
    got["(width of the images the script added)"] = json.dumps(added)
    styled = frame.evaluate("() => getComputedStyle(document.querySelector('.grid')).display")
    print(f"  -- the test page, policy {kind}: style sheet applied={styled == 'grid'}")
    for key, value in got.items():
        print(f"      {key}: {value}")
    page.screenshot(path=str(Path(__file__).parent / f"script-{page.context.browser.browser_type.name}-{kind}.png"))


def attack(page, kind, top):
    hits.clear()
    where = f"{SELF}/preview/{kind}/attack.html" if top else f"{SELF}/host/{kind}/attack.html"
    page.goto(where, wait_until="domcontentloaded")
    page.wait_for_timeout(2500)
    doc = page.main_frame if top else frame_of(page)
    out = json.loads(doc.locator("#out").text_content())
    label = f"policy {kind}, {'own window' if top else 'frame'}"
    print(f"  -- hostile script, {label}")
    for key, value in out.items():
        print(f"      {key}: {value}")
    print(f"      page after: {page.url.replace(SELF, '')}")
    show_hits(label)
    hits.clear()
    page.goto(f"{SELF}/preview/{kind}/attack-nav.html" if top else f"{SELF}/host/{kind}/attack-nav.html")
    page.wait_for_timeout(1800)
    show_hits(label + ", form submit and navigation")


def main():
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    time.sleep(0.3)
    with sync_playwright() as pw:
        for engine in ("chromium", "firefox", "webkit"):
            browser = getattr(pw, engine).launch()
            print(f"\n===== {engine} {browser.version} =====")
            context = browser.new_context(viewport={"width": 1100, "height": 1500})
            web = []

            def stub(route):
                web.append(route.request.url.split("/")[2])
                if "jsdelivr" in route.request.url:
                    route.fulfill(status=200, content_type="text/javascript", body="window.confetti = function () {};")
                else:
                    route.fulfill(status=200, content_type="image/png", body=PNG)

            context.route("https://**", stub)
            page = context.new_page()
            page.set_default_timeout(15000)
            page.goto(f"{SELF}/host/tight/attack-nav.html")  # sets the cookie
            steps = [lambda k=k: (web.clear(), statuses(page, k), print(f"      web requests that left: {sorted(set(web))}")) for k in ("tight", "web")]
            steps += [lambda a=a: attack(page, *a) for a in (("tight", False), ("loose", False), ("loose", True), ("tight", True))]
            for step in steps:
                try:
                    step()
                except Exception as exc:
                    print("    STEP FAILED:", str(exc).splitlines()[0][:200])
            browser.close()


PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de"
    "0000000c4944415408d763f8cfc000000301010018dd8db00000000049454e44ae426082"
)
main()
