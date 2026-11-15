# An HTML file runs its scripts only when the operator allows it

Status: proposed
Kind: structural
Protects: consistency of the workbench; constraint security

When accepted, changes ADR-0072 D10 (amendment of 2026-11-15, rule 1) and
ADR-0049 §2: one route serves bytes of the repository.

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
has a control that reloads the file with its scripts. The permission belongs
to that tab. It is not stored, and it does not pass to another tab, another
file or another browser. It ends when the tab closes, when the page loads
again, and when the file changes on disk: an agent can write the file again
after the operator looked at it.

**D3. One route serves the files, and a token is its only key.**
`GET /preview/<token>/<path>` answers with a file of the repository. A verb
of the registry (ADR-0036 §1) makes the token, for one repository and one
checkout, when the operator allows a tab. The token is random, lives in the
memory of the daemon, and dies with the permission of D2 or after a fixed
time with no request. The route reads no cookie and no bearer: a document
with an opaque origin sends none in two engines of three, so the token must
be enough, and it must be all.

The route serves a closed list of file kinds: HTML, CSS, JavaScript, JSON,
the image kinds of ADR-0049 §3, and fonts. Every other file is not found.
The path is confined to the checkout as `file.read` confines it, and each
kind keeps the size cap it has today.

**D4. Every answer of the route carries this policy.** `<prefix>` is the
address of the route with its token.

```text
sandbox allow-scripts;
default-src 'none';
script-src 'unsafe-inline' 'unsafe-eval' <prefix>;
style-src 'unsafe-inline' <prefix>;
img-src data: <prefix>;
font-src <prefix>;
connect-src 'none';
form-action 'none';
base-uri 'none';
frame-ancestors 'self'
```

`sandbox` never gains `allow-same-origin`, `allow-forms`, `allow-popups` or
a navigation right. With remote images on (ADR-0032 amendment §F), `img-src`
gains `https:`. The answer also says `nosniff`, `no-store` and
`no-referrer`. The frame in the pane has `sandbox="allow-scripts"` too, so
the rule holds when one of the two is lost.

**D5. No script comes from the web.** `script-src` names the route only. A
library from a CDN does not load, with remote images on or off.

**D6. The document is shown in a frame, never in a window of its own.** The
policy of the workbench page lets a frame go only to the daemon's own
origin, so a script cannot send the frame to another site. A window has no
such limit, and its address bar shows the daemon's host over content the
daemon did not write. The detached file window is a workbench page with the
same frame inside, and stays as it is.

**D7. A GET of the daemon changes nothing.** This becomes a rule that a
check holds, because D4 cannot stop a script from sending one by
navigation. The feature is not built until every GET route is read and the
check exists.

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

## Before the build

These are not measured, and the feature is not built until they are:

- the policy of D4 when the page is reached over `https` through ngrok and
  through dev tunnels, where the origin check of the daemon does not work
  (ADR-0032);
- Safari on an iPhone, for the cookie on a navigation;
- the frame attribute and the response policy together (D4);
- `import()` of a module, with a CORS header on the route.

## Compliance

- D1: not checked by code: reviewed in the PR.
- D2: not checked by code yet: the build adds a test that the permission
  ends on a change on disk.
- D3: not checked by code yet: the build adds a test that the route refuses
  a request with a valid session and no token, and a file kind outside the
  list.
- D4: not checked by code yet: the build adds
  `every_preview_answer_carries_the_policy` next to
  `every_response_carries_the_security_headers`, and a browser check in the
  three engines.
- D5: held by the test of D4.
- D6: not checked by code: reviewed in the PR.
- D7: not checked by code yet: the build adds the check; no route is read
  yet.
