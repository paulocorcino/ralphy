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
  watcher. `runs.list` joins this class: it reads the repo's run-snapshot
  documents from `.ralphy/runstate/` and classifies each by its header pid
  ([ADR-0047](./0047-run-state-snapshot-channel.md) §9) — bytes on disk, no repo
  semantics, no spawn.
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
  `file.rename`, `file.copy`, `file.delete`); like Observe, they answer on the
  requesting `Command` id and never spawn.
- The confinement module is the shared kernel of Observe **and** Write; its
  test suite covers write-escape attempts (traversal, symlink, rename-across
  the boundary) as exhaustively as reads.
- Deletion stays a plain confined unlink/rmdir; any confirmation UX is the
  browser's job, not a daemon semantic.

## Amendment (2026-07-25, issue #300): the run-snapshot subscription

The `/ws/tree` socket carries a **second subscription kind**, so the live Runs
panel (ADR-0047 §9) reuses the watcher manager of §4 rather than introducing a
second push mechanism. A client sends `runs.watch { repo }` (the payload path is
ignored — the target dir is fixed) and `runs.unwatch { repo }`; a settled change
under that repo's snapshot dir is pushed as `runs.dirty { repo }`, and the
browser re-reads `runs.list`. Push discrimination is by the watched **rel dir**,
not by the verb that subscribed, so both kinds share ONE per-connection watched
list and one exactly-once teardown.

`.ralphy/runstate` is the ONE directory exempt from §4's "never descend
`.ralphy`" filter. Every repo ralphy touches gitignores `.ralphy/`, so without
the exemption the pump would drop every snapshot event and §9 of ADR-0047 could
not be built on this watcher at all. The exemption is a single constant
(`watch::RUNSTATE_REL`) matched on the event's parent dir, and is still gated on
the watch-set: nothing is watched that a client did not ask for.

Establishing that watch **creates** the directory when it is absent. This is a
named, bounded exception to §3's observe-never-mutate: `notify` errors on a
missing path, and a repo where `ralphy run` has never run has no snapshot dir —
so a first run started while the panel is open would stay invisible until the
operator reopened the project. What is created is an empty, gitignored directory
that ralphy itself owns; no repo content is written, read, or interpreted.

The consequence for the browser: a snapshot is **state**, not a log, so the
client applies it by replacement and the extra read on every (re)connect is free.
The client-side run-event fold and the demo advance control therefore have no
role in daemon mode and are reachable only from the static `file://` demo.

## Amendment (2026-07-25, issue #310): the run-completion nudge

The `/ws/tree` socket carries a **third push kind**, `changes.dirty { repo }`,
which is NOT fed by the §4 watcher. Its source is a daemon-wide broadcast the
Spawn path (§1) sends when a dispatched child exits: the daemon already sees
that exit, and a run that just touched the working tree is exactly when the
Changes count (issue #307) goes stale.

This push kind has **no subscription verb and no per-connection state**. Every
`/ws/tree` connection gets every nudge, and the browser filters by the repo
it currently has open — a nudge for another repo is a no-op it drops. The
alternative (a `changes.watch` / `changes.unwatch` pair) would add a teardown
obligation for a push that costs nothing to ignore.

Two boundaries hold. The §4 **watch-set is not widened**: it stays bounded by
what the browser expanded, so no repo-wide filesystem watch is established.
And **no timer is introduced**: the nudge is edge-triggered by the child exit,
not polled. The Changes count therefore stays a snapshot between events — a
tree change with no run and no manual refresh stays invisible by design.

The send lives inside the blocking wait task, not the socket `select!` loop, so
it fires on every exit path the daemon outlives — including the client-disconnect
and shutdown arms that abandon the socket while the run keeps living. A run that
outlives the daemon process itself is the one case with no nudge to send; the
browser's reconnect catch-up read covers it, with no operator action.

The nudge is scoped to the Spawn class. The Write verbs and the branch mutations
answer in-daemon and deliberately send NO nudge: their caller is the browser
itself, which already knows what it just wrote. The count they leave behind is
the same snapshot the manual refresh exists for.

## Amendment (2026-07-26, ADR-0049): Observe is not text-only

§2's Observe class reads "the working tree as OS bytes", and `file.read` — the
only reader built here — narrowed that to *text*. A second Observe reader,
`file.image`, serves an allowlisted image type as base64 for a `data:` URL, so
the workbench can display a `.png` instead of refusing it. The decision, the
rejected alternatives (a raw HTTP byte route; a fourth binary frame tag), the
media-type allowlist and the magic-byte check live in
[ADR-0049](./0049-workbench-serves-image-bytes.md). §5 is not reopened:
confinement is the same kernel and the escape→`not found` masking still holds.

## Amendment (2026-07-26, issue #327): the desk is a daemon-served resource

The **desk** — which consoles were open and where each window sat — moves out of
the browser's `localStorage` and into the daemon's global store, as
[ADR-0050](./0050-desk-layout-is-daemon-state.md) decides. A workbench session
already survives the browser; its window now does too, so opening the workbench
from a second machine restores the stage instead of cascading it.

Two REST endpoints join `/api/*`, behind the same auth guard as the rest of the
API (no new policy, no new exemption):

- `GET /api/desk` → the desk as `{ windows, fences }`, each in layout order. A
  missing or corrupt `desk.toml` answers `200 {"windows":[],"fences":[]}`: an
  unreadable layout costs a cascaded stage, not a daemon, so there is no error
  state for the shell to handle.
- `PUT /api/desk` → replaces the desk wholesale and answers `200` with the
  stored object. The daemon prunes each record type to its own cap (24 windows,
  12 fences, newest by `ts`), so the cap is enforced server-side rather than
  trusting the upload; the response is the post-prune truth in one round trip
  (last write wins — no ETag, no merge). A body that is not a
  `{ windows, fences }` object — including the pre-#340 bare array — is rejected
  before the store is touched.

The desk is a *typed* store (`desk.toml`, one `[[windows]]` table per record),
not an opaque blob, and lives beside `repos.toml` — same `$RALPHY_DAEMON_DIR`
rooting as the repo registry. Uploads are debounced and fire-and-forget in the
shell: a failed write costs a stale position, never the drag that triggered it.

## Amendment (2026-07-26): the tree shows gitignored files

§4's **`ignore`-crate filtering** bullet is narrowed to noise only: `tree.list`
and the watcher pump **no longer consult `.gitignore`/`.git/info/exclude`**. The
surviving filter is the fixed `HARD_EXCLUDE` list — `node_modules`, `target`,
`.git` — which is a *name* list, not a git decision, and stays because a repo
whose tree opens onto 40k transitive packages or git's own object store is
unusable for the reason the filter was invented.

The reason is the operator's day: the files that matter most while a run is
executing are precisely the ignored ones — `.ralphy/plan.md`, `.ralphy/runs/*`,
run logs, build output, a local `.env`. A mini-IDE that cannot open them is not
a mini-IDE. §5 already settled this stance for *reads* ("an authenticated
operator reads the whole repo, secrets included… a mini-IDE that hides half the
files is not the product") and already refused gitignore as a secret filter
("inconsistent by construction"). The listing filter was the same half-hidden
product arriving through the UX door: the file was readable if you knew its
name, and invisible if you did not. This amendment removes the inconsistency in
the direction §5 already chose.

**§5 is not reopened.** Confinement and the login remain the only two controls,
both untouched. Nothing becomes readable that was not readable before — only
*nameable without knowing the name*.

Three consequences, all accepted:

- **The watcher nudges on ignored paths.** The gitignore check in the pump goes
  with it, so an expanded build directory now pushes `tree.dirty` on every
  settled batch. §4's other three levers still hold the cost: the watch-set is
  still the expanded set (an operator who does not open `target/` pays nothing),
  the debouncer still coalesces a storm into one nudge, and the nudge still
  carries no payload.
- **`.ralphy/` becomes an ordinary directory in the tree**, which is what §4's
  "`.ralphy` is deliberately NOT in `HARD_EXCLUDE`" (issue #203) always intended;
  the gitignore filter had been quietly defeating it, since every repo ralphy
  touches ignores `.ralphy/`.
- **The `RUNSTATE_REL` exemption stops being an exemption.** The constant stays
  (the runs subscription still names that directory), but it no longer skips a
  filter that no longer exists.

**Rejected: a per-operator "show ignored files" toggle.** It buys a preference
where there is one operator and one answer, and it doubles the tree's truth —
the same path present or absent depending on hidden state, which is exactly the
confusion the removed filter caused.

## Amendment (2026-07-28, docs/adr/0054): `run.stop`, the one lock-blind Mutate

The registry (§1–2) gains one row: **`run.stop`** — a **Mutate**, argv
`stop --runid=<id>`, one closed client parameter (`runid`, validated as bare
ASCII alphanumerics so it can never become a flag, a path, or a second token).

It is a Mutate rather than a Spawn deliberately: it runs one short `ralphy stop`
and collects it, exactly as `sync push` does, and the process it starts writes a
file rather than driving a run.

**It is the one Mutate exempt from §6's run-lock awareness.** Every other write
verb refuses under a held `run.lock` because a run owns the tree while it works.
This verb exists precisely to act *while* a run holds the lock — guarding it
would make it refuse in the only situation it is for. `runlock.rs`'s refusal
message already said "wait for it to finish or **stop it**"; that sentence is now
literally actionable.

Everything else in §2 is unchanged, including the Spawn bullet's "keeps its own
lifecycle": a stopped run still ends itself. See
[ADR-0054](0054-cooperative-run-stop.md) for the mechanism and
[ADR-0032](0032-daemon-mode-supervised-launcher.md)'s amendment for why §6's
no-remote-kill exclusion survives.

## Amendment (2026-07-29): `plan.discard`, a Write whose target the verb fixes

The registry (§1–2) gains one row: **`plan.discard`** — a **Write**, in-daemon,
never a spawn, with **no client parameter at all** beyond the repo. It deletes
`.ralphy/plan.md` and nothing else.

### Why a named verb instead of `file.delete`

The Write amendment's denylist (`PROTECTED_DIRS` = `.git`, `.ralphy`) refuses
every generic byte-op that names `.ralphy`, and that refusal is correct: the
directory is daemon-and-run state the daemon itself reads back as trusted config.
So the capability could arrive one of two ways — a hole in the denylist, or a verb
whose target is not the client's to choose. This takes the second.

It is the same shape §1 already uses for `runs.list` ("no `path` input: the verb
alone fixes what is read") and for `board_argv`: **the table, never the client,
chooses what is touched.** A path in the payload is ignored rather than honoured,
and the integration test sends one to prove it. Nothing about `.ralphy` becomes
writable; one named artifact becomes discardable.

### Why the capability exists at all

A finalized plan is *picked up by the next run*: the planner writes
`<!-- ralphy-plan: issue=N -->` as the plan's last line, and
`ralphy_adapter_support::resume::plan_is_finalized_for` treats that as "do not
re-plan". That is the right behaviour for resuming an interrupted run, and it also
means an operator who changes their mind about a planned issue — most sharply when
the plan itself reports `## Feasible: no` — has a ready plan that will be executed
next. Until now the only way out was deleting the file by hand, outside the
workbench and outside every guard in it.

### What it does NOT gain

- **Not recursive, and not a follow.** Only a regular file is removed;
  `symlink_metadata` refuses a symlinked `plan.md` (which would unlink a file
  outside the repo) and a directory of that name. A path-less delete is exactly
  where a recursive one must never appear.
- **Not a plan writer.** There is no `plan.write`; the panel still *reads* the
  plan through `file.read`, like any other file.
- **Not run-lock-aware**, per the Write amendment's "Write does not consult the
  run lock". The browser gates the affordance while a run is live (a run owns the
  plan it is executing) and confirms through the design-system dialog first —
  which is where "any confirmation UX is the browser's job" already put it.
- **Absent is `NotFound`**, never a silent success: "there was no plan" and "the
  plan is gone" are different answers, and the panel says which.

## Amendment (2026-07-30): `project.remove`, a Mutate that spawns the existing subcommand

The registry (§1–2) gains one row: **`project.remove`** — a **Mutate**,
spawn-and-collect `ralphy daemon remove <slug>`, so the sidebar can drop a
project from the daemon's registry. No HTTP route is added: it rides the same
`/ws/command` envelope every other verb rides, and the argv is composed by
`dispatch::project_remove_argv` from a single `slug` param pinned to what
`ralphy_core::git::project_slug` can actually produce: `<owner>/<name>` for a
forge remote, or a SINGLE `path-<hash>` segment for a remoteless repo. Every
segment non-empty, every byte in `[A-Za-z0-9._-]`, no leading `-`, at most one
separator. An out-of-shape slug yields no argv and no spawn. Pinning the
two-segment form alone would have made every local-only project unremovable.

### Why a Mutate and not a Write

The Write amendment's division rule would have permitted a direct edit: the
registry is a daemon-owned file under the global store, not a repo path, so no
denylist stands in the way. It is still the wrong side of the line. `repos.toml`
has exactly **one owner today — the `daemon add` / `daemon remove` subcommand
pair** — which owns not just the bytes but the load→mutate→save sequence and
whatever invariants it grows. A second writer inside the daemon would make that
sequence a convention instead of a fact, and the two writers would drift the
first time either side gains a field. One owner beats a shorter code path.

That is also why the verb spawns rather than reimplements: `daemon remove` is
already idempotent (removing an unregistered slug prints "was not registered"
and exits 0), so the "already absent is harmless" acceptance falls out of the
existing subcommand rather than being re-established at the wire.

### Routing, and what is NOT deleted

The WS envelope routes on a registered `repo` before dispatch runs, so the
browser sends **both**: `repo` names the cwd the spawn runs in, `slug` names
what is unregistered. Those are normally the same project — which is fine, the
routing lookup precedes the removal — and the split is what keeps the existing
peer-proxy path working unchanged for a peer environment's project.

**The directory on disk is never touched.** The verb unregisters; it does not
delete, move, or clean anything under the project. The browser states that
literally in its confirmation, which is where "any confirmation UX is the
browser's job" already put it.
