# The desk layout is daemon state, not browser state

Status: accepted.

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
