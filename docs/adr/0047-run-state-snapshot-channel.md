# The run publishes a run snapshot on disk; the daemon reads it

Status: **accepted** (2026-07-24, issue #298, under PRD #296) — decided, not yet
implemented. This ADR gates the run-state track: the implementation issues under
#296 build what is written here.

_Extends [ADR-0024](./0024-unified-delivery-worker-seam.md) with a third
destination on the delivery-worker seam, and touches
[ADR-0032](./0032-daemon-mode-supervised-launcher.md) §10 (the daemon's
crate dependencies) and [ADR-0036](./0036-workbench-daemon-integration-protocol.md)
§2 (a new `Observe` verb). It changes no observable contract of ADR-0007
(the Telegram card) or ADR-0019 (the CloudEvents wire shape)._

The workbench's Runs panel is empty in daemon mode. Its picker, issue trail and
plan viewer already ship; what is missing is any way for the browser to learn
that a run exists. The only thing the operator sees today is merged stdout from
a run *that browser* happened to spawn — a run started from a terminal is
invisible, and a page reload loses even that.

The run process already holds exactly the model the panel wants: the pure
`RunState` fold (`runstate/state.rs`, ADR-0007 D6) that today feeds the Telegram
card and the console presenter. Nothing carries it across the process boundary
to the daemon. That boundary — CLI → daemon — is covered by no ADR today, and
per CLAUDE.md a seam with no deciding ADR is probably in the wrong place. Hence
this decision, before any code.

## Decision

### 1. The channel is a snapshot on disk, one document per `runid`

A **run snapshot** is a versioned JSON document holding the projection of a
run's `RunState` at a moment in time. The run writes it into its own repo, under
`.ralphy/`, one file per `runid`, rewritten atomically whenever the projection
changes. The daemon discovers runs by reading that directory. Nothing is sent
anywhere; nothing listens.

That it is a **snapshot and not an event stream** is the load-bearing part:

- It is **state, not a log** — idempotent, applied by replacement. No ordering,
  no sequence numbers, no replay protocol, no catch-up contract when a browser
  reattaches or a reader starts late.
- It **survives a daemon restart**, because it is on disk in the repo and the
  daemon owns none of it.
- It **sees runs the daemon did not spawn**, because the run writes it
  unconditionally — a run typed by hand in a terminal appears in the panel like
  any other.
- It needs **no inbound port, no token, no configuration**, and behaves
  identically on Windows and Linux.

The cost is honest and accepted: the operator sees the *last written* state, not
every transition. Anything that must be a durable log of what happened is the
**event sink**'s job (ADR-0019/ADR-0039), not this channel's.

### 2. The publisher is a third destination on the ADR-0024 seam

It is not a new mechanism. ADR-0024 made the ring / Layer / worker / bounded
shutdown one spine, parameterized by a `DeliveryEngine` fold, precisely so that
"adding a third sink is implementing one trait, not copying a worker". The
snapshot publisher is that third engine:

| Hook | Snapshot engine |
| --- | --- |
| `on_start` | write the initial document (the run appears in the panel before its first event) |
| `on_event` | fold into its own `RunState`, exactly as `TelegramEngine` does |
| `on_tick(changed)` | project; write **only if the projected document differs** from the one last written |
| `on_finish` | write the terminal document (runs on every worker exit path) |

Consequences that fall out of riding this seam, rather than being invented here:

- The write is **off the run path**. The Layer only enqueues; the worker thread
  writes. A slow or full disk delays a panel update, never the run.
- Writes are **coalesced by the 250 ms poll**, so a burst of events costs one
  write, and an idle run costs none.
- The engine gets its own ring and its own `DeliveryLayer` `self_target` marker
  (the loop guard of ADR-0024 D3), plus its own `detach_warn` hook emitting under
  its own module target.
- Failure is **best-effort and warned once per run**, in the shape of the sink's
  existing drop notice — a write error never fails the run and never spams the
  log.

### 3. Where the document lives, and how it is written

Path: `<repo>/.ralphy/runstate/<runid>.json`.

- Under `.ralphy/`, which the repo already gitignores (`gitignore.rs`), so a
  snapshot can never be committed.
- **Not** under `.ralphy/runs/`: that namespace already holds the per-run log
  directories keyed by a timestamp *stamp*, not by `runid`. Mixing a flat file
  keyed one way into a tree of directories keyed another way makes the reader's
  listing ambiguous and invites the two keys to be confused.
- The filename is the `runid` — the ULID that already correlates the run's
  events (`events/emitter.rs`), so the panel, the event stream and the snapshot
  all name a run the same way.

Each write is a **temp file in the same directory plus `fs::rename`**, following
`pricing/fetch.rs::atomic_write_cache` verbatim in shape and for the same
reason: `rename` replaces the destination on both Unix and Windows (there
`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`), so a concurrent reader sees
either the old document or the new one, never a truncated one and never a gap.
The temp name carries the writing pid so two concurrent runs in one repo cannot
collide on it.

### 4. The `runid` must be minted unconditionally

Today the process `runid` is minted **inside** the events-sink branch
(`run.rs:599`) — a run with no `events.url` configured has no `runid` at all.
The snapshot is keyed by `runid` and is written unconditionally, so the mint
moves to run boot, and the sink consumes the process `runid` instead of minting
its own. This is a prerequisite, not a side effect, and it is the one change
this decision forces on existing code outside the new destination.

### 5. What the document carries — and what it deliberately does not

The document is the *panel's* view of a run, small enough to rewrite four times
a second:

- **Header** — `v` (see §6), `runid`, run title, repo slug, branch, planner and
  executor agent, `started_at`, and the publishing process's `pid` (§7).
- **Queue** — `total`, `order`, `stop_before`.
- **Issues** — the `IssueEntry` list in queue order: number, title, status
  (using the existing `status_wire` names so the snapshot and the
  `run.finished.issues` rollup cannot drift), skip `kind`, `blocked_by`, model,
  effort, budget.
- **Phase** — `active` issue, `sleep` (`reset` + `target_epoch`, so the browser
  computes its own countdown), `final_summary`.

It carries the **path** of the run's plan, not the plan's text. The plan is
already a file in the repo (`.ralphy/plan.md`) and the daemon already serves
repo files through the ADR-0036 `file.read` Observe verb under path
confinement; copying a growing markdown document into a file rewritten on every
fold change would be the only unbounded thing in it. The events sink already
passes `ws.plan_path()` for the same reason.

The projection from `RunState` to the document is a **pure function**, in the
prior art of the runstate fold and the CloudEvents envelope mapper — unit
tested without a process, a disk or a network.

### 6. Versioning: `v`, additive within a version, refused above it

The document carries an integer `v` as its first field.

- Within a version, change is **additive only**: new optional fields. Readers
  parse permissively and ignore fields they do not know, so a newer run and an
  older daemon agree without anyone being upgraded.
- `v` is bumped **only** for a change that an older reader would misread — a
  field removed, retyped, or given a new meaning.
- A reader that finds `v` **greater than it knows refuses the document and says
  so**. It does not guess, and it does not silently omit the run: the panel must
  distinguish "no runs" from "this run was published by a newer ralphy". That is
  the same requirement as distinguishing "no runs" from "could not read runs" —
  an unreadable run is reported, never dropped.
- A malformed or unparseable document is treated the same way: reported, not
  hidden. The run-lock reader's `LockState::Corrupt` is the precedent — a
  document we cannot read is a fact, not an absence.

### 7. Liveness is not in the document; it is classified from the pid

The document never says whether its run is alive. There is no `alive` flag, no
heartbeat timestamp, no "last seen at". Such a field is a lie the instant the
process dies — the crash that most needs detecting is exactly the one that
leaves the flag reading `true` forever — and it would force the writer to keep
writing while nothing changes.

Instead, liveness is **derived by the reader** from a pid, using the run lock's
existing classifier: `runlock::pid_is_alive`, whose liveness predicate is
already injectable so tests never need a second process (and which is already
conservative on Windows — a process it can see but not open counts as alive).
A snapshot whose pid is dead is an **orphan**: the run crashed or was killed.
An orphan is reported as dead and swept — never rendered as a live run.

**The pid travels in the snapshot header, not in the run lock.** The obvious
carrier is the repo-scoped run lock (`.ralphy/run.lock`), which already records
its holder's pid and start time — and it is the wrong one, because the lock has
one slot and this channel has N documents. The lock is a *signal, not a mutex*
by design: `acquire` overwrites it, so with two runs in flight in one repo it
names only the newer one, and the older run's snapshot would classify as dead
while that run is working. Concurrent runs in a repo are a supported case (PRD
#296, user story 9), so the carrier must be per-document.

This does not weaken §7. What §7 forbids is a *liveness assertion* — a flag or a
timestamp the document maintains about itself. A pid is an identity fact, exactly
as it is in the lock's own `LockInfo`; the document states who wrote it and stays
silent about whether that process still lives. The mechanism is unchanged and
unduplicated: one stale-pid classifier, with injectable liveness, shared by the
lock and by this reader.

### 8. Lifecycle: created at start, rewritten on change, removed at exit

- **Created** by `on_start`, before the first event is folded, so a run is
  visible from the moment its delivery worker is up.
- **Rewritten** by `on_tick` whenever the projection differs from the last
  written document. An unchanged fold writes nothing.
- **Finalized** by `on_finish`, which ADR-0024 guarantees runs on every worker
  exit path, with the terminal statuses and the final summary.
- **Removed** when the run process exits, by an RAII guard modelled on
  `RunLockGuard` — the same shape, the same repo, the same reason. A finished
  run leaves the panel because its document is gone, not because anything
  computes an expiry.
- **Orphans** — a document left by a crashed or killed run — are recognised by
  §7's dead pid and swept by the reader (which also deletes them, so a machine
  that crashes hard does not accumulate documents forever). This is the run
  lock's recovery story exactly: there is no signal handler, a killed process
  leaves the file behind, and stale-pid takeover is the recovery mechanism.

**A finished run does not linger.** Its document is deleted at exit, so it leaves
the panel at once — which is what the parent PRD asks for (user story 12: a
finished run leaves cleanly). The alternative, keeping the terminal document
until something prunes it, immediately raises "how long, and pruned by whom",
and that is **run history** — explicitly out of scope for this channel. The cost
is accepted: an operator who is not looking at the moment a run ends does not see
its terminal state in the panel (the run report, the log and the events all still
have it). If that proves to matter in use, lingering is an additive change — stop
deleting, start pruning — not a redesign.

### 9. The daemon reads; it does not spawn

Reading is an **`Observe` verb** in the ADR-0036 sense (`runs.list`): it reads
repo state in-daemon and answers on the same connection, under the same path
confinement as `tree.list` / `file.read`. It does **not** shell out to `ralphy`,
so listing runs costs nothing, cannot fail because a tracker is unreachable, and
cannot be slowed by a vendor CLI. Updates reach the browser over a subscription
that reuses the existing `WatcherManager` behind `/ws/tree` rather than
introducing a second push mechanism.

Because a snapshot is state, the browser applies it by **replacement**. Running a
snapshot channel *and* the existing client-side event fold in daemon mode would
be two mechanisms for one job; the event fold survives only in the static demo,
where seed data is honest.

### 10. The document type is shared by a small crate, not duplicated

The writer is in `ralphy-cli` (that is where `RunState` and the delivery seam
live) and the reader is in `ralphy-daemon`. ADR-0032 §10 says the daemon "never
imports the core", and `ralphy-cli` is a binary crate with no `lib.rs`, so
neither side can name the other's type today.

Decision: a small, sync, vendor-neutral crate — `ralphy-run-snapshot` — owning
the document type, the version constant, the atomic write and the directory
reader. Both `ralphy-cli` and `ralphy-daemon` depend on it. This is the shape
`ralphy-usage-scan`, `ralphy-pty` and `ralphy-proc-util` already have: leaf
crates the core-free daemon shares with the CLI. It has two real callers on day
one, so it does not offend the "no new crate for flexibility" rule; this ADR is
its deciding record.

The pid-liveness predicate moves to `ralphy-proc-util` — its natural home, a
cross-platform process fact, and the crate already carries the Windows
`Threading` feature it needs — re-exported from `ralphy-cli::runlock` so that
module's public path and tests are unchanged (`libc` is added to proc-util's
unix target; it is already in the lockfile).

Rejected for this seam:

- **Give the daemon a `ralphy-core` dependency and put the type there.** It
  pulls git, GitHub and the runner into the daemon to share one serde struct,
  and it reverses ADR-0032 §10 for no gain — nothing about this document is
  core's business.
- **Two structs, one on each side.** One wire shape with two definitions and no
  compiler between them is exactly the defect the parent PRD is repairing
  elsewhere: two well-tested units that disagreed about a string, shipped
  silently because nothing tested the contract.

### 11. Scope: `ralphy run` only

`ralphy triage` also mints a `runid` and rides the same event bus, but it is not
what the Runs panel shows and it has no queue trail. It publishes nothing here.
The seam admits it later without a redesign — it would be the same engine on the
same spine.

## Considered options — rejected

**The run POSTs its state to the daemon over HTTP.** Rejected. The run would
need the daemon's address and a token in its environment, and the daemon would
need an authenticated ingest route — a new inbound surface on a component whose
whole posture (ADR-0032) is that it dials out and opens nothing. Worse, it is
useless exactly when it is most needed: a run that starts before the daemon, or
outlives a daemon restart, loses everything it tried to send, because a push has
no memory. A snapshot on disk has all the memory it needs.

**The daemon tails the CloudEvents the run already pushes to the event sink.**
Rejected. Those events only exist when `events.url` is configured, so a purely
local workbench would show runs only for operators who had also set up a remote
events platform — and the panel would then depend on a remote service to display
what is happening on the operator's own machine. It also inverts the shapes: the
sink carries a log, the panel wants state, so the daemon would have to
reimplement the `RunState` fold that the run process is already computing.

**A `RunState` heartbeat with a liveness field.** Rejected in §7: a flag that
cannot be cleared by the failure it is meant to detect.

## Consequences

- Three of the parent PRD's frictions — the empty Runs panel, the stale board,
  the invisible running card — collapse onto this one channel; each becomes
  small once it exists. That is why this decision gates the track rather than
  trailing it.
- Every run pays a small, bounded, off-path cost: one JSON write per changed
  250 ms tick into a gitignored directory, whether or not a daemon is running.
  A run with no observer writes a document nobody reads — accepted, because
  conditioning the write on an observer is what makes terminal-started runs
  invisible in the first place.
- `runid` becomes an unconditional property of a run rather than an
  events-sink-only one. Prose that treats it as part of the events feature must
  be corrected.
- The daemon gains its first dependency on a shared *data* shape produced by the
  CLI. ADR-0032 §10's "never imports the core" stands; §10 should record that
  the leaf-crate exception now includes `ralphy-run-snapshot`.
- Run **history** stays out. This channel describes live runs only, and its
  documents are deleted. Browsing finished runs is a different feature with
  different storage questions, and nothing here forecloses it.
