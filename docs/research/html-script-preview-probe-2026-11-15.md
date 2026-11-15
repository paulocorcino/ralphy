# An HTML file with scripts, served by a route: probe of 2026-11-15

Measured on Windows 11 with Playwright Chromium 148.0.7778.96, Firefox
150.0.2 and WebKit 26.4, on loopback over `http`. The script is
[html-script-preview-probe-2026-11-15.py](./html-script-preview-probe-2026-11-15.py).
It starts a test server on one port. The server stands for the daemon: it
sends the page policy of `crates/ralphy-daemon/src/routes/headers.rs`, sets
a `SameSite=Strict; HttpOnly` cookie, and records each request to
`/api/hit/<name>` with its method, its cookie and its `Origin`. No daemon
ran. A tunnel, `https` and a real Safari were not measured.

This probe grounds
[ADR-0089](../adr/0089-an-html-file-runs-its-scripts-only-when-the-operator-allows-it.md).

Run it with the folder that holds the test page `html-preview-test.html`
and its files:

```sh
python docs/research/html-script-preview-probe-2026-11-15.py <folder>
```

The test page is not in the repository. It holds nine images from different
sources and seven scripts that arrive in different ways.

## The three policies

The document comes from `/preview/<kind>/<file>`, inside a frame of the
page, or in a window of its own.

| Kind | Policy of the answer |
| --- | --- |
| `loose` | `sandbox allow-scripts; frame-ancestors 'self'` |
| `tight` | the policy of ADR-0089 D4 |
| `web` | `tight`, with `https:` in `img-src` and `https://cdn.jsdelivr.net` in `script-src` |

Requests to `https:` were answered by the test, so none left the machine.

## M1 — the test page under `tight` and `web`

The same in the three engines.

| What the page does | `tight` | `web` |
| --- | --- | --- |
| Style sheet in another file | applied | applied |
| `<img>` with a relative path (SVG, PNG, JPEG) | drawn | drawn |
| CSS `background-image`, relative path | drawn | drawn |
| Image added by a script, relative path | drawn | drawn |
| `<img>` and CSS image from the web | not loaded | drawn |
| Inline `<script>` block | runs | runs |
| `<script src>`, relative file | runs | runs |
| `<script>` element that a script adds | runs | runs |
| Inline `onclick` attribute | runs | runs |
| `new Function("…")` | runs | runs |
| `<script src>` from a CDN | not loaded | runs |
| `import()` of a module, from a classic script | fails | fails |

`import()` fails with a different message in each engine. Chromium: the
base URL is `about:blank` because the call comes from a script loaded
across origins without CORS. Firefox: error loading the module. WebKit: the
load is denied by the CORS policy. The test server sends no CORS header.

## M2 — a hostile script

`attack.html` tries each row. "Reaches" means the test server recorded the
request.

| Attempt | `loose` | `tight` |
| --- | --- | --- |
| Read `document.cookie`, `localStorage`, the parent document | refused | refused |
| `window.open`, navigate the top page | refused | refused |
| `fetch` GET and POST, XHR, `sendBeacon`, WebSocket | reaches | does not reach |
| `<img>`, `<script src>`, style sheet, `import()` to `/api/…` | reaches | does not reach |
| A path with `../..` out of the token, another token | reaches | does not reach |
| `form.submit()` | does not reach | does not reach |
| `location.href = "/api/…"` on its own frame | reaches | reaches |

The rows are the same in the three engines, and the same in a frame and in a
window of its own.

## M3 — the cookie and the `Origin` of what reaches

| Engine | Cookie on the requests that reach | `Origin` on a POST |
| --- | --- | --- |
| Chromium | never | `null` |
| Firefox | never | `null` |
| WebKit | always, on a GET by navigation too | `null`; `sendBeacon` sends the daemon's own origin |

So in WebKit, under `loose`, `sendBeacon` sends a POST with the session
cookie and with an `Origin` that the daemon's check accepts. Under `tight`
the same call returns `true` and sends nothing.
