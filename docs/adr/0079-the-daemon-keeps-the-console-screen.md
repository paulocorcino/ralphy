# The daemon keeps the console screen, and a reattach is ordered and resumable

Status: accepted
Kind: structural
Protects: consistency of the workbench, responsiveness

Amends ADR-0051 §9 (one size per session; the size frame; the attempt number;
the resumable reattach) and ADR-0076 D14 (the output an agent reads).

## Context

The operator drives two or more agent consoles from an iPad through dev
tunnels. After a switch to another console and back, the text the agent wrote
in the meantime is narrow, and a later wide redraw paints over it (#689). The
text in the agent is correct. Only the screen is wrong. The causes, checked in
#689:

- A covered console still set the size of its PTY (F1). Fixed in the page on
  2026-10-12: a covered console without the focus sends no size.
- Every attach replays the raw ring, cut at any byte, and the bytes in it were
  written at several widths (F3). A TUI's cursor-up redraw is right only at
  the width it was written for. In the operator's log (2026-10-05 to
  2026-10-07, 600 console sockets), 407 sockets got a full ring of 262 153
  bytes.
- No frame tells a watcher the size of the PTY, so it paints at its own
  size (F4).
- A late socket of the same tab takes the writer slot from the window's
  current socket (F5, #598).
- Reattaches are the largest cost on a slow link (F6, #581): in the same log,
  113.7 MB of replay against 54.5 MB of live output. The 220 sockets that
  ended `dormant` had a median replay of 262 153 bytes.
- `tungstenite` 0.29 has no permessage-deflate. The daemon already depends on
  `flate2`, and browsers have `DecompressionStream("gzip")` (F7).

A spike on 2026-10-12 (Windows 11, release build) compared
`alacritty_terminal` 0.26.0 and `vt100` 0.16.2. It fed recorded Claude Code,
Codex and OpenCode streams (spawned through `ralphy-pty`, with a resize
120 -> 60 -> 120 in the middle), synthetic streams and hostile input. The
method and every number are in the spike report of #689; the deciding numbers:

| criterion | alacritty_terminal 0.26.0 | vt100 0.16.2 |
|---|---|---|
| reflow on resize (a 300-char line, 120 -> 60 -> 120) | yes | no: cut to 60 chars |
| scrollback while DECSTBM 1;37 of 40 is set, 3000 lines | 3000 kept | 0 kept |
| heap per session, 120 cols, 1000 / 2000 / 5000 / 10000 lines | 5.00 / 7.78 / 16.21 / 30.20 MiB | 3.84 / 7.54 / 18.71 / 37.27 MiB |
| heap per session, 200 cols, same lines | 6.98 / 11.59 / 25.51 / 48.65 MiB | 6.38 / 12.52 / 31.01 / 61.78 MiB |
| resident, 30 full sessions, 1000 lines, 120 / 200 cols | 94.1 / 153.3 MiB | not measured |
| parse 512 KiB, median of 10 (flood; recorded claude) | 7.44 ms; 3.69 ms | 9.02 ms; 4.85 ms |
| snapshot writer | none in the crate; about 330 lines in the spike | built in, but no scrollback and no alternate-screen switch |
| snapshot round trip, 20 streams (recorded, modes, alt screen, region, flood at two widths) | all equal | visible screen equal only |
| input modes the snapshot keeps (of 10) | 10 | 8: focus 1004 and kitty keyboard lost |
| 16 MiB random bytes; CSI with 10 000 parameters | no panic; 5.11 MiB after | no panic |
| 16 MiB OSC that never ends | no panic; 66.22 MiB after (2.48 MiB with a 64 KiB string cap) | no panic; 32.15 MiB after |
| resize to 0x0; to 1x1 | panic; no panic | panic; panic |
| new crates in the workspace; `cargo tree -e normal` lines | 16; 55 | 4; 21 |

Five more facts from the spike shape the rules:

- `alacritty_terminal` keeps the scroll region and the primary grid (while the
  alternate screen is on) private. The spike reads the primary grid by
  cloning the alternate grid, swapping twice and putting the clone back, and
  it tracks DECSTBM with a second `vte` parser (+0.6 to 1.1 ms per 512 KiB).
- vte's `Processor` buffers a synchronized update (mode 2026) for up to
  150 ms and allocates a 2 MiB buffer for it. A snapshot taken during that
  time would miss bytes that the ring already sent. A `Timeout` that never
  pends turns the buffering off; the 2 MiB buffer is then never touched (30
  empty sessions: 6.5 MiB resident).
- vte keeps an unterminated OSC string in a buffer that never shrinks.
- The daemon accepts any `u16` size: `routes/ws_session/bridge.rs` passes
  `rows` and `cols` from a `resize` frame to the PTY with no clamp. `0x0`
  panics both crates.
- On Windows the PTY is ConPTY, which is itself a terminal emulator and
  repaints: the harness's `flood 600` (about 600 KiB written by the child)
  reaches the daemon as about 250 KB. So on Windows the daemon's emulator
  reads ConPTY's rendered output; on Linux and macOS it reads the child's
  own bytes. No scroll region reached the daemon in the recorded Codex
  stream on Windows.

The page's xterm keeps its default scrollback of 1000 lines (no `scrollback`
option is set). A full 1000-line snapshot of prose (the repo's ADRs, colored)
is 64 052 bytes, 23 841 bytes with gzip; 1000 lines of the flood pattern are
82 104 bytes, 3 541 with gzip.

## Decision

**D1. The daemon keeps one terminal emulator per workbench session, built on
`alacritty_terminal`.** It reads every output byte of the session, at the
session size, under the scrollback lock that already keeps the replay exactly
once. Only the screen module (`session/screen.rs`) names the crate. No other
module, no wire type and no page code sees an `alacritty_terminal` type. The
crate is added with `default-features = false`.

**D2. The emulator applies every byte at once.** Its `vte` processor uses a
timeout that never pends, so a synchronized update (mode 2026) is never
buffered. The screen therefore always holds every byte the ring holds.

**D3. Output is untrusted data, and the emulator is capped (ADR-0072 D11).**
- An OSC, DCS, APC, PM or SOS string longer than 64 KiB is ended with CAN, and
  the rest of that string is dropped before it reaches the parser.
- Every session size is clamped to 2..=300 rows and 2..=500 columns. The
  clamp applies to the PTY and the emulator alike, at launch and on every
  resize, so the two never differ.
- A panic inside the emulator is caught. The session then marks its screen
  as broken, and every later attach gets the raw ring, as before this ADR.

**D4. The emulator keeps 1000 lines of scrollback.** That is the page's own
xterm scrollback. The budget for 30 consoles is 160 MiB resident when every
console is full at 200 columns (measured: 153.3 MiB), plus the 512 KiB rings
(15 MiB). A console uses this memory only as its output scrolls (an empty one:
about 0.2 MiB resident).

**D5. A reattach gets a screen snapshot in place of the raw ring.** The
snapshot is escape sequences in ONE `Frame::Terminal` replay frame. The page
calls `term.reset()` and writes it, as it does today; it needs no new parser.
In order, the snapshot writes:
1. The primary screen's scrollback and visible rows, from the oldest line
   down, with their colors and attributes. A wrapped row is joined to the
   next, so xterm wraps it at its own width. Every other row ends with CR LF.
   Trailing blank cells with default attributes are not written.
2. When the alternate screen is on: the primary cursor, `CSI ? 1049 h`, and
   the alternate rows. The primary screen and its scrollback stay under it.
3. The scroll region (DECSTBM), when one is set.
4. Each input mode that is on: mouse tracking 1000/1002/1003, mouse encoding
   1006 and 1005, bracketed paste 2004, application cursor (DECCKM),
   application keypad (DECKPAM), focus events 1004, the kitty keyboard flags
   (`CSI > n u`), insert mode, new-line mode, alternate scroll 1007, no
   auto-wrap, and origin mode.
5. The cursor position (a pending wrap is rebuilt by writing the last cell
   again), the current attributes, the cursor visibility and the cursor shape.
6. The window title (OSC 2), without control characters. It is kept because
   it is one short string, so it costs nothing. The page does not show it
   today.

Hyperlinks (OSC 8) are kept: the cells carry them, and the spike round trip
kept them. The snapshot never writes a clipboard sequence (OSC 52), a palette
change (OSC 4, 10, 11), the saved cursor (DECSC), the character sets or the
tab stops.

**D6. The daemon announces the session size.** A push frame with verb
`session-size` and payload `{rows, cols}` goes to the writer and to every
watcher: on attach (after `session-open`, before the replay) and after every
accepted resize. A watcher's terminal takes that size and stops fitting its
own box. When its box is smaller, its body scrolls; the font is not scaled.

**D7. A writer claim carries an attempt number.** The attach query key is
`attempt` (u64). The page keeps one counter per holder in the tab's
`sessionStorage` and raises it on every connect: launch, reattach, reconnect
and wake. Each new value is the larger of the clock in milliseconds and the
last value plus one, so it only grows. A duplicated tab and a fence popup copy
the `sessionStorage`, and so the holder; the clock keeps the claims of these
windows in the order of their connects. The daemon keeps the attempt of the
slot's writer. A claim with the same holder, a LOWER attempt and no
`takeover` is refused with the end reason `superseded`, and it evicts nobody.
A claim with `takeover` is never refused for its attempt: Take over is the
operator's own choice, and a page reconnect never sends it. A claim with the
same holder and an equal or higher attempt reclaims the slot, as today. A
claim with no attempt follows today's rule (an older page). The relay passes
`attempt` to a peer.

**D8. A reattach can resume from a byte offset.** The daemon counts every
output byte of a session (u64, under the scrollback lock) and keeps the count
at the last accepted resize. `session-open` carries `offset` (the count at the
end of the replay it sends) and `resume` (bool). The attach query key is
`since`. The daemon resumes, and sends only the ring bytes after `since`, when
`since` is inside the ring, not before the last accepted resize, and not
after the count. Otherwise it sends the snapshot (D5). On a resume the page
keeps its terminal and does not reset it. A page that resets its terminal
forgets its offset.

**D9. The replay frame can be gzip.** The attach query key `gzip=1` asks for
it. The daemon that hosts the session compresses the replay frame with
`flate2` and sets `session-open.replay_encoding = "gzip"`. Only the replay
frame is compressed; live frames are not. A peer that does not know the key
sends the replay raw and no `replay_encoding`, and the page then reads it raw.
The page decodes with `DecompressionStream("gzip")`, and live frames that
arrive during the decode wait behind the replay, in order. The traffic line
records the raw and the compressed replay bytes. permessage-deflate is
rejected for now (F7).

**D10. A slow link keeps a covered console awake longer.** The page measures
the handshake time of its last console socket (`onopen` minus the time of
`new WebSocket`). A handshake of 750 ms or more is a slow link: three round
trips (TCP, TLS, upgrade) at 250 ms, the round trip that only the slowest 10%
of remote sockets passed in the operator's log (p90 256 ms). On a slow link a
console waits 120 s, not 15 s, before it sleeps, and `wb-notes.ts` reads the
same delay. The GPU budget still applies.

**D11. #581 item 5 is decided by the size of the compressed snapshot on
wake.** If the compressed snapshot on wake, measured by the closing stage of
#689, is at most 64 KiB, item 5 is dropped: a dormant console keeps no bytes
and wakes with the snapshot. 64 KiB fits in the first three round trips of TCP
slow start (initial window of 10 segments: about 14 + 28 + 57 KiB), which is
under 1 s at the p90 round trip of remote sockets (256 ms). If it is larger,
a dormant console keeps its bytes and resumes by offset (D8). The spike
predicts 24 KB for a full 1000-line prose snapshot. The number on the iPhone
through dev tunnels is still to be confirmed by a person.

Decided on 2026-10-12: **item 5 is dropped.** The closing measure of #689
(`tests/browser/console/wb_console_screen_689.py`, Windows 11, loopback) put
the compressed snapshot on wake at 3 168 bytes (27 947 bytes before gzip),
well under 64 KiB. That number comes from the measure's flood pattern, which
compresses about 9 times; the spike's prose estimate (about 24 KB) is the
better guide for the iPhone check. Both are under 64 KiB. A dormant console
keeps no bytes. The iPhone number through dev tunnels is still to be
confirmed by a person.

**D12. The ring stays, at 512 KiB.** It serves the resume (D8), the fallback
of D3, and `read_output` (#679). The ring is no longer the replay of a
normal attach.

**D13. An older peer keeps working.** `PEER_PROTOCOL_VERSION` stays 4. Every
new `session-open` field is optional in the page's `SessionOpen`, and
`PEER_REPLY_PIN` gets the new hash with version 4. A peer that sends none of
the new fields gets the raw replay and today's rules.

**D14. The output an agent reads comes from the screen.** When `read_screen`
(#679) is built, it reads this emulator as plain text, not the raw ring.

## Consequences

- On Windows the snapshot is as good as ConPTY's own rendering, because the
  emulator reads what ConPTY wrote. On Linux and macOS it reads the child's
  bytes. A test that compares screens must use the same platform for both
  sides.
- The daemon owns about 330 lines of snapshot code that the crate does not
  have, and two work-arounds for private fields (the scroll region and the
  primary grid under the alternate screen). A new `alacritty_terminal`
  version can change both. The round-trip tests of D5 catch a change.
- `alacritty_terminal` brings 16 new crates into the lock file (55 lines of
  `cargo tree`), all MIT or Apache-2.0. Some are for its own PTY event loop,
  which the daemon does not use.
- A watcher whose box is smaller than the session size shows scroll bars
  instead of a narrow, broken screen.
- ADR-0067's prediction for a reattach after a long disconnection changes:
  the operator sees the screen and the last 1000 lines, not the last 512 KiB.

Two limits found while this decision was built:

- **A terminal refitted while its socket was dead.** The page keeps its
  byte count across a reconnect. When its box changed size while the socket
  was down, the resume tail (D8) can hold bytes written at the old size. The
  size the page sends on open makes the TUI repaint, which corrects the
  screen; a plain shell does not repaint.
- **A live receiver that lags (resolved).** A burst can outrun a slow socket, and the
  broadcast then drops bytes for it. The bridge starts that socket again: it
  takes a new snapshot, a new receiver and the size under the same lock as
  an attach, and sends a new `session-open` with `resume: false`, the size
  and the snapshot. The page resets its terminal and takes the new byte
  count. The traffic line counts it in `lagged_events`. Checked by the
  manager test `a_resync_after_a_lag_shows_the_session_screen`, the bridge
  test `a_lagged_receiver_sends_the_screen_again_from_a_new_offset` and the
  `wb-console-terminal` ui-test
  `a second session-open on one socket resets the terminal and sets the count`.
  A relaying daemon counts the replay of such a second `session-open` as live
  bytes in its own traffic line.

## Considered options

- **vt100.** No reflow; 0 of 3000 lines of scrollback while a scroll region is
  set (Codex's input box sets one); the focus and kitty keyboard modes are
  lost from its snapshot; it panics at a 1x1 resize. A fork exists only to fix
  the scroll-region case.
- **Keep the raw ring, with a larger cap.** The bytes stay cut at any byte and
  written at several widths (F3). A larger ring makes the cut rarer and the
  replay larger.
- **permessage-deflate.** Not in `tungstenite` 0.29; a change of WebSocket
  library means leaving axum's extractor (F7). Whether dev tunnels pass the
  extension is not known.

## Compliance

Rust tests named here are in `crates/ralphy-daemon/src/`:
`session/screen/tests.rs` (the screen), `session/manager/tests.rs` (the
manager), `routes/ws_session/bridge.rs` (the bridge) and `tests.rs` (the
relay). Browser checks are in `tests/browser/console/`; they are not CI tests.

- D1: not checked by code. The crate is named only in `session/screen.rs` and
  `session/screen/`; a test in `crates/xtask/tests/` that fails on any other
  file is still to be written.
- D2: checked by the screen test `a_synchronized_update_is_applied_at_once`.
- D3: checked by the screen tests
  `a_string_past_the_cap_is_ended_and_its_rest_dropped`, `sizes_are_clamped`
  and `hostile_output_does_not_panic`, the manager tests
  `a_resize_gives_the_screen_the_clamped_session_size`,
  `a_broken_screen_falls_back_to_the_raw_ring` and
  `a_resume_still_works_after_the_screen_broke`.
- D4: checked by the screen test `the_scrollback_keeps_its_line_count` and
  the manager test `thirty_full_sessions_keep_only_the_scrollback_lines`. The
  resident memory of 30 consoles is a measure, not a test.
- D5: checked by the screen round-trip tests (a width change, each input
  mode, the cursor shape, the alternate screen, the scroll region, a wrapped
  row before an empty row, the pending wrap, the title and a hyperlink), the
  manager tests `a_replay_rebuilds_the_screen_after_a_width_change`,
  `every_line_is_in_the_replay_or_on_the_live_receiver_once` and
  `a_watcher_gets_the_same_replay_as_the_writer`, and the browser check
  `wb_console_screen_689.py` (`screen_equal` for reconnect, wake and reload).
- D6: checked by the bridge test
  `a_watcher_gets_the_size_before_the_replay_and_on_each_resize`, the manager
  test `a_resize_by_the_writer_reaches_every_attachment`, the
  `wb-console-fit` and `wb-console-frames` ui-tests, `session-size` in the
  push verbs of `crates/xtask/tests/ui_contract.rs`, and the browser check
  `wb_console_watch_size_689.py`.
- D7: checked by the manager tests
  `a_lower_attempt_of_the_same_holder_is_superseded_and_evicts_nobody`,
  `a_take_over_with_a_lower_attempt_of_the_same_holder_evicts_the_writer`,
  `another_holder_is_busy_and_a_take_over_still_evicts` and
  `without_an_attempt_on_either_side_the_holder_reclaims`, the relay test
  `peer_session_query_forwards_the_attempt_with_the_holder`, the
  `wb-session-route` ui-tests of the attempt, and the browser check
  `wb_console_late_socket_598.py`.
- D8: checked by the manager tests
  `a_since_inside_the_ring_resumes_with_the_bytes_after_it`,
  `a_since_that_cannot_resume_gets_the_snapshot`,
  `a_since_before_the_ring_start_gets_the_snapshot`,
  `a_resume_while_publishing_gets_every_byte_after_since_once` and
  `an_attachment_says_its_offset_and_whether_it_resumed`, the bridge test
  `session_open_says_the_offset_and_whether_the_replay_resumes`, the relay
  test `peer_session_query_forwards_since_on_a_reattach`, the
  `wb-console-terminal` ui-tests of the offset, and the browser check
  `wb_console_screen_689.py` (`resume`).
- D9: checked by the bridge test `the_replay_is_gzipped_only_when_the_tab_asks`,
  the traffic test `a_relayed_gzip_replay_is_counted_on_the_wire_and_raw`, the
  relay test `peer_session_query_forwards_gzip_on_every_shape`, the
  `wb-console-frames` ui-tests of the gzip queue, and the browser check
  `wb_console_gzip_689.py`.
- D10: checked by the `wb-console-gpu` ui-tests
  `dormantAfterMs waits longer only after a slow handshake` and
  `dormantAfterMs reads the handshake the last console socket reported`.
- D11: not checked by code: a measure. `wb_console_screen_689.py` writes
  `wake_gzip_bytes`; the decision and its number are in D11.
- D12: checked by `crates/ralphy-daemon/src/session/manager/tests.rs`
  (`scrollback_cap_is_512_kib`).
- D13: checked by `crates/xtask/tests/ratchets.rs`
  (`peer_reply_types_are_pinned_to_the_protocol_version`).
- D14: not checked by code: `read_screen` is not built yet (#679).
