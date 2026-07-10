# Ralphy

A global, in-place runner that works a repo's GitHub issue queue unattended on a
Claude subscription, committing each issue and handing back a branch to merge by
hand. Its triage vocabulary follows Matt Pocock's canonical roles — the full set
of five and Ralphy's stance on each is in [docs/triage-roles.md](./docs/triage-roles.md).

## Language

**Run**:
One invocation of the runner over a repo's queue, identified by a timestamp.
_Avoid_: session (that's one issue's Claude execution within a run).

**Queue label**:
The label that puts an open issue into the run, worked in ascending issue-number
order. Canonical `ready-for-agent`; `AFK` is an accepted synonym.
_Avoid_: todo, backlog.

**Human label**:
The canonical `ready-for-human` (synonym `HITL`). Marks an issue as human-only —
it is **never** queried, so the agent never works it. Carries no other runtime
behaviour in Ralphy.
_Avoid_: blocked, manual.

**Green**:
An issue whose execution finished cleanly (the agent emitted `RALPHY_DONE_EXIT`),
as opposed to a non-green stop (blocked / timeout / stuck / usage limit).

**The cycle / close-on-green**:
A green queue issue is closed by the runner so it leaves the queue; its label is
left untouched and the human still merges the branch by hand.

**Acceptance ledger**:
The per-issue mapping of each of the issue's Acceptance criteria to a verdict —
*verified* (backed by a passing test) or *review-only* (only a human can confirm)
— plus the evidence (the commit/test that proves it). The planner emits it from
the issue's criteria verbatim; the executor fills the evidence as it works; the
runner transcribes it onto the issue at close. It does **not** gate green —
green stays defined by the plan's test-verifiable "Done when". The ledger is the
honesty record that the green gate's outcome maps back to what the issue asked.
_Avoid_: acceptance check (sounds like a gate), checklist.

**Evidence (close handoff)**:
What the runner writes onto a closed green issue beyond the bare close: it ticks
the issue body's verifiable Acceptance-criteria checkboxes (matching each line
verbatim — only ticking, never rewriting) and posts a comment pairing each
criterion with its verdict and proof from the **acceptance ledger**. Review-only
criteria are left unticked and flagged for the human merging the branch.

**Run branch**:
Where commits land. `BranchMode new` cuts a fresh `afk/run-<stamp>` off the base;
`BranchMode current` commits onto the branch the repo is already on.

**Adapter**:
The isolated unit holding everything specific to one agent CLI vendor (Claude
Code, Codex, Kimi, and OpenCode), behind the core's agent contract. Each
adapter owns its own execution mode and completion protocol.
_Avoid_: driver, plugin, backend.

**Planner / Executor (phase roles)**:
The two phase-roles a run's **adapter(s)** fill: the *planner* writes
`.ralphy/plan.md` from the issue; the *executor* carries that plan out and
commits. By default one adapter fills both. They are independently selectable —
`--agent` picks the executor, `--plan-agent` the planner (defaulting to
`--agent`). The plan artifact is vendor-neutral markdown, so any planner's plan
is executable by any executor.
_Avoid_: stage, role (overloaded with triage roles).

**Split run**:
A run whose **planner** and **executor** are different **adapters** (e.g.
`--agent opencode --plan-agent claude`: Claude plans on subscription, OpenCode's
Kimi codes). Wired by a composition-root wrapper that delegates `plan()` to one
adapter and `execute()` to another — the core still sees one `Agent` and never
learns it is split. Usage-limit handling is per-phase: each phase inherits its
own adapter's limit stance (the planner may auto-resume while the executor
stops). The plan-phase ledger line carries the executor's `agent` name (the
wrapper reports one identity), while the `model` column stays per-phase-true.
_Avoid_: mixed-vendor (informal), planner override.

**Settings**:
Per-repo operator configuration at `.ralphy/settings.json` (gitignored), managed
by `ralphy config set/get/unset`. Its first key is the **OpenCode model
default** — a persistent `-m` the operator picks once. The schema tolerates
unknown keys so future knobs grow in the same file. Distinct from the Telegram
config, which stays its own global TOML for now.
_Avoid_: config (the subcommand), preferences, dotfile.

**Event sink**:
A `tracing_subscriber::Layer` consuming the run's structured event bus. Four
exist or are decided: the console presenter, `ralphy.log`, the Telegram
notifier, and the CloudEvents HTTP sink (ADR-0019) that POSTs each event as
CloudEvents 1.0 JSON to a configured `events.url` — additive, best-effort,
never blocking the run. The event catalog is [docs/events.md](./docs/events.md).
_Avoid_: exporter, webhook (the sink pushes; it exposes nothing), logger.

**Emitter identity**:
The extension attributes every CloudEvent carries so a fleet of Ralphys (many
devs, many machines, concurrent processes) stays distinguishable: `runid`
(ULID minted at process start — the correlation **key** and the envelope's
only extension attribute, since CloudEvents extensions must be simple types),
plus attribution and diagnostics grouped in the reserved `data.emitter`
object (`version`, `user`, `host`, `os`, `pid`, `ip`, `tz`). PID is
diagnostic, never a key (recycled, collides across hosts).
_Avoid_: instance id (the persistent identity is the **daemon**'s `daemon_id`,
a different species — run events stay keyed by the ephemeral `runid`), session id.

**Queue snapshot**:
The per-issue backlog view as the runner judges it — number, title, labels,
`queue_status` (eligible/skipped/blocked/stop_before), skip reason, blockers,
position. One shape, three surfaces: the `ralphy issues` listing, the
enriched `queue.built` event, and the on-demand `queue.snapshot` event from
`ralphy issues --push` (ADR-0020).
_Avoid_: backlog dump, issue list (the GitHub-side raw list, without judgment).

**OpenCode model resolution**:
The precedence Ralphy uses to pick the OpenCode execution model:
`--exec-model` (per-run) **>** `settings.json` `opencode.model` (persistent
default) **>** omitting `-m` so OpenCode resolves its own (ADR-0005 D4, amended
by ADR-0010). An empty/unset setting falls back to OpenCode's own default, which
stays the out-of-the-box behaviour. The model that *actually* ran is read back
from `opencode.db` into the ledger (ADR-0008 D5).
_Avoid_: model selection (reserved for Claude complexity routing).

**Adapter support**:
The shared machinery every **adapter** leans on but that is specific to *no*
vendor — the headless child-driving loop (spawn, drain stdout/stderr, poll to
completion-or-timeout, kill on deadline), the `RALPHY_DONE_EXIT` /
`RALPHY_BLOCKED_EXIT` sentinel parser, and skill/plugin materialization. It is
the deliberate counterpart of **Adapter**: where an adapter holds what is
vendor-specific, adapter support holds what is common. It owns **no** completion
protocol and produces **no** `Outcome` — it hands back raw captured output and
each adapter still classifies it (the seam ADR-0004 protects). Lives in
`ralphy-adapter-support`; depended on by the vendor adapter crates, never by the
core.
_Avoid_: shared runner, headless runner (ADR-0004 forbids a shared *Outcome*
runner — this is only the plumbing), utils, helpers.

**Completion signals / Outcome classifier**:
The seam between an **adapter**'s vendor-specific end-state extraction and the one
shared rule that maps it to a core `Outcome`. Each adapter reduces its raw session
end-state to **completion signals** — `done`, `blocked`, `limit` (set only when the
vendor judges it *trustworthy*), `committed`, `timed_out`, `exited_ok` (a
vendor-normalized "ended in a state where a DONE claim is trustworthy"), `errored`
— and the vendor-neutral **outcome classifier** applies one fixed precedence ladder
to produce the `Outcome`: a trustworthy `limit` outranks a `done` (resume-after-reset
beats closing a throttled session) and a `timeout`; a `done` needs only
`done && exited_ok && !errored` — **never** a fresh `committed`, because
protocol-completion and flake-repair hand-backs legitimately finish with no commit
(the plan lives in gitignored `.ralphy/plan.md`). `committed` is a *progress* signal
feeding the Claude headless no-commit **streak**, not a gate on **green**. This
*narrows* — does not reopen — ADR-0004: raw→signal extraction (including limit
trustworthiness and exit normalization) stays per-adapter; only the signal→`Outcome`
ordering is shared (ADR-0023). Claude is the reference implementation; the behavior
change lands on the Codex and OpenCode adapters.
_Avoid_: completion protocol (that is the sentinel parser in **adapter support**),
classify (the bare function name), outcome mapping.

**Execution mode** (interactive vs headless):
How an adapter drives its CLI — an adapter/billing concern, **never** the core's.
For Claude Code, interactive (over a PTY) bills against the subscription, while
headless `-p` is metered programmatically (API-like) from 2026-06-15, so the
Claude adapter defaults to interactive to save cost. A PTY exists only to give an
interactive CLI a TTY; it is an adapter capability, not core infrastructure.
_Avoid_: -p mode, batch.

**Complexity routing**:
A Ralphy-invented capability where the planner judges an issue's complexity and
*picks* the execution model (Claude: `sonnet` for mechanical, `opus` for complex).
An **optional adapter capability**, not a core guarantee — a deterministic adapter
(fixed model + fixed effort) is a first-class citizen. Distinct from **effort**,
which is a deterministic knob the operator sets, not an auto-judged choice.
_Avoid_: model selection (too broad), auto-model.

**Supervised session**:
Live human oversight of a *running* agent session — following it and intervening
mid-flight, via Remote Control (mobile) or an on-screen terminal (local/Tauri).
The human is in the loop while the agent works. Distinct from the **Human label**
triage role: there the agent never works the issue at all.
_Avoid_: HITL (reserved for the triage role), human-in-the-loop.

**Daemon**:
The resident "department" of the same `ralphy` binary (`ralphy daemon`; ADR-0032).
A **supervised launcher, never a runtime**: remote commands make it spawn
ordinary run-scoped child processes — exactly the invocations a scheduled
timer would fire (ADR-0026's blessed forms), no more — and it hosts
**workbench sessions**. It never contains a run's execution loop, so "the run,
not the cron" survives it. A run *is the daemon's* only when the daemon
spawned it (it then carries the daemon's identity in its events); a run typed
by hand inside a free-console session is an ordinary manual run. **One daemon
per environment**: WSL is a plain Linux host running its own daemon; the
**control plane** groups a machine's daemons by host. It reaches the control
plane by dialing **out** (see **Control-plane tunnel**); it opens no inbound
port.
_Avoid_: service, server (it dials out), agent (reserved for the CLI vendors),
instance id (the persistent key is `daemon_id`; `runid` stays run-scoped).

**Daemon identity**:
Three layers, three audiences: the `daemon_id` (minted once at install — the
stable machine key that credentials and history reference; humans never see
it), the **name** (operator-given baptism, fleet-unique, renameable — the
handle humans *and models* address: "run X on *anvil*"; names colliding with
command-vocabulary terms — e.g. "forge", "queue" — are refused at
enrollment, or the handle becomes ambiguous to the very models it serves),
and the **emoji
avatar** (cosmetic, non-unique — the daemon's face in fleet UIs). Models and
humans speak the name; machines speak the id — name→id resolves at the
control plane, so a rename never breaks anything in flight. A daemon joins
the **fleet** by **enrollment**: a one-time short-lived code exchanged for a
per-daemon revocable credential (revoking one daemon never shuts the fleet).
_Avoid_: hostname (a suggestion for the name, not the name), token (the
credential is per-daemon and revocable, not a shared static secret).

**Fleet**:
The set of enrolled **daemons** an operator commands through the **control
plane** — many machines, many environments (a Windows host and its WSL distro
are two fleet members grouped under one machine). Daemon **names** are unique
within the fleet; revocation removes one member without touching the rest.
Distinct from the "fleet of Ralphys" in **Emitter identity**, which is about
concurrent *run processes* telling themselves apart in the event stream.
_Avoid_: cluster (no shared workload), farm.

**Forge**:
The service hosting a repo's remotes, issues and labels — GitHub today, and
GitHub only; the word exists so contracts that *could* one day face GitLab /
Gitea / a local mirror are named neutrally (see **Forge query**), not as a
promise that they exist. A repo with no remote has no forge — forge-facing
concepts (queue, labels, forge queries) simply don't apply to it. Historical
prose in this glossary keeps saying "GitHub" where it means today's only
forge; that is accurate, not a violation.
_Avoid_: provider (vague), host (overloaded with **Emitter identity**'s
`host`), platform (that's the **control plane**'s word).

**Forge query**:
The read-only request/response family of the tunnel's command vocabulary: the
**control plane** asks, the **daemon** answers with repository data (issues in
any state, an issue's full thread, labels, branches…) fetched with the
operator's local forge authentication — the platform itself never holds a
forge token (ADR-0019's stance, extended from push to pull). The vocabulary
is **Ralphy's, never the forge's**: verbs are named in this glossary's terms
(issue, thread, label, queue), parameterized and paginated, each backed by a
fixed read-only invocation with the repo always resolved from the **repo
registry**. GitHub is the only implementation today; the forge-neutral
contract is the seam a future GitLab/Gitea slots into. Complementary to the
**queue snapshot** push (ADR-0020): the sink pushes facts, the query answers
questions.
_Avoid_: gateway/proxy (a raw passthrough was rejected — ADR-0032 §6), GitHub
query (the contract is forge-neutral), graph (nothing is graph-shaped here).

**Interactive usage**:
Token consumption from agent CLI sessions the operator drives directly
(terminal or IDE — Claude Code, Codex, OpenCode, Kimi), outside any **run**.
It is recorded by the vendors' own on-disk session stores, **never written to
the ledger** (the ledger stays the runs' record, ADR-0008), and surfaces only
through the **usage scan**. Durability is the **control plane**'s: it polls
and persists, upserting by session id; history older than the vendor's own
retention window is accepted loss (ADR-0033).
_Avoid_: invisible tokens, proxy capture (rejected twice — ADR-0008 D1 and
ADR-0033 §1), manual usage.

**Usage scan**:
The stateless read-time scan that answers the daemon's read-only `usage`
verb (same request/response family as **Forge query**, though it never
touches the forge): parse the four vendors' session stores from scratch,
deduplicate in memory, exclude sessions whose `session_id` a run already
recorded in the ledger, attribute projects via the **repo registry**, and
respond — run records and **interactive usage** records, tokens only, USD
never (read-time pricing, ADR-0008 D8). No background job, no watermarks, no
state: it executes only when asked, so it is on by default and costs nothing
idle. Each daemon scans only its own environment's stores (WSL scans WSL).
Lives in `crates/ralphy-usage-scan` (ADR-0033).
_Avoid_: harvester (nothing runs in the background), proxy, telemetry
(nothing is pushed), collector.

**Repo registry**:
The list of repos a **daemon** can act on, one registry per daemon. It is
**passive**: every `init`/`run`/`triage` upserts its repo, keyed by the
ADR-0008 project identity (`owner/repo` slug) with the path as a mutable
attribute — a moved repo self-heals on its next run, and the key never
breaks. Entries are never auto-deleted, only marked unreachable; removal is a
human act (`ralphy daemon remove`). Explicit `ralphy daemon add` exists only
to register a repo before its first run.
_Avoid_: workspace list, auto-discovery (nothing scans the disk).

**Workbench session**:
A human-driven interactive agent CLI session (Claude/Codex/OpenCode) hosted by
a **daemon** and driven through a browser terminal. Defined by two
coordinates: repo and agent CLI — always spawned **native to the hosting
daemon's OS** (picking a WSL repo means picking the WSL daemon, never a
cross-boundary spawn). Sessions belong to the daemon, not the connection:
the session and its scrollback survive a dropped connection and the browser
**reattaches** (tmux model). The curated launcher (repo × agent) is the
product; a **free console** is a separate, explicit session kind. Distinct
from **Supervised session** (watching a *run's* agent): here the human
drives; no run is involved.
_Avoid_: remote shell (the free-console kind only), terminal (the widget, not
the session), remote session (too generic).

**Control plane**:
The single web application (Phase 2 of ADR-0032; not yet built) where the
**fleet** converges: it consumes run telemetry (the CloudEvents sink,
ADR-0019), relays **control-plane tunnels**, answers the Telegram command
bot, and serves the fleet UI with browser terminals. One platform, two data
paths by nature — fire-and-forget events from ephemeral runs, live tunnel
from resident daemons — separate in protocol, converging only here. It holds
no GitHub token; Ralphy remains the bus.
_Avoid_: dashboard (it commands, not just displays), relay (one of its roles,
not the whole), events platform (subsumed).

**Control-plane tunnel**:
The single persistent outbound connection each **daemon** holds to the
**control plane**, multiplexing terminal streams and control commands, plus a
presence heartbeat. Carries only what is *interactive*; run telemetry stays
on the CloudEvents sink (ADR-0019). The relay side is a stateless bridge
(session state lives in the daemon), but it sits in the trust path — the
relay host is critical infrastructure.
_Avoid_: webhook, event channel (that's the sink), gateway (the daemon is not
a server).

**Blocked by / dependency gating**:
An issue's `## Blocked by` section names other issues (`#N`) it depends on. The
runner gates on it: if any named blocker is still **open**, the blocked issue is
*skipped* this run (not closed, not a stop) and picked up by a later run once the
blocker clears. A blocker counts as satisfied when simply **closed** — safe only
because every issue in a run shares one branch (see ADR-0002).
_Avoid_: depends-on, prerequisite, stop-before (that's flow control, not a dependency).

**stop-before**:
A fixed control label (not configurable) on one queued issue that halts the run
**before** that issue is worked — everything earlier in the sequence is done
first. The human creates the label, removes it from the issue, and re-runs to
continue. Not a triage role — a flow control, named for its semantics.
_Avoid_: pause, hold, breakpoint.

**Init / onboarding**:
The interactive command (`ralphy init`) that brings an *unprepared* repo to a
state `ralphy run` can work: it validates the environment, scaffolds `.ralphy/`
and `docs/agents/*`, creates the **labels**, installs the engineering skills, and
turns an existing backlog into **issues**. Rust owns all control flow, gates, git,
labels, and the interactive questions; it spawns agent sessions only for the
read/judgment work, each receiving a fully-assembled non-interactive prompt. It
**inverts setup-pocock**: the asking moves into a Rust console Q&A, and the skill's
templates are fed answers instead of interviewing. See [docs/adr/0012](./docs/adr/0012-init-onboarding-command.md).
_Avoid_: setup (overloaded with the setup-pocock skill), bootstrap, scaffold (only
one stage of init).

**Repo diagnosis**:
The read-only first agent pass of **init** that scans the target repo and returns a
structured report (against a Rust-defined schema) describing what is and isn't
present — existing project vs empty, backlog/milestone docs, existing agent skill
dirs, domain docs, remote host. It runs from a **neutral cwd** with the repo passed
as data, so the target's `CLAUDE.md`/`AGENTS.md` are *read as data*, never
auto-loaded as instructions that could sabotage the diagnosis. Its output pre-fills
the init Q&A.
_Avoid_: scan, audit (reserved for security/review), analysis.

## Relationships

- The **queue** = open issues carrying any **queue label**, ascending by number.
- A green **queue** issue is closed by the runner (the **cycle**); a non-green one
  stops the whole run and hands over the **run branch** for inspection.
- A **human label** issue is never in the **queue**.
- A **stop-before** issue halts the run before itself; the issues before it still run.
- A **blocked-by** issue with an open blocker is skipped (not stopped); later
  unrelated issues still run. Closing a green issue also writes its **acceptance
  ledger** back as **evidence** — without changing what makes it **green**.
- The **core** is execution-mode-agnostic: it asks an **adapter** to work an issue
  and receives an outcome. PTY, interactive sessions, and completion sentinels
  live inside the **adapter**, never in the core.
- A **daemon** launches runs but never contains them: a daemon-spawned run is
  a normal run that additionally carries the daemon's identity in its events;
  a cron or manual run (including one typed inside a **free console**) simply
  doesn't. Observability never depends on a daemon existing.
- A **workbench session** involves no run; a **Supervised session** watches a
  run. Tokens either kind burns are **interactive usage** — visible to the
  **usage scan**, never in the ledger. The **control plane** sees both worlds — tunnel (interactive) and
  CloudEvents sink (telemetry) — but the two never share a channel.
- **Triage evidence** = the issue body, its full comment thread, and the
  guardrailed attachments the CLI fetches for the triage agent (ADR-0025); an
  attachment listed `not fetched (<reason>)` is evidence the agent does **not**
  have, never treated as absent.

## Testing conventions

- **Subprocess/PTY plumbing is tested against a dedicated helper bin**, located
  via `CARGO_BIN_EXE_<name>` from an integration test under `tests/` — see
  `ralphy-adapter-support`'s `headless_test_child` driven by `tests/headless.rs`.
  `CARGO_BIN_EXE_*` is only reliable in integration tests (not lib unit tests),
  and shell-script children are not portable to Windows CI; plans that test
  child-process behavior should follow this pattern.

## Refactoring conventions

- **Splitting files over 500 lines** follows the guardrails in
  [docs/adr/0022](./docs/adr/0022-file-split-conventions.md): `foo.rs` + `foo/`
  layout (never `mod.rs`), public API unchanged (re-export from the parent),
  tests migrate with the code they exercise, and every split PR is gated on
  `/rust-skills` + `cargo test` + `cargo clippy` green. Split by existing
  responsibility only — no new abstractions to justify a file boundary.

## Flagged ambiguities

- "AFK" and "ready-for-agent" are treated as synonyms (same for "HITL" /
  "ready-for-human"). Canonical is the Matt Pocock role; the shorthand is a
  transitional alias.
- "HITL" was used to mean both the **Human label** triage role (agent never works
  the issue) and live human oversight of a running session — resolved: HITL is
  **only** the triage role; the oversight concept is a **Supervised session**.
- "Fleet" was used loosely for both concurrent run processes ("fleet of
  Ralphys", **Emitter identity**) and the set of enrolled daemons — resolved:
  **Fleet** canonically means the enrolled daemons; the event-stream sense is
  descriptive prose, keyed by `runid`, never by fleet membership.
- "GitHub" vs "forge": **Forge** is the neutral term — already used
  informally (ADR-0021: "the forge does: the GitHub assignee"), canonized
  2026-07-09 for contracts that must not bake in a vendor dialect
  (**Forge query**). GitHub
  is the only forge and existing prose naming it stays as-is; new
  cross-boundary contracts say forge, vendor-specific mechanics say GitHub.
