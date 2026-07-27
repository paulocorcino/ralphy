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

- `GET /api/desk` → the records, in layout order.
- `PUT /api/desk` → replaces them wholesale; the daemon prunes to the cap
  (24 records, newest by `ts`) and persists.

Whole-array, not per-record: the shell already computes the full desk on every
mutation (`loadDesk` → upsert → `saveDesk`), so a record-granular API would be a
second model of the same state. Writes are **debounced** in the shell and
resolve **last-write-wins** — no ETag, no merge, no lock. The daemon is a solo
developer's (ADR-0032), so concurrent desks are not a real contention case, and
the cost of guessing wrong is a window in the wrong place.

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
