# The desk layout is daemon state, not browser state

Status: accepted.

Extended by ADR-0070 (proposed): an unreadable desk file is a failure, never an empty desk, and is never written over (D4); the daemon pushes `desk.dirty` to other tabs (D5).

The **desk** is the workbench's console layout: which consoles were open and
where each window sat on the stage. It was hardened in the shell during PRD #185
and has lived in the browser's `localStorage` (`wb.desk.v1`) ever since — a
choice never recorded in an ADR, and wrong for the way the daemon is actually
used.

The daemon is reachable from anywhere the operator is (ADR-0032; over a dev
tunnel, from a second machine). A **workbench session survives the browser** —
it is a daemon-owned PTY, detached and reattached at will. The desk does not: it
is scoped to one browser profile on one machine. Open the workbench from a
different computer and the sessions come back while the layout does not, so
every window lands on the cascade fallback and the operator re-arranges the
stage by hand. The state that describes daemon-owned sessions was living on the
wrong side of the seam.

Vocabulary (**canvas**, **Consoles tab**, **workbench session**, **free
console**) lives in [CONTEXT.md](../../CONTEXT.md); the canvas structure lives in
[ADR-0037](0037-workbench-canvas-tabbed-workspace.md); the workbench↔daemon
protocol lives in [ADR-0036](0036-workbench-daemon-integration-protocol.md).

## Decision

### 1. The daemon owns the desk

The desk is persisted in the global daemon store as `desk.toml`, beside
`daemon.toml` (identity) and `repos.toml` (registry), rooted at
`$RALPHY_DAEMON_DIR` when set. It is **typed**, not an opaque client blob: one
table per record, carrying the fields the shell already writes — the stable
client-side `id`, `repo`, `agent`, `kind`, the `rect` (`left`/`top`/`width`/
`height`), the `max` flag, the volatile `session_id`, and `ts`. Typing it lets
the daemon enforce the cap itself rather than trusting whatever a browser
uploads.

### 2. Two routes, whole-array semantics

- `GET /api/desk` → the desk as `{ windows, fences }`, each in layout order.
- `PUT /api/desk` → replaces them wholesale; the daemon prunes each record type
  to its own cap (24 windows, 12 fences, newest by `ts`) and persists. The body
  became an object in #340, when fences joined the store (ADR-0051 §10).
  *Since the amendment of 2026-09-20 a body carrying `removed` is folded into
  the store instead — see below.*

Whole-array, not per-record: the shell already computes the full desk on every
mutation (`loadDesk` → upsert → `saveDesk`), so a record-granular API would be a
second model of the same state. Writes are **debounced** in the shell and
resolve **last-write-wins** — no ETag, no merge, no lock. The daemon is a solo
developer's (ADR-0032), so concurrent desks are not a real contention case, and
the cost of guessing wrong is a window in the wrong place.

> **Narrowed by the amendment of 2026-09-20 below.** The premise held for one
> browser at a time; a solo developer with the workbench open on a phone, a
> tablet and a laptop at once is three pages on one desk, and the cost of
> guessing wrong turned out to be a console coming back twice.
>
> **Superseded by the amendment of 2026-10-04 (changes, not the desk).** The
> PUT carries a list of changes that the daemon applies in arrival order.

### 3. One store, no browser fallback

`localStorage` is dropped entirely, including the retired `wb.console.geometry.v1`
eviction. Nothing is lost by this: `restoreDesk` already returns early when
`WBMode.isDaemon()` is false, so the static demo never restored a desk — the
browser store was only accumulating records it would not read. A second store
that is authoritative in no mode is not a fallback.

> **Narrowed by [ADR-0051](0051-consoles-stage-plane-and-fences.md) §8 (issue
> #339).** What is dropped is a second copy of the **desk**. The **per-client
> view** — the viewport offset and the open file tabs — is different state with
> a different lifetime and lives in the browser under one key, `wb.view.v1`:
> shared, one operator's panning would drag another's view. The desk (windows,
> and later fences) stays daemon-owned.

### 4. Restoration and the smaller screen are unchanged

`reconcileDesk` keeps its shape — a pure fold of layout over the daemon's live
sessions, with its four verdicts (`attach`, `relaunch`, `placeholder`, `adopt`).
This ADR changes where the layout comes *from*, not how it is reconciled.

A rect is likewise still stored as **absolute pixels**. A desk saved on a large
monitor and restored on a smaller one needs no special handling, because
`clampAll` already resizes and repositions every window to fit `#workspace` and
already runs from a `ResizeObserver` on it. Out-of-bounds windows are pulled in
by machinery that exists.

> **Superseded by [ADR-0051](0051-consoles-stage-plane-and-fences.md) §4 (issue
> #336).** `clampAll` and its `ResizeObserver` are deleted: the rect stays
> absolute pixels, but its frame is the **stage**, a plane the viewport scrolls
> over. An out-of-bounds window is reached by scrolling, never pulled in.

## Rejected alternatives

- **Keep the desk in `localStorage` and sync it.** Rejected: two stores and a
  reconciliation between them, to arrive at the state one store gives directly.
- **A desk per project, or per resolution.** Rejected as speculative
  generality: neither has a demanded use, and both multiply the store by a key
  nobody asked for. Global is what the operator asked for.
- **A desk per operator.** Rejected for now: the daemon is scoped to a solo
  developer (ADR-0032). If multi-operator daemons arrive, the desk is keyed by
  identity then — the store already sits beside `daemon.toml`, which holds the
  identity to key it by.
- **A proportional rect (fractions of the workspace) or a desk keyed by
  viewport size.** Rejected: `clampAll` already solves the smaller-screen case,
  so both are new mechanism for a handled problem. (The rejection stands under
  [ADR-0051](0051-consoles-stage-plane-and-fences.md) §4, which superseded the
  reason: the smaller-screen case is now solved by SCROLLING, not by clamping.)
- **Per-record `POST`/`DELETE` routes.** Rejected: the shell has no per-record
  path to drive them; it already holds the whole desk in hand at every write.

## Consequences

- The layout follows the operator across machines, as the sessions already do.
- The desk becomes daemon state and gains that lifecycle: it survives the
  browser, and it is a file an operator can inspect or delete.
- Every desk mutation is now a network write, not a synchronous local one —
  hence the debounce. A desk write failing must never break the window
  interaction that triggered it; a lost write costs a stale position, and the
  next mutation supersedes it.
- `$RALPHY_DAEMON_DIR` now scopes the desk too, so a scratch-store daemon (the
  visual-test path) starts with an empty stage instead of inheriting the
  operator's.

## Amendment (2026-09-15, ADR-0063): `checkouts`

The desk document gains a third record type: **`checkouts`**, a map
`{ <repo-ref>: <worktree name> }` — the selected checkout per project
(ADR-0063 §4), `[checkouts]` in `desk.toml`, declared LAST so the table lands
at top level after `[[fences]]`. One selection per project; the value is a
worktree NAME, stored, not validated per read — a listing per desk read would
be a spawn — with only the name shape gated on the PUT (`400` for anything
that is not one path component). Omitted when empty, so an old desk and an old
shell keep their exact `{ windows, fences }` shape. The shell mirrors it like
fences (local wins per ref, a cleared ref stays cleared over a later-arriving
GET), and drops an entry on the first `unknown checkout` reply from any verb —
the worktree is gone, and the primary tree is shown. Same last-write-wins
consequence as the rest of the desk.

## Amendment (2026-09-15, #411): `checkout` on the window record

A window record gains an optional **`checkout`** — the worktree NAME the
console was launched in (ADR-0063 §3), written by the shell from the daemon's
own `session-open` announcement (and, before that answers, from the launch
request, so a daemon that dies mid-launch still leaves the intent behind).
`None` is the primary tree and is not serialised, so a pre-#411 desk and shell
keep their exact record shape. The PUT gates it with the same name check as
`checkouts`. It is what a relaunch reads: the `relaunch` verdict, the
placeholder's button and a window's own restart all request the recorded
worktree — the per-repo `checkouts` selection is never consulted for a console
that already exists. When the recorded worktree no longer exists the shell
asks first (an Observe read carrying the checkout answers `unknown checkout`
without a spawn) and renders a placeholder naming it, whose one button
relaunches on the primary and clears the field; a console is never moved to
a tree the operator did not pick.

## Amendment (2026-09-16, ADR-0036): records are served and stored under the registry's canonical key

A record's `repo` and a `checkouts` key are the registry's project key, and
that key can be re-keyed once (`path-<hash>` → `owner/repo`, ADR-0036
amendment 2026-09-16). Both desk routes rewrite a former slug to its canonical
key through the registry's `former_slugs` — on the way out, so a migrated
project's consoles come back to it; on the way in, so a tab that read the
desk before the migration cannot write the old key back. The desk file itself
is never rewritten by the migration: it converges on the first save.

## Amendment (2026-09-20): the PUT is a fold, a page reads before it writes, and a session finds its record

Three pages on one desk (a phone, a tablet, a laptop)
showed the hole: a page's mirror is only as fresh as its last `GET`, so a
drag on the tablet uploaded a desk that did not know about the console the
laptop had just opened — the record, and with it the `sessionId`, was gone.
The next page to load found a live session no record claimed, adopted it into
a fresh record, and the laptop's own copy came back on its next flush: two
records, one session, and on every load after that the loser was a placeholder
beside the live window — or, for a shell, a second PTY.

Three rules close it. The first changes §2's wire semantics; the other two
are the shell's.

> **The first two rules are superseded by the amendment of 2026-10-04
> (changes, not the desk).** The third, a session finds its record, stays.

- **The PUT folds.** The body gains `removed: { windows, fences, checkouts }`
  — the ids this page deleted since it loaded — and a body carrying it is
  FOLDED into the stored desk rather than replacing it: per id the newer `ts`
  wins (a tie goes to the upload), an id named in `removed` is dropped
  whatever the store holds, a stored record the body does not mention
  survives; checkouts have no `ts`, so the body's entry wins per ref. The
  fold runs under a process-wide lock — read, fold, write as one step — so
  two pages flushing at once cannot drop each other's fold. Deletion has to
  be SAID because absence no longer means it: a record missing from a body
  is one the page may not have read yet. A body without `removed` is a shell
  from before this amendment; it cannot say what it deleted, so it is the
  wholesale replace it always was — the one place the old semantics remain.
  Still no ETag: the fold makes the write commutative enough that a version
  check would only ever refuse a write the fold can absorb.
- **Read before write.** Each debounced flush `GET`s the desk and folds it
  through `mergeDesk` before it `PUT`s: per record id the copy with the newest
  `ts` wins (a local mutation is newer by construction; a stale local copy is
  not), a record this page deleted stays deleted, and the other pages' records
  come in. The flushes of a page are chained in the shell, so a slow read
  cannot reorder two writes. With the fold on the daemon this is belt and
  braces: it keeps the page's own mirror current, which is what `reconcileDesk`
  and the cap's live-window pinning read.
- **A session finds its record.** In `reconcileDesk`, a live session no
  record claims by `sessionId` first looks for a record waiting on the same
  repo, vendor, kind and worktree — a placeholder, or a shell that would have
  relaunched — and attaches there. Only with no such record is it adopted into
  a fresh one. This also absorbs a daemon restart that reissues ids: the
  relaunched console lands in the box it left.

## Amendment (2026-09-20, lock): `locked` on the window record and on the fence

A window record and a fence each gain a boolean **`locked`** — the operator
pinned it in place. `false` is the default and is not serialised, so a pre-lock
desk and a pre-lock shell keep their exact record shape (the #411 template).
It is a plain `bool` on the wire, never `null`: a shell sending `null` has its
PUT refused, and that is pinned rather than papered over with an `Option`.

- **Who writes it.** The shell, from the lock button on a console's title bar
  and the lock tool on a fence's head, through the same `persistWin` /
  `saveFences` writes every other layout act uses — so the toggle bumps `ts`
  and rides the fold like any other field. The daemon validates nothing about
  it: the shell is what refuses the gesture, and a PUT carrying a new rect on
  a locked record is a legitimate unlock-then-move from another page that the
  fold arbitrates by `ts` as usual.
- **Who reads it.** Every client, at restore and on each `GET`: a locked
  console refuses drag and resize (maximize, fullscreen and close still work —
  they do not rewrite the rect); a locked fence refuses move, resize and tile,
  and the consoles it holds — membership still derived, ADR-0051 §6 — refuse a
  drag while it holds them. The lock is the one record field the shell applies
  from the mirror onto a live window *without* a reload, because the
  alternative is a page whose next drag uploads `locked:false` with a newer
  `ts` and wins. Rects are still never applied from the mirror mid-session.
- **Why shared, not per client.** ADR-0051 §8 keeps detach per client because
  detach is the presentation of one operator's screen. A lock protects the
  layout itself — the thing every device shows — so it is desk state, and the
  cost is the same last-write-wins §8 already accepts: a lock set on one device
  reaches another on that device's next `GET` (login, or the read-before-write
  of its next flush), and a drag squeezed in before that read wins the fold.
  A desk push channel would close that window and is not part of this
  amendment.
- Prompted by the iPad: a finger tapping a title bar slid the console out of
  its fence. The lock is the belt; the drag threshold (`dragThreshold`, 4px for
  a mouse and 10px for a finger, shipped with it) is the braces — a press under
  it is a tap, moves nothing and persists nothing.

## Amendment (2026-09-27, ADR-0066): `consoleName` on the window record

A window record gains an optional string **`consoleName`**: the name a person
reads for the console, such as `fincal #1`. `None` is not serialised, so an
older desk and an older shell keep their exact record shape (the #411
template). The daemon cuts it to 40 characters when it stores the desk. Like
`locked`, the shell applies it from the mirror onto a live window, so another
page's next drag cannot upload an old name with a newer `ts`. When the
record that wins the fold has no `consoleName`, the fold keeps the stored
one: only a shell that does not know the field sends a record without it. How the name is
chosen, shown and passed to Claude is
[ADR-0066](./0066-console-names.md).

## Amendment (2026-09-30): rules recorded from the glossary

These rules were decided earlier and were recorded only in CONTEXT.md. On 2026-09-30 the glossary was cut back to definitions, so the rules move here without change. Nothing new is decided.

- **Restoration is asymmetric on purpose.** A **free console** relaunches by itself (a shell is free and idempotent), while an agent console returns as a **placeholder** the operator reconnects with one click: loading a page must never spawn vendor CLIs and spend quota nobody authorised.

> **Corrected by the amendment of 2026-10-04 below.** A shell is free, but its
> launch was not idempotent: two pages that load at the same time each start
> one.

## Amendment (2026-10-04): a launch names its window record, and the daemon keeps one session per record

On 2026-10-03 the daemon restarted, and two pages loaded the desk 4 seconds
apart. Each page relaunched the same free consoles, so the daemon started 8
shells for 4 records (sessions 590–593 and 594–597). The fold of 2026-09-20
worked: each record kept one `sessionId`, and no write was lost. But 4 shells
had no record. On 2026-10-04 a third page loaded, found those 4 sessions, and
adopted each one into a fresh record at the cascade position. The operator saw
consoles leave their places, and each later load on two devices made more
of them.

The fault is not a double write. It is a double **side effect**: loading a page
starts a process, and the daemon cannot tell that two launches are for the same
console. Only the daemon sees the launches of every page, so the rule lives
there. The link between a window record and its session becomes a fact the
daemon owns, not a field each page guesses and writes back.

- **A launch names its record.** A new launch on `/ws/session` (an agent
  launch, and a `console=1` launch) carries `record=<window record id>`. A
  reattach by `id` does not carry it: the session already has one. A value
  that is not 1–64 characters of `A–Z`, `a–z`, `0–9`, `-` or `_` (the rule of
  `holder`) is treated as absent.
- **One live session per record.** When a launch names a record and a live
  session already carries that record, the daemon starts nothing. The request
  becomes a reattach to that session, with the normal writer-slot rules: the
  holder that owns the slot gets it back, and any other page attaches as a
  watcher (read-only, with "Take over"). It is never refused: the page asked
  for this console, and this console is running. The check and the claim are
  one step under one lock per record, and the claim is taken BEFORE the spawn,
  so a second launch that arrives during the spawn waits for it and then
  attaches. A check in the page before the launch is not enough: it cannot
  see a launch from another page that is still in flight.
- **The claim ends with the session.** A session leaves the session list when
  its child exits or when it is closed, and its record is free from that
  moment. A restart closes the old session first and then launches, so it
  starts a fresh one in the same record.
- **The session list says the record.** Each session in `/api/sessions`
  carries `record` (absent for a session launched without one).
  `reconcileDesk` attaches a record to the session that names it before it
  tries the `sessionId` tuple. A session that names a record no page has yet
  is adopted into a record with THAT id, so the page that launched it and the
  page that adopted it write the same record.
- **The cap is 30, and it never drops a record a session names.** The window
  cap goes from 24 to 30: one operator already kept 23 records in normal use.
  The daemon now knows which records have a live session, so its prune (the
  backstop for the cap) keeps them, as the shell already keeps the records of
  windows on its stage. When the pinned records are more than the cap, all of
  them stay and only unpinned records are dropped. A cap that deletes the
  record of a live console strands that console, and the next load adopts it
  at the cascade position: the same loop as above.
- **A full desk refuses a new console.** When the desk holds 30 window
  records, opening a NEW console is refused with a message that asks the
  operator to close one first, as a full set of notes already does. Nothing
  is cut in silence to make room. A relaunch, a restart and an adopt reuse
  or bring a record and are not refused.
- **Older peers and older daemons.** The query field is ignored by a daemon
  that does not know it, so a launch relayed to an older peer is not
  protected until that peer is updated. A page from before this amendment
  sends no `record`, and its launches start a session as before.
- **A list that did not hear from a peer decides nothing for that peer.**
  `/api/sessions` used to leave out a peer that did not answer, so the page
  read "no sessions there" and relaunched every shell of that peer. On a peer
  without the record join this started a second shell for each console, and
  the next load adopted the first ones in a cascade (measured on 2026-10-04:
  a Mac on rc.36 behind an ssh tunnel that dropped for a few seconds; 5
  consoles became 11 in a reproduction with a TCP proxy as the tunnel). Now
  the list names those peers in the response header `x-ralphy-unanswered`
  (comma-separated daemon ids; the body stays the array every reader parses).
  For a record of a peer the list did not hear from, the page does not
  relaunch, does not create or drop the record, and does not launch on a
  click. The window is a placeholder that says the peer does not answer, and
  asks the list again on each fleet read: it attaches when the session is
  there, and relaunches a shell only when the list heard from the peer and
  the session is not there. This protects the consoles on every peer
  version, the older ones included.

### Rejected alternatives

- **A version check (ETag) on the desk.** It refuses a write, but the second
  shell is already running when the write happens. It would also refuse every
  drag on one device that follows a drag on another.
- **Ask `/api/sessions` before the relaunch.** Two pages that load at the same
  moment both read "no session" and both launch. It narrows the window; it
  does not close it.
- **Only one page restores at a time (a lease).** A page that dies holding the
  lease blocks every other page until it times out, and the lease is a second
  copy of a fact the daemon already has.
- **Stop relaunching free consoles by themselves.** It removes the race by
  removing the feature. With the daemon's claim, the relaunch is safe.

## Amendment (2026-10-04): desk history

Bugs overwrote the desk twice in one day (the cap that cut a live record,
cedca761; a page that reconciled after a failed read, #537). Each time the
operator's layout was gone, because the daemon keeps one `desk.toml` and
rewrites it on every page load and every reconnect (measured on 2026-10-04:
written at 11:02 and again at 11:05 with no act by the operator). The daemon
now keeps a **desk history**: the last 50 versions of the desk layout, which
the operator can restore, download as a file, and upload again.

- **Where.** A directory `desk-history/` beside `desk.toml`, one JSON file
  per version, written owner-only and atomically like `desk.toml`. The file
  is also the download format: `{ kind: "ralphy-desk-version", id,
  startedAt, savedAt, reason, desk }`. The `id` is the epoch ms of the
  version's first change. `reason` is `change`, `before-restore`, `restore`
  or `upload`.
- **When a version is written.** After a `PUT /api/desk` that changed the
  stored desk, under the same lock as the write. A change of `ts` or
  `sessionId` alone, or a rect that moves by less than 1 px, is not a layout
  change and writes nothing: a reconnect rewrites both. A change that comes
  less than 60 s after the FIRST change of the newest version goes into that
  version; otherwise it starts a new one. A `restore` or `upload` version
  takes such changes too: a page that reloads after a restore gives its
  consoles their names and writes at once (seen in the browser check). A
  `before-restore` version never takes a change: it is the way back. A version therefore holds at most 60 s of changes: one drag is one
  version, and a long arrangement is one version per minute. When the newest
  version does not hold the desk as it was before the change, that desk is
  written first, so the layout before a change is never lost. Beyond 50
  versions the oldest is deleted. A history write that fails is logged and
  never fails the desk write, which already succeeded.
- **Restore.** `POST /api/desk/history` with `{ id }` (a saved version) or
  `{ version }` (an uploaded file, checked with the same rules as a PUT
  body). The daemon always writes the current desk as a `before-restore`
  version first, even when an older version holds the same layout, so the
  way back is the row just under the restore. A failure there stops the
  restore. Then it builds the new desk:
  - a window of the version takes its saved place. When a current record is
    the same console (same id, or the same live session), the version's
    layout goes onto that record and the record keeps its session, so a
    running console is never shown twice;
  - a current window that is not in the version stays where it is when it
    has a session, and is dropped when it is a placeholder: a restore never
    ends a running console;
  - fences and note cards are the version's. A card is only a place on the
    stage, so the text of a note does not change;
  - the selected checkout per project is kept: it is a selection, not
    layout.

  Every restored record gets `ts = now`, the result is written as a
  `restore` or `upload` version, and the daemon pushes `desk.dirty`.
- **The guard: `generation`.** An open page never applies a rect from the
  desk onto a live window (§4, and the lock amendment), and each reconnect
  persists the window's live rect with a fresh `ts`. So a page that loaded
  before the restore would upload its old rects with a newer `ts`, and the
  fold would undo the restore. A push cannot prevent this: a phone that
  sleeps does not get it, and it uploads when it wakes. The desk therefore
  carries `generation`, the epoch ms of the last restore (`0`, and not
  written, until the first one). A page sends the generation it loaded with
  every PUT, and the daemon refuses an older one with
  `409 {"state":"restored"}` before it writes anything. A page that sees a
  newer generation, in a refused PUT or in any desk read, stops writing and
  reloads: there is no way to clear a page's windows and run the restore
  again in place.

### Rejected alternatives

- **One backup copy of `desk.toml` per write.** The desk is rewritten on
  every load and reconnect, so the copy would hold the bad desk minutes
  after the bug.
- **A timer in the daemon that writes a version after a quiet period.** A
  background task for what one comparison on the write path decides; the
  rule "60 s from the first change" needs no timer.
- **A new push verb for a restore.** `desk.dirty` already makes every page
  read the desk, and the generation in that read is what makes it reload.
- **Restore in place, without a reload.** `restoreDesk` reconciles once per
  page by design (#537), and the rects of live windows are never applied
  from the desk. Both would have to change for a rare act.

## Amendment (2026-10-04): the desk upload carries changes, not the desk

One operator uses a phone, a tablet and a PC within seconds. Measured on
2026-10-04 on a scratch daemon: the PC moved a window from left 100 to 400.
The phone still showed 100, because an open page never applied a rect from
the desk. The phone's socket reconnected, the page wrote the window's record
again, and the daemon held 100 again. The PC's move was lost.

The cause is the shape of the write. Each page uploaded WHOLE records, built
from its own screen and stamped with its own clock, and the daemon kept the
record with the newest `ts`. Seven of the eleven places that wrote a window
record were not about the rect (a rename, a lock, a reconnect, a load), but
each one wrote the rect too, with a fresh `ts`. The same fault was in the
other record types: a page that had changed one fence kept its whole local
copy of every fence, and each flush sent the whole `checkouts` map. Each fix
since 2026-09-20 (the fold, the lock and name amendments, the double launch,
the cap, #537, the history) closed one way in. This amendment changes the
write so that a page can only send what it changed.

- **The body is a list of changes.** `PUT /api/desk?tab=<id>` takes
  `{ seq, generation, changes: [ … ] }`. Each change is one of:
  - `{ op: "create", type, record }`: a new window, fence or note card;
  - `{ op: "set", type, id, fields }`: some fields of one record;
  - `{ op: "remove", type, id }`;
  - `{ op: "checkout", repo, name }`: select a worktree for a project;
  - `{ op: "checkout-clear", repo, ifName }`: clear it, only if it still
    holds `ifName`.

  `type` is `window`, `fence` or `note`. A `set` may name only these fields:
  for a window `rect`, `max`, `locked`, `consoleName`, `checkout`, and
  `session` (`sessionId`, `daemonId` and `environment` as one unit); for a
  fence `rect`, `name`, `locked`; for a note card `rect`, `locked`, and
  `file` (`repo`, `path` and `checkout` as one unit). A page computes the
  value at the act (the rect at the end of a drag, the tiles of a tile), so
  it never writes a value it read from a screen that may be stale.
- **The daemon applies the changes in the order they arrive.** No client
  clock decides anything. Under the desk lock, each change is checked and
  applied to the stored desk:
  - a `set` or `remove` of a record that does not exist is ignored: another
    device deleted it, and a change to it must not bring it back;
  - a `create` of an id that exists applies only its `session`: a page that
    adopted a running console never writes its cascade rect over the saved
    one;
  - a `set` to the value already stored changes nothing;
  - a change that fails a check (a rect off the stage, a checkout that is not
    one path component, a field not in the list, a `create` over the cap) is
    skipped. The reply names it in `refused: [{ index, error }]`, and the
    other changes of the body still apply;
  - every repo key goes through the registry's former slugs first.

  `ts` is now the server time of a record's last change, kept for display
  and for the history only.
- **The caps apply to `create` only.** A `create` past the cap (30 windows,
  12 fences, 32 note cards) is refused, with a slack of 5 for windows: a
  running console must always be able to get a record back. Nothing is
  pruned by age on this path: a prune deletes a record that no page asked to
  delete. A restore still prunes, because it builds a whole desk.
- **`seq`: a resend never undoes a later change.** A page numbers each
  upload. The daemon keeps the last `seq` per tab id in memory (the last 64
  tabs). A body whose `seq` is not higher is ignored and answered with the
  current desk. A page resends a batch with the same `seq`, so a batch the
  daemon applied but whose reply was lost is not applied a second time, after
  another device changed the same field. The last upload when a page closes
  sends every change not yet answered, with a new `seq`.
- **`rev`: a page never takes an older view.** The desk carries `rev`, a
  counter the daemon raises on every write that changes the desk. The GET and
  the PUT return it. A page ignores a desk whose `rev` is lower than the last
  one it took, so a slow read cannot put an old desk on screen.
- **The page keeps the daemon's desk and its own pending changes apart.** It
  draws the daemon's last desk with its pending changes applied on top, by
  the same rules as the daemon. One table of cases,
  `ui-tests/fixtures/api-desk--apply-cases.json`, is run by the Rust test and by
  the node test. An upload that fails on the network, or with `401`, `409
  unreadable` or `5xx`, is kept and sent again later. A `400` or `422` drops
  it: the daemon will never accept it.
- **The screens converge.** When a page takes a desk, it applies the rect,
  the lock and the name of each window, fence and card to what it shows,
  except on an element under a gesture of the operator (from the press to the
  release) and except for a field with a pending change. `max` is not applied
  to an open page: a phone that maximizes a console must not maximize it on
  the PC. A window whose record was removed by another device is closed on
  this page when it is a placeholder or has ended; a running console keeps
  its window and gets a record again, because a running console always has
  one. A card with text not yet saved is kept. A console opened on another
  device appears at the next load: opening it live would attach or start a
  process without an act on this page.
- **An old page cannot write.** A body without `changes` is a page from
  before this amendment. The daemon refuses it with `409
  {"state":"restored"}` and writes nothing: a page from the desk history on
  reloads on that reply, and an older page loses that one write. A hidden
  tab that runs an older build than the daemon holds its writes (ADR-0070
  D6).
- **A new desk.** `POST /api/desk/new` sets `generation`, so a page that read
  the unreadable desk reloads instead of writing into the new one.

This supersedes, for the desk upload: §2's whole-array semantics and its
"last-write-wins, no ETag"; the 2026-09-20 rules "per id the newer `ts`
wins" and "read before write"; and the reason the lock and `consoleName`
amendments gave for applying those fields from the mirror, which is now the
general rule above. The `generation` of the desk history stays: it still
means "reload".

The model is the one Figma describes for its multiplayer editor, which also
has a central server: the server decides the order of changes, the last
change to a property wins, a client does not apply a server value over a
change of its own that the server has not answered, a deleted object takes
its later changes with it, and a client that comes back reads a fresh copy
and sends its changes again
(figma.com/blog/how-figmas-multiplayer-technology-works).

Known limits:

- Two devices that change the SAME field at the same moment: the change that
  reaches the daemon last wins. This is intended.
- A fence moved on one device while a member is dragged out of it on
  another, in the same seconds, can put the member back in the fence.
  Convergence makes that window a few seconds long.
- A console opened on another device appears at the next load, not live.

### Rejected alternatives

- **A CRDT library (Yjs, Automerge, Loro).** Yjs is about 18 kB of
  JavaScript; Loro (about 180 kB) and Automerge (about 320 kB) are WASM, and
  all three need a sync provider. A CRDT does not stop a page from writing a
  value it did not change: a reconnect that sets the rect is a new operation,
  and it wins. Automerge resolves a map key by the last writer, the rule used
  here. The daemon also does work on the desk that a CRDT would have to carry
  as its own transactions over a binary file: the caps, the slug re-key, the
  checks, the restore, the history, and the link between a session and its
  record. The change list above is the shape a CRDT would carry, so live
  editing of note text could still move to one later.
- **A version check (ETag) on the whole desk.** It refuses a drag on one
  device after a drag on another, when the two changed different windows.
- **A fence move as one relative change ("move this fence and its members
  by dx, dy").** A race between a fence move and a member's drag gives a
  wrong result either way, and convergence makes the window short. The page
  sends the absolute rect of each member.
- **Keep whole records, but stop the reconnect from writing.** That closes
  one of the seven writers that were not about the rect, and the next new
  writer opens the bug again.
