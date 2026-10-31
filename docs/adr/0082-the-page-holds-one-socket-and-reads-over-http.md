# The page holds one socket to its daemon, and reads over HTTP

Status: accepted
Kind: structural
Protects: responsiveness, recoverability

## Context

ADR-0036 §1 decided one protocol over one connection, and rejected "a second
WebSocket per capability". The code did not keep that rule. On 2026-10-16
the page opens WebSockets from four places:

- `wb-daemon.ts:96` (`spawn`) and `wb-daemon.ts:151` (`observe`, also used by
  `write` and `readImage`): a new `/ws/command` socket for each command;
- `wb-daemon.ts:229` (`persistentSocket`): presence `/ws`, and up to three
  `/ws/tree` sockets for the open project (tree, runs, changes);
- `wb-console-terminal.ts:325`: one `/ws/session` socket for each awake
  console. A peer console uses the same origin; the local daemon relays it.

Counted from the code on 2026-10-16, a resume after the page was hidden opens
about N + 4 + 2H + 2F + 2W + 13 sockets at the same moment: N awake consoles,
H expanded folders, F open file tabs in them, W = 1 when a worktree is
selected. With 14 consoles this is about 41 sockets. Nothing in the page
orders them.

Each WebSocket costs a full connection on a remote link:

- Measured on 2026-10-16: the edges of Microsoft dev tunnels and of ngrok
  speak HTTP/2 (`MAX_CONCURRENT_STREAMS` 128) but do not send
  `SETTINGS_ENABLE_CONNECT_PROTOCOL`. So a browser cannot run a WebSocket
  over HTTP/2 (RFC 8441) through them. Every WebSocket is its own TCP and TLS
  connection over HTTP/1.1: about three round trips before the first frame.
  Neither edge offers HTTP/3, so WebTransport is not available either.
- Chromium lets one WebSocket handshake be in progress per IP and port
  (`WebSocketEndpointLockManager`), and Firefox one per IP. WebKit opened 6
  in parallel (#733, M2). At a 600 ms round trip, Chromium opened one socket
  per round trip, and a `tree.list` that the daemon answers in 5 ms waited
  4 s (#733, M3). With 14 sockets, the 8 s handshake deadline closed 2 of
  them while they only waited in the browser's queue (#733, M4).
- `fetch()` through the same edges shares one HTTP/2 connection that is
  already open: no handshake and no queue.
- `daemon.log` on 2026-10-16 recorded 1295 closed console sockets: 625
  `dormant`, 252 `dropped`, 192 `hidden`. On the operator's iPhone alone, 204
  were `dropped`. Each close is a new connection later. iOS Safari closes
  every socket of a page when the screen locks, so they all come back at the
  same moment.

The same page also has reads that wait with no limit. `observe` has no
handshake deadline and no reply timeout (`wb-daemon.ts:148-180`), and
`apiFetch` passes no `AbortSignal`. A read that never settles never fails,
so ADR-0070 D3 ("a failed read shows as a failure") cannot happen for it.

Products that run terminals in a browser over a remote link do this in one
of two ways. VS Code Server sends all terminals over one channel
(`remoteterminal`) of one connection and resumes it with sequence numbers
and acks (`PersistentProtocol`). Coder keeps each terminal's output on the
server and replays it by a token. Ralphy already has the second half: a
console reattach resumes from a byte count (ADR-0079 D8).

The page and its daemon ship in one binary (`assets.rs`, `include_dir!`),
so the browser-to-daemon wire needs no version negotiation. The protocol
between two daemons (ADR-0052, ADR-0067) is not a browser path, and the
browser's limits do not apply to it.

## Decision

**D1. A command with one reply goes over HTTP.** Observe, Write, registry and
one-reply Native verbs are sent as `POST /api/command` with the same
`Command { id, verb, payload }` body that `/ws/command` takes today. It is
one route for the whole registry, as `/api/peer/command` is for a peer: a new
capability is still a verb, not a route (ADR-0036 §1, ADR-0072 D3). The same
guard, the same `MAX_COMMAND_BYTES` cap and the same audit record apply.

**D2. Every read the page makes has a deadline.** A command, an `apiFetch`
read and a decode of a replay each end by a deadline. At the deadline the
read fails, and it shows as failed (ADR-0070 D3). No promise in the page
waits without a limit, and no shared in-flight read can hold later callers
after its deadline.

**D3. The page holds one socket to its daemon.** It is `/ws`. It carries
presence, every push, the tree, runs and changes subscriptions, the output
of a spawned command, and every console, local or on a peer. Each frame
names its channel. A peer console is still relayed by the local daemon.

**D4. A console resumes on the new socket by its byte count.** After a
resume the page opens one socket and sends one resume message for each awake
console (ADR-0079 D8). When the daemon no longer has those bytes, it sends
the snapshot, as today. Each subscription sends its watches again on the new
socket and reads its fact again once (ADR-0036, #484).

**D5. One channel cannot hold back the others.** The daemon cuts each frame
to a bounded size and sends the channels in turn. A console that floods
delays another channel by one frame at most. The browser's input to a
console is never queued behind output.

**D6. The handshake deadline is the handshake of the one socket.** With one
socket there is no queue in the browser, so the deadline measures only the
link. A slow-link verdict (ADR-0079 D10) is read from this handshake.

**D7. What this decision forbids.** No new WebSocket for a capability, a
read, a subscription or a console. No WebSocket over HTTP/2 and no
WebTransport, while the tunnels the operator uses do not offer them. Measure
the tunnels again before you change this rule.

**D8. Order of the change.** D2 comes first; it changes no wire. D1 comes
next; it removes every socket for a read. D3 to D6 come last, together. The
daemon serves `/ws/command`, the `/ws/tree` sockets and `/ws/session` until
the release after D3 ships, so a page loaded before the update keeps working
until it reloads (ADR-0070 D6).

## Consequences

- One socket carries all channels on one TCP connection. A lost packet
  delays every channel until it is sent again. D5 keeps a flood from doing
  the same.
- When the socket drops, every channel drops. On iOS this is already the
  case today: the screen lock closes every socket of the page.
- The `/ws` handler gains a state per channel. The per-console traffic line
  (`ws_session/traffic.rs`) is kept, one line per console channel.
- ADR-0036 §1 is now held by code. Its 2026-09-30 amendment, which describes
  one socket per command, per kind and per console, becomes history when D3
  ships.
- ADR-0072 D3 is unchanged: `POST /api/command` reaches only the verb
  registry.

## Considered options

- **A persistent `/ws/command` socket for reads** (the first direction in
  #733). Rejected: it needs its own timeout, reconnect and replay of pending
  reads. `fetch` gets a deadline from `AbortSignal` and shares the HTTP/2
  connection that is already open, as measured.
- **A queue of handshakes owned by the page**, with the deadline started when
  the page starts each socket. It fixes the 8 s deadline, but keeps one
  connection per console: at a 600 ms round trip, 14 consoles need about
  14 × 1.8 s = 25 s to come back on Chromium.
- **WebSocket over HTTP/2 (RFC 8441).** Rejected: the dev tunnels and ngrok
  edges do not offer it (measured 2026-10-16), and users report that Safari
  26 sends that handshake in a form some proxies refuse.
- **WebTransport.** Rejected: it needs HTTP/3, which neither tunnel offers.
- **Server-sent events for the pushes.** Rejected: they go one way, so console
  input would still need a socket.

## Compliance

- D1: `crates/ralphy-daemon/tests/command_http.rs` (the route answers a command with one reply, refuses a Spawn verb, and caps the body); `websocket_sites_match_the_baseline` in `crates/xtask/tests/ratchets/`, a ratchet on the `new WebSocket(` sites in `crates/ralphy-daemon/assets/ui/` (3 on 2026-10-16), which goes to 1 when D3 ships; `crates/ralphy-daemon/ui-tests/wb-daemon.test.mjs` (`observe`, `write` and `readImage` send their command over HTTP and open no socket).
- D2: `crates/ralphy-daemon/ui-tests/wb-api.test.mjs` and `wb-daemon.test.mjs` (a read or a command with no answer settles with `ReadDeadline` at its deadline, and a late reply changes nothing), and the decode deadline test in `wb-console-frames.test.mjs`.
- D3: the same ratchet as D1, at 1; and the browser check
  `tests/browser/wb_one_socket_740.py` (one page, one socket, with consoles,
  a subscription and a spawned command open).
- D4: the browser check `wb_one_socket_740.py` (a dropped link, one socket,
  one open per awake console); a resume over a real dropped link also needs
  a check in `tests/browser/console/`.
- D5: the daemon test `a_flooding_channel_delays_another_by_one_piece` (the
  fair writer), and the integration test
  `one_flooding_console_delays_another_channel_by_one_piece`.
- D6: not checked by code: manual, reviewed in the PR.
- D7: not checked by code: manual, reviewed in the PR.
- D8: not checked by code: manual, reviewed in the PR.

## Amendment (2026-10-30): the channel wire

This amendment fixes the wire that D3 to D6 need. It changes no rule of D1
to D8. In this section, "channel" means a **socket channel** (CONTEXT.md),
not the release channel. Phase A of the work adds the frame and the writer
without a change of behavior. Phase B moves the tree, runs and changes
subscriptions and the spawned command onto the socket. Phase C moves the
consoles.

**The channel frame (D3).** A new binary tag, `0x04`:

    [0x04][channel: u32 BE][flags: u8][piece]

Bit 0 of `flags` is MORE: another piece of the same inner frame follows on
this channel. The other bits are 0, and the daemon refuses a frame with any
other bit set. The pieces of one channel, joined in order, are one inner
frame in today's codec: `0x01` terminal (with its u64 session), `0x02`
command JSON or `0x03` presence. A piece is at most `MAX_PIECE` = 16 KiB, in
both directions. A joined inner frame above `MAX_COMMAND_BYTES` closes that
channel. Rejected: the u64 slot of `0x01` with a `channel` field in the JSON.
It cannot cut a JSON frame or a gzip frame, and it changes what an existing
field means.

**Channel 0 is the socket itself (D3).** Bare `0x02` and `0x03` frames, with
no `0x04` tag, are exactly what `/ws` sends and takes today: presence,
`presence.report` and every push. A page that never sends `0x04` sees
today's `/ws`. Two verbs control the other channels, both on channel 0. The
page sends `channel.open {channel, kind, ...}` and `channel.close {channel}`.
The daemon sends `channel.closed {channel, reason}` when it ends a channel
itself. The page picks each id: a u32 of 1 or more, never reused on one
socket. The `reason` is one of four words:

- `"refused"`: the daemon did not serve the open or the piece. This covers a
  duplicate id, an unknown id, an unknown kind, an open that its kind cannot
  serve, an open when 256 channels are already open on the socket, and a
  piece with an unknown flag. The socket stays open, except for an open
  that comes after the login changed (a logout, a new token, a change of the
  login policy): the daemon then ends the whole socket, and the page
  connects again through the login check.
- `"too-large"`: the pieces the page sent on the channel join to a frame
  past the cap.
- `"done"`: the kind finished its work, such as a spawned command that ended
  or a console session that ended.
- `"overflow"`: the page sent the channel more frames than its kind had
  taken and its queue holds; only that channel ends.

Rejected: ids that the daemon picks. They add one round trip
before a console can resume.

**Kinds (D3).** The set is closed: the Rust enum
`ChannelKind { Console, Tree, Runs, Changes, Spawn }` and the TypeScript
union `"console" | "tree" | "runs" | "changes" | "spawn"`.

- `console` takes today's `SessionQuery` fields in the open payload (`repo`,
  `agent`, `id`, `takeover`, `watch`, `console`, `checkout`, `command`,
  `holder`, `name`, `record`, `gzip`, `attempt`, `since`, `net`, `net_rtt`,
  `net_down`). It then carries today's session frames on its channel.
- `tree` and `runs` carry today's `/ws/tree` verbs and pushes.
- `changes` carries today's `changes.dirty`.
- `spawn` takes one `Command {id, verb, payload}`, streams today's
  `SpawnStatus` frames, and then closes.

**Fair sending (D5).** Each channel has one bounded queue. One writer task
sends one piece from each non-empty channel in turn, channel 0 included. A
reader task reads the socket on its own, so console input never waits behind
output. Rejected: awaiting `socket.send` inside each `select!`, as today's
loops do. A slow link then holds the input and every other channel.

**Liveness (D3).** The socket has one WebSocket ping: every 20 s, and 45 s
with nothing received ends the socket, as `/ws/session` does today. A console
channel has no ping of its own. The local daemon answers the pings of a peer
on the daemon-to-daemon socket itself, and no longer forwards them to the
browser.

**The resume message is the open (D4).** After a resume, the page opens one
socket and sends one `channel.open {kind: "console", id, since, ...}` for each
awake console (ADR-0079 D8). A daemon that no longer has the bytes sends the
snapshot, as today. Each subscription sends its watches again and reads its
fact once (#484).

**Compliance of this amendment.** Each row above is checked as D3 to D6 say in
Compliance: the unit test `a_flooding_channel_delays_another_by_one_piece`
(fair sending), the integration test
`one_flooding_console_delays_another_channel_by_one_piece` (phase B), the
ratchet `websocket_sites_match_the_baseline` at 1, and the browser check
`wb_one_socket_740.py`.
