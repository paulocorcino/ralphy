# The daemon shows a local web application in a frame

Status: proposed
Kind: structural
Protects: operability; constraint security

Builds on ADR-0089: the token as the only key, the opaque origin, and the
refusal of its D7. It changes no rule of ADR-0089. Nothing here is built. The
points under "Not measured" are checked by a spike before any code in the
daemon, and an amendment records what the spike finds.

## Context

The operator often works from outside, through one address that a tunnel
publishes for the daemon. A web application that runs on the daemon's machine
(a `Bun.serve` page, a development server) listens on a loopback port. The
operator cannot see it from outside without a second address, made and kept
outside Ralphy. On 2026-11-15 the operator decided that the page of the
application must open inside the workbench, through the daemon's own address.

ADR-0089 already serves a document from the daemon's host with an origin of
its own. Its measurements of 2026-11-15, in Playwright Chromium 148, Firefox
150 and WebKit 26.4, decide most of this design:

- `Content-Security-Policy: sandbox allow-scripts` on an answer gives the
  document an opaque origin in the three engines. Its script cannot read the
  cookie, the storage or the workbench page.
- WebKit sends the session cookie of the daemon with every request of such a
  document, though the cookie is `SameSite=Strict`.
- A policy that names an address with its token keeps every request of the
  script inside that address. A path that leaves the token is refused.
- No policy directive stops a script from navigating its own frame.

By the HTML standard, and not measured here, a document with an opaque
origin keeps no cookie, cannot use `localStorage`, and cannot register a
service worker.

An application differs from a file of the repository in three ways. Its
script calls its own server, so the policy cannot close `connect-src`. It
names its files from the root (`/assets/app.js`, `fetch("/api")`), and under
the daemon's address the root is the daemon. It keeps a login in a cookie.

Two libraries were considered. `wsproxy` 0.1.3 was read on 2026-11-15, on
docs.rs and in the README of its repository: it carries a TCP connection
inside a WebSocket, between a client program and a server program.
`lol_html` was not read again for this ADR. What is stated of it here is
what its documentation is known to say: it rewrites HTML as a stream, by CSS
selectors, and passes the other bytes as they are. The spike confirms it.

## Decision

**D1. The capability is an option of the daemon, off by default.** It is a
flag file of the store, set from the Security dialog, and turning it on
costs a fresh code once a TOTP seed is armed (ADR-0072 D1). With it off, the
verb of D2 is refused and the route of D3 is not found.

**D2. The operator names the port, and the daemon dials loopback only.** A
verb of the registry (ADR-0036 §1), `port.preview`, takes a port number and
makes a token. The daemon dials `127.0.0.1` or `::1` and that port, as an
address literal. It never resolves a host name, and it takes no host from
the browser. It refuses its own port and the local port of every peer
tunnel: a request that arrives at a daemon from loopback is not a request of
the operator.

**D3. One route carries the application, and a token is its only key.**
`/app/<token>/<path>` forwards a request to the port of the token, with any
method, and upgrades a WebSocket. The token follows ADR-0089 D3: 32 random
bytes, in the memory of the daemon, dead when its frame closes or after 30
minutes with no request. The route reads no cookie and no bearer, and the
guard still checks `Host`. The route of ADR-0089 is not used: it serves a
closed list of file kinds with `connect-src 'none'`, and that rule stays
whole.

**D4. The daemon builds the request to the application. It does not copy
it.** It sends the method, the path after the token, the query, the body,
and a closed list of headers that describe content. It never sends the
`Cookie`, `Authorization`, `Origin`, `Referer` or `X-Forwarded-*` of the
browser: WebKit puts the daemon's session cookie on these requests. `Host`
is the loopback address and port. The daemon asks for no compression.

**D5. The cookies of the application stay in the daemon.** A `Set-Cookie`
of the application is kept in a jar of the token, in memory, and is not sent
to the browser. The daemon adds the jar to the next requests of the token.
The jar dies with the token.

**D6. Every answer of the route carries the daemon's policy, never the
application's.** `<prefix>` is the address of the route with its token, in
its `http` form and its WebSocket form.

```text
sandbox allow-scripts allow-forms;
default-src 'none';
script-src 'unsafe-inline' 'unsafe-eval' <prefix>;
style-src 'unsafe-inline' <prefix>;
img-src data: blob: <prefix>;
font-src <prefix>;
connect-src <prefix>;
form-action <prefix>;
worker-src blob:;
base-uri 'none';
frame-ancestors 'self'
```

`sandbox` never gains `allow-same-origin`, `allow-popups` or a navigation
right. The daemon removes the `Content-Security-Policy` and the
`X-Frame-Options` of the application. The answer also says `nosniff`,
`no-store` and `no-referrer`. The options of the daemon for remote images
and for web content (ADR-0089, amendment of 2026-11-16) add `https:` to the
same directives as they do there.

**D7. The route never answers with an address outside its prefix.** A policy
does not check the path of an address after a redirect. So the daemon
rewrites a `Location` of the application into the prefix, and refuses a
redirect to another host with `502`.

**D8. The daemon rewrites the HTML answers, and only those.** An answer of
kind `text/html` passes through `lol_html` as a stream. Three changes are
made:

1. A script that Ralphy ships is the first element of `<head>`.
2. An address that starts at the root, in `src`, `href`, `action`, `srcset`
   or `poster`, gains the prefix.
3. `integrity`, `<base>`, and a `<meta>` that sets a policy or a refresh are
   removed.

No JavaScript file and no style sheet is rewritten.

**D9. The shipped script moves the addresses that the script of the
application builds.** It wraps `fetch`, `XMLHttpRequest`, `WebSocket` and
`EventSource`, so that an address that starts at the root gains the prefix.
It gives `localStorage` and `sessionStorage` a stand-in that lives in the
memory of the document. It is served by the route, so `script-src` allows
it.

**D10. A request outside the prefix is not the application's.** The daemon
does not guess the token from a `Referer`, and it does not redirect such a
request into the route. The policy of D6 stops these requests in the
browser.

**D11. The application is shown in a frame of the workbench, never in a
window of its own** (ADR-0089 D6). The frame has
`sandbox="allow-scripts allow-forms"` too. The refusal of ADR-0089 D7 holds
for this frame.

**D12. Only the daemon the browser talks to serves the route.** For a
project of a peer the verb answers a failure, as `file.preview` does. A
later decision that reaches a port of a peer carries the bytes on the peer's
own link (ADR-0052, ADR-0067). It adds no second tunnel program.

**D13. `lol_html` is a dependency of `ralphy-daemon`, named by one module.**
It comes from crates.io under ADR-0072 D12. `wsproxy` is not used.

**D14. Every size and count of the route has a constant** (ADR-0072 D11):
the body of a request, the frame of a WebSocket, the memory of the rewriter
for one answer, the tokens alive, and the open connections of one token. A
dial has a timeout.

## Consequences

- An application that needs a service worker does not work.
- What an application keeps in `localStorage` is lost when the frame loads
  again.
- An address that starts at the root inside a JavaScript file or a style
  sheet is not moved by D8. A module that imports `/src/main.ts`, or a style
  sheet with `url(/font.woff)`, fails unless the spike finds a way that
  needs no rewrite of those files.
- A script that sets `location` to an address at the root leaves the
  application: the frame asks the daemon for that address, and D10 does not
  bring it back.
- A script the operator allowed can send a request with a body to the
  daemon's host, inside the prefix only. ADR-0089 kept `connect-src 'none'`;
  this route cannot.
- The daemon gains an outgoing call on loopback, to a program Ralphy did not
  start. ADR-0072 D9 covers calls to a service outside the machine. When
  this is built, docs/ARCHITECTURE.md §6 names the owner of this call, and
  ADR-0072 gains an amendment for the new document from outside (D10).
- A person who holds a workbench session can already open a console and
  read any loopback port from it. The route gives that person no new reach.
  The new risk is the script of the application, and D4 to D7 are its
  controls.

## Considered options

- **A second port on the tunnel, one address per application.** It needs no
  code, and the application runs unchanged with an origin of its own. The
  operator refused it on 2026-11-15: the address is made and kept outside
  Ralphy, and the page opens outside the workbench.
- **`wsproxy` between the browser and the application.** A browser cannot
  open a TCP connection, so something must still speak HTTP for it, and the
  daemon already reaches the loopback port. Its server binds a port of its
  own and its documentation names no authentication. For a peer, the link of
  ADR-0052 already exists.
- **A proxy under a path with no sandbox.** The application then has the
  origin of the workbench. Its script, and every package it loads, could use
  the operator's session.
- **A service worker that moves every request.** A document with an opaque
  origin has none, and `allow-same-origin` gives the origin of the
  workbench.
- **A redirect of a request at the root, by its `Referer`.** The policy of
  D6 stops the request before it leaves, the answers say `no-referrer`, and
  the rule would put the application's paths in front of the daemon's own
  routes.
- **A browser on the daemon's machine that sends pixels or its DOM.** The
  session and the script stay on the machine and no address is rewritten. It
  needs a browser engine beside the daemon, and each key press crosses the
  tunnel before it shows. It is another project (the browser window of the
  stage), with another purpose.
- **`html5ever` in place of `lol_html`.** It builds a tree of the whole
  document and writes it again, so the answer waits for its end and its
  bytes change.

## Not measured

Nothing of this ADR is built. The spike is a program apart from the daemon,
on loopback, with three applications: a `Bun.serve` page that calls
`fetch("/api")`, a Vite development server with hot reload, and an
application with a login cookie. It records, for Chromium, Firefox and
WebKit:

- which of D8 and D9 each application needs to open, to move between its
  pages, to reload on a change of its source, and to keep its login;
- whether `history.pushState` works in a document with an opaque origin;
- whether an import map added by D8, which maps the root to the prefix,
  moves the module addresses that a development server writes inside
  JavaScript files;
- whether a form sent with `allow-forms` stays inside the prefix;
- the time and the memory `lol_html` adds to an HTML answer;
- through a tunnel over `https`: whether `<prefix>` names the address the
  browser uses for a WebSocket.

Still open from ADR-0089: whether Safari sends `Sec-Fetch-Dest` on the
navigation of a frame.

## Compliance

- D1: not checked by code: not built.
- D2: not checked by code: not built. The check will be a test that the verb
  refuses the daemon's own port, a peer tunnel's port and any host.
- D3: not checked by code: not built.
- D4: not checked by code: not built. The check will be a test that lists
  the headers the application receives.
- D5: not checked by code: not built.
- D6: not checked by code: not built. The check will be a test of the policy
  text, as for ADR-0089 D4.
- D7: not checked by code: not built.
- D8: not checked by code: not built.
- D9: not checked by code: not built; a browser check in the three engines.
- D10: not checked by code: not built.
- D11: not checked by code: reviewed in the PR.
- D12: not checked by code: not built.
- D13: not checked by code: not built. The check will be a pin that one
  module names `lol_html`.
- D14: not checked by code: not built.
