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
the size cap it has today. A font is not served yet: a document with an
opaque origin loads a font and a module only with a CORS header, which the
route does not send.

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
library from a CDN does not load, with remote images on or off.

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
  of a frame (D7). A browser that does not send it is not refused;
- `import()` of a module and a font, with a CORS header on the route.

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
  `the_policy_names_only_the_route_and_takes_web_images_when_allowed`,
  `a_preview_file_is_served_to_its_token_with_its_own_policy` and
  `the_policy_names_the_host_and_scheme_of_the_front`.
- D5: held by the test of D4.
- D6: not checked by code: reviewed in the PR.
- D7: checked by `a_frame_that_navigates_to_a_data_endpoint_is_refused`, and
  in the three engines by the browser check (a script reaches no data
  endpoint).
