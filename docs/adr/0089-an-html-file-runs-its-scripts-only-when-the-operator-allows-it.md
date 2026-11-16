# An HTML file runs its scripts only when the operator allows it

Status: accepted
Kind: structural
Protects: consistency of the workbench; constraint security

Changes ADR-0072 D10 (amendment of 2026-11-15, rule 1) and ADR-0049 §2: one
route serves bytes of the repository. Adds the verb `file.preview` to the
registry of ADR-0036.

## Context

The file pane shows an HTML file in a frame with no rights (ADR-0072,
amendment of 2026-11-15). A script in the file does not run, and a style
sheet, an image named in CSS or a script in another file does not load. That
is enough for a report. It is not enough for a page that draws a chart, or
for a prototype an agent wrote.

A switch on the frame cannot give the script. The frame is a `srcdoc`, and
a `srcdoc` inherits the policy of the workbench page, which takes no inline
script and no script outside the daemon's own origin. A document gets a
policy of its own only when a response carries it. So a document with
script must come from an address.

Measured on 2026-11-15, in Playwright Chromium 148, Firefox 150 and WebKit
26.4, on Windows, on loopback over `http`, against a test server that sends
the policy of `routes/headers.rs` for the page. The record and the script
are in
[the probe of 2026-11-15](../research/html-script-preview-probe-2026-11-15.md).

- A document served from the daemon's host with
  `Content-Security-Policy: sandbox allow-scripts` has an opaque origin in
  the three engines. Its script cannot read the cookie, the storage or the
  workbench page, cannot open a window, and cannot navigate the page.
- With that policy alone, every request of the script reaches the daemon:
  `fetch`, XHR, beacon, WebSocket, image, script and style sheet. WebKit
  sends the session cookie with each one, though it is `SameSite=Strict`.
  One of them, `sendBeacon`, carried the daemon's own `Origin`, so the origin
  check of the daemon (ADR-0032) would let it pass.
- With the policy of D4, none of these requests reaches the daemon in the
  three engines. A path that leaves the token (`../..`, another token) is
  refused too.
- One request is left. The script can navigate its own frame to an address
  of the daemon. The GET arrives in the three engines, and in WebKit it
  carries the session cookie. No policy directive stops a navigation.
- The test page of the operator works under the policy of D4 in the three
  engines: the style sheet, the image named in CSS, an inline script, a
  script file, a script that another script adds, an `onclick` attribute
  and `new Function`. `import()` of a module fails in the three.
- The same document in a window of its own gave the same results as in a
  frame, in the three engines.

## Decision

**D1. The static frame stays the default.** A file opens as ADR-0072
describes. Nothing in this ADR changes what a file does before the operator
acts.

**D2. The operator allows the scripts of one tab.** The pane of an HTML file
has a control, Run scripts, that draws the file again with its scripts. The
permission belongs to that tab. It is not stored, and it does not pass to
another tab, another file or another browser. It ends when the tab closes,
when the page loads again, and each time the pane draws the file again: a
reload, a save, a change on disk, a return from the editor. An agent can
write the file again after the operator looked at it. The control refuses a
pane with unsaved text, because the route serves the file on disk.

**D3. One route serves the files, and a token is its only key.**
`GET /preview/<token>/<path>` answers with a file of the repository. The verb
`file.preview` of the registry (ADR-0036 §1) makes the token, for one
repository and one checkout, when the operator allows a tab; with `revoke`
it ends one. The token is 32 random bytes, lives in the memory of the
daemon, and dies with the permission of D2 or after 30 minutes with no
request. At most 32 tokens live at once; the oldest makes room. The route
reads no cookie and no bearer, and the guard does not check the `Origin` of
its requests: a document with an opaque origin sends no cookie in two
engines of three, and Firefox sends `Origin: null` on each of its requests.
So the token must be enough, and it must be all. The `Host` is still
checked.

The verb is served only by the daemon the browser talks to. For a project
of a peer it answers the failure `scripts-on-a-peer`: the browser cannot
reach the route of a peer.

The route serves a closed list of file kinds: HTML, CSS, JavaScript, JSON
and the image kinds of ADR-0049 §3. Every other file is not found. The path
is confined to the checkout as `file.read` confines it, and each kind keeps
the size cap it has today. The amendment of 2026-11-15 adds fonts, a CORS
header and a cap of the route.

**D4. Every answer of the route carries this policy.** `<prefix>` is the
address of the route with its token.

```text
sandbox allow-scripts;
default-src 'none';
script-src 'unsafe-inline' 'unsafe-eval' <prefix>;
style-src 'unsafe-inline' <prefix>;
img-src data: <prefix>;
connect-src 'none';
form-action 'none';
base-uri 'none';
frame-ancestors 'self'
```

`sandbox` never gains `allow-same-origin`, `allow-forms`, `allow-popups` or
a navigation right. With remote images on (ADR-0032 amendment §F), `img-src`
gains `https:`. The answer also says `nosniff`, `no-store` and
`no-referrer`, and it does not carry the page's own policy or
`X-Frame-Options`. The frame in the pane has `sandbox="allow-scripts"` too,
so the rule holds when one of the two is lost. `<prefix>` is built from the
request: `X-Forwarded-Host` when a front sent it, else `Host`, and `https`
when the front said so.

**D5. No script comes from the web.** `script-src` names the route only. A
library from a CDN does not load, with remote images on or off. The
amendment of 2026-11-16 adds an option of the daemon that opens this.

**D6. The document is shown in a frame, never in a window of its own.** The
policy of the workbench page lets a frame go only to the daemon's own
origin, so a script cannot send the frame to another site. A window has no
such limit, and its address bar shows the daemon's host over content the
daemon did not write. The detached file window is a workbench page with the
same frame inside, and stays as it is.

**D7. The daemon refuses a frame that navigates to a data endpoint.** D4
cannot stop a script from navigating its own frame to an address of the
daemon. So the guard refuses, with `403`, every request to `/api` or `/ws`
whose `Sec-Fetch-Dest` is `iframe`, `frame`, `embed`, `object` or
`fencedframe`. The page reads those endpoints with `fetch` and a socket,
never by a navigation, so nothing of the workbench is refused.

The first form of this rule was "a GET of the daemon changes nothing". It
was read against the code on 2026-11-15 and it is not true. `GET /api/repos`
can start `ralphy daemon add` to give a repository its new key.
`GET /api/fleet` and `GET /api/agents` with a peer's repository can start
the `ssh` tunnel of a peer that does not answer. `GET /api/session` can
write the device key and set the device cookie. Five more reads ask every
peer, and `GET /api/telegram/bot` asks Telegram. None of these takes a
value the caller chooses, and the open page sends them all the time, but
they are effects. The refusal above does not depend on them.

## Consequences

- ADR-0049 §2 refused a route that serves bytes of the repository. This ADR
  adds one. The reasons of §2 are answered this way: the route is one door
  with one key and no session (D3), every answer is a document that the
  browser isolates (D4), and a refusal is a `404` that the pane reads before
  it shows the frame.
- A document with its scripts allowed loads its files by itself. The
  workbench rewrites nothing for it, so the HTML library in the daemon that
  ADR-0072 names as a later step is not needed for a tab the operator
  allowed.
- A script the operator allowed can use the processor and the memory of the
  tab without a limit, and can show any content inside its frame.

## Considered options

- **A switch on the `srcdoc` frame.** The inherited policy refuses the
  scripts of the document (Context).
- **A window of its own.** Measured: no protection is gained, and D6 lists
  what is lost.
- **Another port or another host for the route.** A cookie is not bound to a
  port, a tunnel publishes one port, and a local daemon has no second host.
  The opaque origin of D4 is what isolates the document.
- **A permission the daemon remembers for a file.** The file can change
  between two visits, and nobody would look at it again.

## Not measured

The build was measured on loopback over `http`, in Playwright Chromium 148,
Firefox 150 and WebKit 26.4. These are open:

- the route when the page is reached over `https` through ngrok and through
  dev tunnels: whether `<prefix>` of D4 names the address the browser uses;
- Safari on an iPhone: whether it sends `Sec-Fetch-Dest` on the navigation
  of a frame (D7). A browser that does not send it is not refused.

## Compliance

- D1: checked in the three engines by
  `tests/browser/files/wb_html_file.py` (the file shows no effect of its
  scripts until Run scripts is pressed).
- D2: checked by `ui-tests/wb-viewer-scripts.test.mjs` (a second click ends
  the token; no script for unsaved text, a refusal, or a pane that went
  away), and in the three engines by the browser check (Stop scripts and
  Reload draw the file with no script; the token is dead after Stop). Not
  checked by code: that every later draw of the pane ends the permission.
- D3: checked by `a_preview_file_is_refused_without_its_token_or_outside_the_list`
  and `a_preview_file_is_served_to_its_token_with_its_own_policy`
  (`crates/ralphy-daemon/src/tests/preview_route.rs`),
  `only_the_closed_list_of_file_kinds_is_read`,
  `a_token_opens_its_root_until_it_is_idle_or_closed` and
  `the_oldest_token_makes_room_for_a_new_one`
  (`crates/ralphy-daemon/src/preview/tests.rs`), and
  `file_preview_makes_a_token_for_the_repo_and_ends_it`. Not checked by
  code: the refusal for a project of a peer.
- D4: checked by
  `the_policy_names_only_the_route_and_takes_from_the_web_what_the_daemon_allows`,
  `a_preview_file_is_served_to_its_token_with_its_own_policy` and
  `the_policy_names_the_host_and_scheme_of_the_front`.
- D5: held by the test of D4.
- D6: not checked by code: reviewed in the PR.
- D7: checked by `a_frame_that_navigates_to_a_data_endpoint_is_refused`, and
  in the three engines by the browser check (a script reaches no data
  endpoint).

## Amendment 2026-11-15 — D3, D4: fonts, a CORS header, and a cap of the route

Changes D3 and D4. A mock of the workbench (`mocks/workbench-shell/`) showed
what the first build could not load: its icon fonts, and a vendored library
of 3.5 MB.

- **The route serves fonts.** The closed list of D3 gains `woff`, `woff2`,
  `ttf` and `otf`, and the policy of D4 gains `font-src <prefix>`.
- **Every answer of the route says `Access-Control-Allow-Origin: *`.** A
  document with an opaque origin loads a font and a module only with a CORS
  header. The token stays the only key: the header gives nothing to a page
  that does not hold the address with its token.
- **A text or font file of the route has its own cap, 16 MiB**
  (`MAX_PREVIEW_BYTES`, ADR-0072 D11). The cap of `file.read`, 2 MiB, is the
  size an editor opens; a vendored library is one larger file. The file is
  the body of its own answer and never rides inside a command, so the
  transport cap of D11 does not apply. An image keeps the cap of ADR-0049.

**What the header costs.** With it, a script the operator allowed can read
the text of the style sheets, the JSON files, the modules and the images of
the checkout, not only use them. It still has no way to send them: D4 keeps
`connect-src 'none'`, and no script or image comes from the web (D5). A
later decision that lets a document load from the web opens that way, and
must weigh this.

**Measured**, in Playwright Chromium 148, Firefox 150 and WebKit 26.4, on
loopback:

- without the header, the icon font fails in Chromium and Firefox; with it,
  the font loads in the three;
- with the header, `<script type="module">` runs in the three
  (`tests/browser/files/wb_html_file.py`);
- with the header, `import()` called from a classic script runs in Firefox
  and WebKit and still fails in Chromium, which gives a classic script
  loaded with no `crossorigin` attribute no base address. This is accepted.

**Compliance.** `only_the_closed_list_of_file_kinds_is_read` (the fonts and
the cap),
`the_policy_names_only_the_route_and_takes_from_the_web_what_the_daemon_allows`
(`font-src`), `a_preview_file_is_served_to_its_token_with_its_own_policy`
(the header), and the browser check in the three engines.

## Amendment 2026-11-16 — D5: content from the web, as an option of the daemon

Changes D5. D5 said that no script comes from the web. A page that uses a
library from a CDN then stays broken, and the operator asked for a way to
accept that risk.

**The decision.** The daemon gains an option, **web content in HTML files**,
off by default. With it on, the policy of D4 gains `https:` in `script-src`,
`style-src` and `font-src`: an HTML file that runs its scripts may load
scripts, styles and fonts from other websites. `img-src` still follows
remote images (ADR-0032 amendment §F), and `connect-src` stays `'none'`:
a script still cannot call an address with `fetch` or a socket.

The option is a flag file of the store, `daemon-web-content`, as remote
images is. `POST /api/security/web-content` sets it, and turning it on costs
a fresh code once a TOTP seed is armed (ADR-0032 amendment E); turning it
off is free. `GET /api/security/state` reports it. The Security dialog shows
it with its risk in words. The next answer of the route carries the new
policy, so a page needs no reload.

**The risk the operator accepts.** With the CORS header of the amendment of
2026-11-15, a script the operator allowed can read the style sheets, JSON
files, modules and images of the checkout. With this option on, it can also
send them: a request for a script or a style at an address of its choice
carries what it wants in that address. The code of the other website also
runs inside the document. So, with the option on, Run scripts on a file
gives that file the readable files of its project. Three limits stay: the
route serves only its closed list of kinds, the document has an opaque
origin and cannot use the session, and the daemon refuses a frame that
navigates to a data endpoint (D7).

When the page is reached over `https`, `https:` also names the daemon's own
origin, as it does for remote images. A script can then ask the daemon's
own address for a script or a style: a GET, with the session cookie in
WebKit. This case is not measured.

By ADR-0072 D1 the stronger setting stays the default, and the weaker one
is the operator's own act.

**Rejected.**

- *A permission per tab, on the Run scripts control.* The control already
  asks one question. A second one beside it, with a larger risk, would be
  answered the same way without a thought. The Security dialog is where the
  other postures of the daemon are lowered.
- *`connect-src https:` too.* It adds nothing an attacker needs, but it lets
  a script call the daemon's own address with a body when the page is
  reached over `https`. It is left closed.

**Compliance.** Checked by
`web_content_is_opt_in_and_costs_a_fresh_code_to_enable`
(`crates/ralphy-daemon/tests/security_web_content.rs`),
`the_policy_names_only_the_route_and_takes_from_the_web_what_the_daemon_allows`,
`ui-tests/wb-security-dialog.test.mjs` (each switch posts to its own route),
and in the three engines by the browser check (a script from the web is
asked for and runs only with web content on).

## Amendment 2026-11-16 — D4: a worker from a blob

Changes D4. The policy gains `worker-src blob:`. A library that draws on a
canvas from a Web Worker builds the worker from a `blob:` of its own code.
Without the directive the worker is refused, and in Chromium the library
then draws nothing (measured with canvas-confetti 1.9.3: the canvas is made
and stays empty).

The worker is code the document's own script wrote, and that script already
runs. A worker from a `blob:` inherits the policy of the document, so
`connect-src 'none'` holds inside it. A worker from a file of the route is
not possible: a document with an opaque origin has no address of its own
origin.

**Compliance.**
`the_policy_names_only_the_route_and_takes_from_the_web_what_the_daemon_allows`
(the directive), and the browser check in the three engines (a worker from a
blob answers the page).

## Amendment 2026-11-16 — a check by hand through dev tunnels

Changes the list under "Not measured". No rule changes.

The operator checked the build by hand on 2026-11-15, on an iPad that
reached the daemon through dev tunnels over `https`. Run scripts worked on
the test page of the probe. With web content on, the library from a CDN
loaded and drew (canvas-confetti, which needs the worker of the amendment
above). The version of the system and of the browser was not recorded.

This closes one open point: behind dev tunnels, which rewrites `Host`, the
`<prefix>` of D4 names the address the browser uses. The files of the
document would not load otherwise.

These stay open:

- the route behind ngrok;
- whether Safari sends `Sec-Fetch-Dest` on the navigation of a frame (D7).
  A page that works does not show it: only a file that tries the navigation
  does;
- with web content on and the page reached over `https`, a script that asks
  the daemon's own address for a script or a style.
