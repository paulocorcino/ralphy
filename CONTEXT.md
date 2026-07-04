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
Code today; Codex, OpenCode later), behind the core's agent contract. Each
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
_Avoid_: instance id (implies persistence we don't have), session id.

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
