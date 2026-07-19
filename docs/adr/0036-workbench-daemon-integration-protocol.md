# Workbench↔daemon integration: a verb registry over one protocol

Status: proposed (design interview 2026-07-13; not yet implemented).

The mock workbench shell (`mocks/workbench-shell/`) needs a real backend: the
browser must drive the resident **daemon** (ADR-0032), and the daemon must reach
each repo's **run**, **config**, and forge state — always *through* `ralphy` in
the repo's context, never by touching the repo itself. The mock already fixes the
UI half of the seam (`workbench:action` out, `ralphy:run-event` in; see
`docs/WORKBENCH-BUILD-GUIDE.md`) and the daemon already fixes the wire half
(the tagged-frame codec in `protocol.rs`, the closed dispatch vocabulary in
`dispatch.rs`). This ADR fixes the *contract between them* before the
implementation slices: one integration method, so adding a capability is a table
row, not a new endpoint.

Vocabulary (**Daemon**, **Workbench session**, **Forge query**, **Repo
registry**, run-lock) is defined in [CONTEXT.md](../../CONTEXT.md) and
[ADR-0032](./0032-daemon-mode-supervised-launcher.md); this document records the
decisions that extend ADR-0032 §6 without reopening it.

## Decision

### 1. One envelope, one registry: capabilities are rows, not routes

The daemon already carries a transport-agnostic request/response envelope —
`Command { id, verb, payload }` over channel tag `0x02` (`protocol.rs`). This ADR
promotes it to *the* RPC surface: every browser gesture that is not raw terminal
I/O or a presence heartbeat is a `Command`, correlated by `id`.

The closed `dispatch::Verb` enum (today: `run`/`triage`/`push`) generalizes into a
**verb registry** — a table mapping each verb to an **effect class** (§2) and,
for the classes that reach the repo, a *fixed* `ralphy` invocation template. The
table, not the client, chooses the argv; the argv is derived only from the verb
(the `&'static`-argv guarantee of `dispatch.rs`, widened but not weakened). Adding
a capability is adding one row here plus (for a new Mutate) one `ralphy`
subcommand — never a new axum route, a new WebSocket, or client-composed command
lines. This is the whole point: it collapses "N ad-hoc routes" into one dispatch
table, so the backend grows by data, not by surface.

**Rejected: a second WebSocket per capability** (one for the tree, one for runs,
…). The tagged-frame codec already multiplexes channels over one connection; a
socket per feature multiplies teardown logic and connection state for no gain.

### 2. Effect classes: the class, not the daemon, chooses the mechanism

Every verb is one of four effect classes, and the class alone decides whether the
daemon acts directly or delegates to `ralphy`:

- **Native** — the daemon's own state: list/close/reattach **sessions**,
  identity, presence, registry. No repo involved.
- **Observe** — read-only projection of the **working tree as OS bytes**: list a
  directory, read a file's contents, watch for change. Carries **no repo
  semantics** (no git, no issues, no config resolution), so the daemon does it
  **directly** — the same species as the `reachable` `stat` in `/api/repos` and
  the PTY multiplexing it already owns. See §3 for the boundary and §4 for the
  watcher.
- **Query** — read-only requests whose answer requires `ralphy`'s **judgment**:
  the judged queue, an issue's thread, resolved config. Backed by a fixed
  `ralphy … --json` spawn; the daemon collects stdout and answers on the same
  `id`. This is ADR-0032 §6's **forge query** family, generalized. `ralphy
  issues --format json` (and `issues show <n> --format json`) already back the
  first verbs; issue-body-plus-comments is a declared gap (see Consequences).
- **Spawn** — the run-triggering verbs `run`/`triage`/`push`: a detached, blessed
  `ralphy` child that keeps its own lifecycle (the teardown invariant of
  `dispatch.rs`). Already built; unchanged.
- **Mutate** — a write to repo state: `config.set`/`config.unset`,
  `branch.switch`/`branch.create`, `label.set`. Each is a **new `ralphy`
  subcommand** (never the daemon shelling out to `git`/`gh`), and each is
  **run-lock-aware** (§5).

The division rule is one sentence: **if a verb needs to *understand* or *write*
the repo, it is a `ralphy` invocation; if it only reads OS bytes or the daemon's
own state, the daemon does it directly.**

**Rejected: routing tree navigation through a per-read `ralphy` spawn.** Tree
expansion is the IDE's most frequent gesture; paying a process cold-start per
click (git resolution, etc.) would make navigation sluggish. Observe reads carry
no semantics, so the spawn buys nothing.

### 3. Boundary refinement: the daemon may *observe* the working tree, never *interpret or mutate* the repo

ADR-0032 keeps the daemon a launcher that reaches runs only by spawning `ralphy`,
never importing the core. That rule is about **repo semantics** (git, issues,
labels, config, the run loop), not about reading bytes from disk — the daemon
already `stat`s paths for reachability. This ADR states the line explicitly, so
the file-tree feature does not look like a violation:

> The daemon **may observe** the working tree as OS bytes — list directories,
> read file contents, watch for change events. It **may not interpret or mutate**
> the repo — git, issues, labels, config, plan.md meaning — which stays a
> `ralphy` invocation.

"Observe" sits in the same class as the PTY plumbing: OS mechanics, vendor- and
semantics-neutral. "Interpret/mutate" stays `ralphy`'s.

**Rejected: a resident `ralphy watch` child per open repo** streaming tree deltas
to the daemon. It would honor a purity the real boundary does not require, at the
cost of one more resident process and lifecycle per repo — pure overhead.

### 4. The file-tree watcher: event-driven, lazy, cached, coalesced

Live tree updates use `notify` (per-OS backend: inotify / FSEvents /
ReadDirectoryChangesW), so idle cost is ~zero — but `notify`'s cost is in the
storm and the watch-descriptor count, and the naive recursive watch is the trap.
Four levers keep it cheap, and all four are load-bearing:

- **Lazy watch, matched to the UI's lazy-load.** The tree is a single-open
  accordion with lazy-loaded subfolders; watch a directory **only while it is
  expanded**, unwatch on collapse. The watch-set equals the visible-set — bounded
  by the screen, not by repo size. On Linux (one inotify descriptor per
  directory) this is the difference between tens of watches and tens of
  thousands. WSL runs its own native Linux daemon (ADR-0032 §3), so this is
  native inotify, never `\\wsl$` polling.
- **`ignore`-crate filtering.** Walk and watch gitignore-aware — never descend
  `node_modules`/`target`/`.git`/`.ralphy`. Reuses the semantics the core already
  honors (`ralphy-core`'s gitignore handling).
- **Debounce/coalesce** with `notify-debouncer-full`: a `git checkout`/build fires
  thousands of events; the debouncer delivers one settled batch after a quiet
  window and correlates renames. Without it, a checkout floods the socket.
- **Cache + deltas, push-nudge / pull-detail.** The daemon holds an in-memory
  tree snapshot per (repo × open dirs). On a settled batch it emits a **minimal**
  `tree.dirty { repo, path }` frame — **no tree payload** — and the browser pulls
  the subtree via `tree.list` (Observe) **only if that dir is visible**; an
  invisible nudge is dropped (refetch lazily on expand). The heavy data travels
  only for what is on screen; the event is a ping.

Lifecycle mirrors sessions (ADR-0032 §2): one watcher per (daemon, repo, dir-set),
**fanned out** to all attached browsers — never one watcher per connection. It
falls when the last client closes the project. A hard cap bounds total
descriptors; a monstrous expand degrades to `PollWatcher` rather than exhausting
the OS. The `notify` thread bridges to tokio by the **same reader-thread→channel
pattern the daemon already uses for the PTY** — no new concurrency model, the
async stack stays confined to `ralphy-daemon` (ADR-0032 §10).

### 5. Repo confinement and the security stance: an IDE, gated at the door

Observe reads (§2) let the daemon serve file contents to a browser, possibly over
a network bind. Two controls, and only two, are the security boundary — both hard:

- **Confinement.** Every resolved path is `canonicalize`d and asserted to be a
  prefix of the canonicalized repo root; anything outside is refused. This blocks
  path traversal (`../../.ssh/id_rsa`) **and** symlink escape (canonicalize
  resolves the link, the prefix check catches the escape) in one step. This is
  non-negotiable — without it the mini-IDE is a whole-disk reader.
- **The existing login.** Who may read is gated by ADR-0032 §4's auth
  (TOTP/token on a network bind; nothing on localhost). The control is *who
  enters*, not *what they read*.

Given those two, an authenticated operator reads the **whole repo, secrets
included** — exactly like VS Code opens your `.env`. This is a deliberate,
accepted trade-off: a mini-IDE that hides half the files is not the product, and
this is a **single-operator** tool.

**Rejected: a secret-content blacklist** (hide `.env`, `*.pem`, …). It fails open
(the one key you forget leaks) and it is the inverted mental model: `.env` is
gitignored *because* it is secret, so gitignore-based hiding is inconsistent by
construction. `.gitignore` filtering stays in §4 as **UX cleanliness only**
(hiding `node_modules`/`.git`/`.ralphy` noise) — **never** labelled a security
boundary. A file hidden from the listing is still readable if named; that is not
protection.

### 6. Mutate is run-lock-aware, and that awareness lives in `ralphy`

A Mutate that touches git — `branch.switch` while a run is committing on that
branch — corrupts the working tree. Two facts force the design: git is repo
semantics (so it is a `ralphy` verb, §2), and only `ralphy` owns the
repo-scoped `.ralphy/run.lock` (`runlock.rs`), which the daemon neither knows nor
should. Therefore:

> Every Mutate verb that touches git/repo state is a **new `ralphy` subcommand**
> that **inspects the run lock and refuses under `HeldAlive`**. The daemon spawns
> the verb and relays the refusal to the UI; it never runs `git`/`gh` itself.

The mock's current note that "the daemon runs the real `git checkout`" is
superseded by this: the daemon runs *`ralphy branch switch`*, which is
run-aware.

**Rejected: the daemon running `git`/`gh` directly** for speed. It is blind to
the run lock — the exact corruption path — and it crosses the §3 boundary. The
new subcommands are more work; the corruption is not negotiable.

### 7. Authorization stays binary; the justification is corrected

The Mutate verbs (`config.set`, `branch.switch`, `label.set`) are powers a
scheduled timer never fires, so ADR-0032 §6's original justification for binary
auth — "the daemon gains no power a cron timer lacks" — no longer holds. The
**conclusion survives, the reasoning is replaced**:

> Binary authorization holds because **Mutate ⊆ the powers a workbench session
> already grants**: an interactive agent session is a remote shell in practice
> (ADR-0032 §2), so switching a branch or writing config is *strictly less*
> than what "may open a session" already concedes. No verb widens the blast
> radius beyond the session.

This is a **single-operator** tool (no guests), so scoped/read-only roles are
overengineering and are not built. This ADR records the corrected justification;
it does **not** edit ADR-0032's text.

### 8. Three state planes, one connection; Phase 1 run feed is raw output

State is not one thing — it is three planes, kept separate exactly as the codec's
tags keep them separate:

- **Presence** (`0x03`) — daemon liveness; dies with the daemon.
- **Session** (`0x01` Terminal) — the PTY, tmux-model: survives a dropped
  WebSocket, the browser reattaches (ADR-0032 §2).
- **Run** — owned by the run, not the daemon (a Spawn child outlives the daemon).

For Phase 1 the Runs panel is fed by the **raw merged output** a daemon-spawned
run already streams over `/ws/command` (`status:"output"`, ADR-0032 §5 / issue
#180). The mock's **structured** run feed (issue trail, plan viewer, phase glyphs
from `ralphy:run-event`) is **deferred to the events platform** (ADR-0019, Phase
2); no daemon-side CloudEvents relay is built now, so ADR-0032 §5 is not stretched.

## Consequences

- **Adding a capability is a table row.** The verb registry + effect classes turn
  backend growth into data: one row (plus a `ralphy` subcommand for a new Mutate).
  The mock's `workbench:action` map and a ~40-line `wb-daemon.js` client are the
  whole browser-side integration; both are documented in
  `docs/WORKBENCH-BUILD-GUIDE.md`.
- **New `ralphy` subcommands** are required by §6: `branch switch`, `branch
  create`, `label set` — each run-lock-aware. This is the one place the low-impact
  package spends new CLI surface.
- **The Query family depends on `--json` surfaces.** `issues`/`cost` already emit
  JSON; the declared gap is an issue's **body + comments** (the Kanban's need,
  already flagged in BUILD-GUIDE) — a future read-only `ralphy` surface, not a
  daemon concern.
- **The new code is bounded**: generalize `dispatch::Verb` into the registry, the
  file-tree watcher (`notify` + `notify-debouncer-full` + `ignore`, new daemon
  deps), `wb-daemon.js`, and the three git Mutate subcommands. Everything else
  reuses the existing daemon routes, PTY plumbing, auth, and CLI.
- **ADR-0032 is extended, not reopened.** §6's command vocabulary grows the effect
  classes and the Observe/Query/Mutate families (additive); the boundary note
  (§3) and the corrected auth justification (§7) live here, referencing 0032.
- **Explicitly deferred (not decided here):** the structured run feed via the
  events platform (Phase 2), and moving schedule orchestration into the daemon
  (a future ADR revisiting 0032 §1's "launcher, not scheduler" — its robustness
  and missed-run/catch-up trade-offs are real and unowned). Phase 1 keeps
  `ralphy schedule`'s OS timers and the daemon-as-trigger, unchanged.

## Amendment (2026-07-13): the Write effect class — workspace byte-writes

The mock emits four gestures no §2 class covers: `save`, `create`, `rename`,
`delete` — writes of **working-tree OS bytes**. Observe is read-only by
definition; Mutate is *repo semantics* routed through a `ralphy` subcommand
(config, branch, label). An editor save is neither: it carries no repo meaning
the daemon would have to understand — it is the same species of operation as an
Observe read, pointed the other way. This amendment is **additive**; no frozen
section is reopened.

### The class

**Write** — a write of working-tree bytes: save a file's contents, create a
file/folder, rename, delete. The daemon performs it **directly**, under the
**same confinement as Observe** (§5: canonicalize + repo-root prefix, on *every*
path involved — a rename checks both source and destination). The §2 division
rule is extended by one word:

> if a verb needs to *understand* the repo, it is a `ralphy` invocation; if it
> only **reads or writes OS bytes** or the daemon's own state, the daemon does
> it directly.

§3's boundary sentence is refined accordingly: "interpret or mutate the repo"
means *repo semantics* — git state, issues, labels, config, plan.md meaning.
Writing a file's bytes inside the confined root is not that; it is what every
IDE does to an open working tree.

### Write does not consult the run lock

A byte-write proceeds regardless of `.ralphy/run.lock`. The lock guards
**repo-semantic transitions** that corrupt a run's assumptions wholesale — a
branch switch under a run's feet (§6) — not ordinary edits. An operator saving
a file from the workbench during a run is exactly an operator saving from VS
Code during a run: visible to the run, owned by the operator, and this is a
**single-operator** tool. Gating every save on the lock would make the editor
unusable for the duration of every run, for no corruption it actually prevents.

**Rejected: routing byte-writes through new `ralphy` file-op subcommands.** It
would spend a process cold-start per save, force `ralphy` to grow verbs with no
repo semantics (pure plumbing, against §2's whole point), and buy nothing: the
run-lock question is answered above, and confinement is enforced at the daemon
either way.

### Consequences of the amendment

- The verb registry gains the Write rows (`file.write`, `file.create`,
  `file.rename`, `file.delete`); like Observe, they answer on the requesting
  `Command` id and never spawn.
- The confinement module is the shared kernel of Observe **and** Write; its
  test suite covers write-escape attempts (traversal, symlink, rename-across
  the boundary) as exhaustively as reads.
- Deletion stays a plain confined unlink/rmdir; any confirmation UX is the
  browser's job, not a daemon semantic.
