# The workbench shows only what it has read, and says when a read failed

Status: proposed
Kind: structural
Protects: consistency of the workbench, integrity of change

## Context

A **shown fact** (CONTEXT.md) is a fact the workbench shows whose owner is
outside the browser: the session list, the desk layout, the tree, the change
set, the branch, the runs, the board, the project list, the peers. No ADR says
who owns each one, when the browser reads it again, or what the screen shows
when a read fails. Each panel decides for itself.

What was measured on 2026-09-30 (`git log --since=2026-06-01 --no-merges`;
the classes are a reviewer's reading of each commit, see
`docs/architecture-diagnosis-2026-09-30.md` §11):

- 418 of the commits since June are fixes. 225 of them touch the
  browser↔daemon seam (`assets/ui/`, `src/routes*`, `src/peer*`). Only 12
  touch the Rust route handlers; `app.js` alone is in 101.
- The largest class that involves the daemon is **state that disagrees with
  its owner**: 39 fixes. Several were fixed again and again: a failed read
  shown as empty or clean (5 fixes, for example `79f9bfce`), desk writes
  racing (4), a stale branch after HEAD moves (3), data not read again after
  login (3). A contract mismatch (a field or error text the other side did not
  send) is 16 fixes.
- How each shown fact is read again today:
  - by push: the tree, the runs, the change set, and the branch of a local
    repo (`tree.dirty`, `runs.dirty`, `changes.dirty`, `head.dirty`);
  - by poll: the session list, every 2 s, driven by the presence frame and so
    also in a hidden tab (`assets/ui/app.js:306-314`);
  - only when the page loads: the desk, the project list and the peers' state.
    A peer repo gets no `head.dirty` (`src/routes/ws_tree.rs:149`, on purpose:
    an older peer would loop).
- The owner can hide a failure too. `desk::load_from`
  (`src/desk.rs:419-437`) reads an unreadable `desk.toml` as an empty desk.
  `PUT /api/desk` then merges into that empty desk and writes it over the file
  it could not read (`src/routes/api_read.rs:478-495`). `peer::read_store`
  serves no peers when the store directory cannot be read (`src/peer.rs`).
- The presence frame carries no version (`src/protocol.rs:50`). Only the tab
  that pressed Update reloads (`awaitNewBuild`, `assets/ui/app.js:3104`);
  every other tab runs the old JavaScript against the new daemon.
- Of 94 message types on the seam (49 verbs, 27 REST routes, 18 WebSocket
  messages), about 5 have a test that checks both sides agree. The JS tests
  feed hand-written replies. The 86 Playwright scripts in
  `tests/browser/<area>/wb_*.py` are not run in CI.

## Decision

**D1. Every shown fact has one owner, named in the fact index.** The fact
index in `docs/ARCHITECTURE.md` §7 lists each shown fact with its owner and
the events on which the workbench reads it again. A new panel adds its row
before it adds code. The browser is never the owner of a shown fact; what the
browser owns is the **per-client view**.

**D2. The workbench reads a shown fact when its panel opens, and again only
on these events:**

1. a push from the owner for that fact (`<fact>.dirty`);
2. the socket that carries that push opens again (a push may have been lost);
3. the tab becomes visible;
4. login, or a change of identity;
5. the reply to the operator's own action that changed the fact;
6. a periodic read, only when the owner cannot push (the forge; a fact of a
   peer that does not push it yet), with a period written in the fact index,
   and only while the tab is visible.

Every shown fact has events 2, 3 and 4. A hidden tab reads nothing; it reads
again when it becomes visible. When the owner is the daemon, the fact has a
push (event 1), not a periodic read.

**D3. A failed read shows as a failure, never as empty or clean.**

- If there was no good read yet, the panel says the read failed and why.
- If there was a good read before, the panel keeps that value, marked as not
  current, with the time of the good read and the reason for the failure.
- While a value is marked as not current, every action that writes on the
  basis of that value is disabled (for example stage, commit, discard, or
  moving a board card).

**D4. An owner never answers "empty" for a store it could not read, and never
writes over it.** A store that does not exist reads as empty. A store that
exists but cannot be read or parsed is a failure: the read returns the
failure, and a write to it is refused. The daemon still starts. The operator
can start a new store; that action renames the old file to
`<name>.unreadable-<date>` first, so nothing is deleted. The action is
offered only for a store that cannot be parsed. A store that cannot be read
is tried once more, and if the read still fails it is a failure without that
action: a read error may pass (an antivirus or an indexer that holds the
file), and the file behind it may be fine. This applies to every
file the daemon owns: the desk, the peer store, the registry and settings.

**D5. A shown fact the browser writes is merged by its owner, per record, and
the owner pushes the change.** Today this is the desk layout: the daemon
merges each window record by its change time, as it does now, and pushes
`desk.dirty` so that other open tabs read it again.

**D6. The browser and the daemon know each other's build.** The presence
frame carries the daemon's build id, and the page carries the build id it was
served with. When they differ:

- with no unsaved work in the tab, the tab reloads itself;
- with unsaved work (a modified file in the editor, a note being edited), the
  tab does not reload. It shows a notice that stays until the reload, allows
  only saving the open work, and disables other writes as in D3.

A console does not hold back the reload: the daemon owns the PTY, and the
console reattaches after the reload. A file detached into its own window is
unsaved work of the tab that opened it: the window saves through that tab,
and a reloaded tab no longer hears it. Before the reload, the tab closes its
detached file windows, which sends each file home as a tab.

## Consequences

- This adds to ADR-0036 §8. That section splits state by lifetime (presence,
  session, run); this ADR says who owns what the screen shows and when it is
  read again. ADR-0050 (the desk) and ADR-0067 (peers) gain D4 and D5.
- The 2 s session-list poll ends. The daemon pushes `sessions.dirty`, and the
  per-peer fan-out of `/api/sessions` runs only when something changed.
- The project list and the peers' state, now read only at load, gain events 2,
  3 and 4.
- A peer repo's branch stays on a periodic read (event 6) until peers push
  `head.dirty`.
- Accepted risk: D5 merges by the time the browser wrote into the record. Two
  devices whose clocks differ can pick the wrong winner for the same window.
  This was not seen; the 4 desk fixes were races inside one tab.
- A corrupt `desk.toml` now shows as a failure with an action, instead of an
  empty stage. That is more friction, and it is the point: the empty stage
  hid a loss.

## Considered options

- **Remove the old value when a read fails** (instead of D3's "keep, marked as
  not current"). Rejected: the operator loses what was last known, for example
  which files were changed on a peer that went offline.
- **A revision from the daemon, and refuse a write based on an old one
  (ETag).** Rejected for the desk: it refuses the whole desk when two tabs
  changed different windows, and the measured desk bugs were not conflicts
  between tabs.
- **Run the Playwright scripts in CI.** Not now: Python and browsers on three
  operating systems is the most expensive check. It comes back only if the
  shared replies in Compliance do not hold the contract.

## Compliance

- D1: not checked by code. The "Read again on" column of the fact index is the
  review checklist; a PR that adds a panel fills its row.
- D2: not checked by code: whether a panel reads again on the right events is
  a property of the JS control flow, which a pattern cannot find reliably.
  Reviewed in the PR against the fact index.
- D3, D4, D5, D6: not checked by code; each is pinned by behaviour tests where
  it is built. These rules change what one path does, not a boundary a pattern
  can find.
- Contract of the seam (what D1–D3 rely on): checked by three tests, not built
  yet:
  - **Shared replies**, as a ratchet. The Rust tests that already produce real
    replies write them to `crates/ralphy-daemon/ui-tests/fixtures/<type>.json`,
    and the UI tests run the real JS folds on the same files. Baseline: about
    89 of 94 message types have no shared reply; the check fails if that
    count goes up, and a new verb, route or message starts with one.
  - **Error literals**: every string the JS compares with a reply's `reason`,
    `message` or `state`, and every key of the cause table in
    `assets/ui/wb-fail.js`, appears as a literal in the daemon's or the CLI's
    Rust source.
  - **Mirrored constants**: a table pairs each limit the JS repeats (desk,
    fence and note caps, console name length, image size, frame tags, the
    write denylist) with its Rust constant, and the test compares the values.
    It follows the settings test `every_settable_key_the_panel_offers_is_a_key_the_cli_accepts`
    (`crates/ralphy-daemon/src/tests.rs`).
