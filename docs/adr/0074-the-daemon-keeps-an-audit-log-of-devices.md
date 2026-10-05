# The daemon keeps an audit log of each device and what it did

Status: proposed
Kind: structural
Protects: security, observability and cost

## Context

The daemon cannot say which device made a change. The session cookie
(`crates/ralphy-daemon/src/cookie.rs`) carries the session epoch, its times
and its kind, and nothing about the browser. No code reads the client's
address. Login, logout and the requests that change state leave no record.
When three devices share one **desk layout**, a wrong change cannot be traced
to the device that sent it.

The operator wants a record of who connected, from where, with what, and what
each device did. Data that the browser reports about itself is wanted too,
even when it can be false, because it helps an investigation.

Measured facts:

- Behind dev tunnels, the client's public address arrives in `X-Real-IP`,
  with `X-Forwarded-Host` (measured 2026-09-21, recorded in
  `routes/guard.rs`). The TCP peer is the tunnel agent on loopback, so the
  socket address alone is useless there.
- With `Accept-CH` and `Critical-CH` on the response, Chromium 148 on
  Windows sends the high-entropy client hints (`Sec-CH-UA-Platform-Version`,
  `-Model`, `-Arch`, `-Bitness`, `-Full-Version-List`) on the next request,
  also over plain http on loopback (measured 2026-10-04 with Playwright and a
  probe server).
- Seven real devices through dev tunnels (2026-10-04, probe server; Edge 153
  on Windows 11, Safari 17.6 on macOS, Safari 26.6 on iPhone and iPad,
  Chrome 148 on Android 12):
  - `X-Real-IP` and `X-Forwarded-For` carried the public address on every
    device. The iPhone on a mobile network showed a different address.
  - Safari sends no client hints. On Safari the user agent string and the
    facts the page reads are the only sources. Firefox was not measured.
  - Safari on iPhone reports `iPhone OS 18_7` with `Version/26.6.1`: the
    system version in the user agent is frozen, and the `Version/` token
    follows the real one.
  - Safari on iPad reports `Macintosh` and `MacIntel`, with
    `maxTouchPoints` 5. A Mac reports 0.
  - Chrome on Android reports `Android 10; K` in the user agent; the client
    hints report `12.0.0` and the model `moto g(30)`.
  - Edge on a Windows desktop answered `pointer: none` and `hover: none`, so
    the pointer media queries do not decide the form of a device alone.
  - `document.fonts.check` returned true for every font on every device, so
    it is not a fact.
- Playwright's three engines on one Windows 11 machine (2026-10-04,
  `tests/browser/security/wb_device_audit.py`): Firefox 150 sends no client
  hints and reports the graphics card as `NVIDIA GeForce GTX 980` on a
  machine with an RTX 3060, so Firefox's graphics card is a generic model.
  Playwright's WebKit reports a Mac user agent on Windows.

## Decision

**D1. A device has an ID that the daemon issues.** The daemon sets the
cookie `ralphy_device` on the first `/api/*` request that does not carry a
valid one. The value is 16 random bytes and an HMAC of them, keyed by
`daemon-device-key`, a key of its own in the daemon's store (owner-only).
The cookie has no session epoch, so the ID stays the same across logout and
login, and a new access token does not change it. It lives 400 days. It is `HttpOnly` and
`SameSite=Strict`, and `Secure` under the same rule as the session cookie.
The session cookie does not change.

**D2. Every recorded fact has a source: `server` or `client`.** `server` is
what the daemon read from the request headers. `client` is what the page
reported about itself. **No access decision reads a device fact or a device
ID.** They are records, not credentials.

**D3. The audit log is one file, `daemon-audit.jsonl`, in the daemon's
store.** It is owner-only, like `daemon-token`. The daemon only appends, one
line per event, each line in one write. Lines older than 90 days are removed,
and the file is kept under 20 MiB. The daemon prunes on its first write
after it starts, and then at most once a day.

**D4. The events are `login_ok`, `login_failed`, `logout`, `device_facts`,
`device_profile_changed` and `action`.** An `action` is each authorized
`/api/*` request with method POST, PUT, PATCH or DELETE: its method, its path
without the query string, its status, the device and the actor. **A line
never holds a request body, a query string, a cookie or an `Authorization`
value.**

**D5. The page sends its facts to `POST /api/device/facts`, once for each
page load.** The body is a fixed shape, capped at 16 KiB. Each string is cut
at a fixed length, and fields outside the shape are dropped.

**D6. The daemon normalizes; the browser only collects.** The rules that turn
raw facts into `os`, `browser`, `engine`, `form` and a model hint are Rust
code with closed enums. Each line records the rules' version
(`normalizer`) and keeps the raw facts, so an old line can be normalized
again. When two sources disagree, the line lists the field in `conflicts`.
The normalizer does not pick one silently.

**D7. No outside call finds a device fact.** The public address comes from
the front's `X-Real-IP` only. No address lookup service, no STUN server, no
GeoIP database. The `connect-src 'self'` policy does not change.

**D8. A request with an `Authorization` header records the actor
`bearer`.** A peer daemon and a machine client both send one, and the
request does not say which peer sent it. The device behind the other daemon
does not cross the peer link.

**D9. The workbench reads the log only through `GET /api/audit/devices` and
`GET /api/audit/events`.** The daemon owns the fact. The page keeps no copy.

**D10. The device ID is the key that joins the daemon's other records to a
device.** A record outside the audit log that names a device (for example
the traffic summary of a console socket) writes the device ID only, in the
same form as the audit log. It never writes the cookie value with its HMAC:
the HMAC is what makes the ID hard to forge, and such a record is read by
more people and tools than the daemon's guard. The record reads the ID from
the request that opened it, an HTTP request or a WebSocket upgrade. D2
applies to it: no access decision reads it.

**D11. A `device_facts` report names the tab.** The D5 body carries the
tab's holder ID: the ID that the console sockets already send, kept in
`sessionStorage`, so it is the same across a reload of the tab and differs
between tabs. Its source is `client`. A record that knows only the holder
joins to a device through the `device_facts` line of that tab.

## Consequences

- Deleting `daemon-device-key` gives every device a new ID. Old lines keep
  the old ID.
- The log tells a reader which networks and devices the operator uses. It is
  a secret in the sense of ADR-0072 D7, and it gets the same file protection
  as the token.
- Logout still ends every session. This ADR adds no way to end the session
  of one device.

## Considered options

- **The FingerprintJS library.** It computes an identifier in the browser,
  which a client can forge. Its own documentation says the open-source
  version has low accuracy. The accurate version is a hosted service, which
  breaks D7.
- **The device ID inside the session cookie.** The ID would change at each
  login. Adding a field changes the cookie format, which signs out every
  browser once.
- **A device key pair in the browser (WebCrypto), signed at login.** This is
  stronger proof that a request comes from the same browser. It is useful
  only together with per-device revocation, which needs a session store on
  the server (ADR-0032 §4 has none). Not now.

## Compliance

- D1: checked by `crates/ralphy-daemon/src/tests/audit_routes.rs`
  (`a_browser_gets_one_device_cookie_and_a_forged_one_is_replaced`).
- D2: not checked by code: manual: reviewed in the PR (no guard or policy
  code reads the device module).
- D3: checked by `crates/ralphy-daemon/src/tests/audit_routes.rs`
  (`an_action_line_keeps_the_path_and_drops_the_query_and_the_body`, which
  checks the file is owner-only) and `crates/ralphy-daemon/src/audit/tests.rs`
  (the two `prune_*` tests).
- D4: checked by `crates/ralphy-daemon/src/tests/audit_routes.rs`
  (`an_action_line_keeps_the_path_and_drops_the_query_and_the_body`,
  `a_login_and_a_replayed_login_are_recorded_with_the_server_facts`).
- D5: checked by `crates/ralphy-daemon/src/tests/audit_routes.rs`
  (`a_facts_body_over_the_cap_is_refused`).
- D6: not checked by code: manual: reviewed in the PR. The rules are pinned
  on the measured devices by `crates/ralphy-daemon/src/audit/normalize/tests.rs`.
- D7: checked by `crates/ralphy-daemon/src/tests.rs`
  (`every_response_carries_the_security_headers`, which pins
  `connect-src 'self'`).
- D8: not checked by code: manual: reviewed in the PR.
- D9: checked by `crates/ralphy-daemon/tests/shared_replies.rs`
  (`the_audit_log_reads_are_the_shared_replies`) and
  `crates/ralphy-daemon/src/tests/audit_routes.rs`
  (`the_device_list_marks_the_device_that_asks`). That the page keeps no
  copy is not checked by code: manual: reviewed in the PR.
- D10: not checked by code yet: to be pinned by a daemon test that a
  console socket's traffic summary carries the device ID and not the
  cookie value.
- D11: checked by `crates/ralphy-daemon/src/tests/audit_routes.rs`
  (`device_facts_are_recorded_once_and_again_when_the_profile_changes`).
