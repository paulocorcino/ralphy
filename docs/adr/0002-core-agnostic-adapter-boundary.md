# Core is execution-mode-agnostic; adapters own how an agent is driven

The Ralphy rewrite (Rust workspace) splits into a `ralphy-core` that knows the
*method* — queue, run lifecycle, branch policy, stop-at-non-green, close-on-green —
and one **adapter** per agent CLI vendor (Claude Code today; Codex, OpenCode later).
The core never names `claude`, `--settings`, `sonnet`/`opus`, PTY, or any
completion sentinel. The `Agent` trait it defines is execution-mode-agnostic:

```rust
fn plan(&self, issue: &Issue, ws: &Workspace) -> Result<Plan>;
fn execute(&self, plan: &Plan, ws: &Workspace) -> Result<Outcome>;
```

Everything vendor-specific lives behind that trait, inside the adapter: the
**execution mode** (interactive over a PTY vs headless `-p`), the **PTY** itself,
**completion detection** (Stop hook + transcript + flag file), and **complexity
routing** (judging an issue and picking a model). These are adapter capabilities,
not core guarantees — a purely deterministic adapter (fixed model + fixed effort,
no PTY) is a first-class citizen.

## Why this is load-bearing

The motivating constraint is a Claude Code particularity: from 2026-06-15 the
headless `-p` path is metered programmatically (API-like), so interactive-over-PTY
is the only way to bill against the subscription. That makes interactive/PTY the
Claude adapter's primary path — but it is *not* a property of the domain. Codex and
OpenCode do not need interactive mode today. If the core knew about PTY, every
future adapter would inherit a Claude-shaped assumption.

## Considered options

- **PTY and interactive sessions in `ralphy-core`** (rejected): treats the
  subscription-billing workaround as if it were domain infrastructure; leaks
  `claude`-shaped assumptions into every future adapter.
- **`execute(.., pty: &mut dyn Pty)` — PTY in the trait signature** (rejected):
  the core would have to know PTYs exist, defeating the boundary even if the PTY
  type lived elsewhere.
- **Core defines `plan`/`execute` only; adapters own PTY/mode/completion/routing**
  (chosen): the only thing that must be right up front is the PTY-free trait
  signature; where the `portable-pty`-backed `ralphy-pty` crate is consumed, and
  how completion is detected, stay reversible inside the adapter.

## Consequences

- The PTY-free trait signature is the one hard-to-reverse commitment; the rest
  (which crate holds the PTY, transcript-tail vs Stop-hook, tier vs literal model
  name) is swappable inside an adapter without touching the core or other adapters.
- `ralphy-pty` is a shared crate (consumers: Claude interactive exec now; on-screen
  terminal / Tauri and supervised sessions later), not core infrastructure.
- There is deliberately **no shared "headless runner" or `Outcome` runner that
  adapters bend to fit**: the only surface shared between adapters is the core's
  `Agent` trait and `Outcome` enum. Each adapter owns its own raw-output→signal
  detection; vendor-neutral plumbing (`ralphy-adapter-support`) may share
  mechanical steps and, per ADR-0023, the fixed signal→`Outcome` ordering, but
  never the detection itself. (ADR-0004 first articulated this invariant when the
  second adapter arrived; it is a boundary property, recorded here as its home.)
- The plan artifact may keep emitting Claude model names (`sonnet`/`opus`) at
  parity, confined to a single tier↔model translation point in the Claude adapter;
  moving to an abstract tier is a deliberate later improvement, not part of the port.

## Amendment (2026-07-02): vendor vocabulary is injected into core, never known by it (#79)

The boundary now also covers *vocabulary*, not just execution mode. Four moves:

- **Completion sentinel.** The `RALPHY_DONE_EXIT` literal is named once, as
  `ralphy_adapter_support::DONE_SENTINEL`. Detection stays in the adapters
  (`done_sentinel`/`blocked_reason`). The core's repair briefs
  (`protocol::failure_brief`, `verify::repair_brief` — the ADR-0011/ADR-0015
  hand-back files) still *quote* the token so the brief speaks the agent's own
  protocol, but they receive it as data: `QueueConfig.done_signal`, populated by
  the CLI from the constant. Core source contains no sentinel literal. "Lint
  this completion" is thus a structured request (plan markdown in,
  `ProtocolReport` out, token threaded through for prose) — chosen over a
  core-owned constant because this ADR and ADR-0004 already place every
  completion-protocol decision with the adapters.
- **Model names.** The `## Execution model: opus|sonnet` parser moved into the
  Claude adapter (its only caller; Codex already kept a private tier mirror).
  `Plan.recommended_model` is an opaque token the core carries across without
  interpreting — this formalizes the "tier vs literal model name is
  adapter-internal" consequence above.
- **Settings.** `Settings` keeps only agent-agnostic keys plus a generic
  per-agent section blob (`agent_settings`/`set_agent_settings` over the
  ADR-0010 `extra` flatten); `ClaudeSettings`/`OpenCodeSettings` live in their
  adapter crates. The on-disk `settings.json` schema and the ADR-0010
  flag > settings > default precedence are unchanged.
- **Paths.** `Workspace::plugin_dir` moved into the Claude adapter, derived
  from the vendor-neutral `ralphy_dir()`.

Enforced by: `grep -riE "opencode|claude|codex|opus|sonnet|RALPHY_DONE_EXIT"
crates/ralphy-core/src` returning no hits (#79).

## Amendment (2026-07-24): the Claude plan gains an `opus-high` rung — opus thinking hard, chosen per issue

D3-style routing on the Claude path picks the **model** (`opus`/`sonnet`) from
the plan's `## Execution model` judgment, while reasoning effort has been a
separate, run-wide operator knob (`--exec-effort`, default absent) that the plan
never touched. So "run this issue on opus at high effort" was only reachable by
forcing it across the *whole* run (`--exec-model opus --exec-effort high`), not as
a per-issue judgment the planner makes from the work in front of it. This mirrors
the gap ADR-0004's 2026-07-24 amendment closed for Codex with the `xhigh` rung.

**Decision.** The Claude plan judgment gains a third, effort-bearing rung
`opus-high`, kept in the adapter's own model-name vocabulary (not the neutral
`low..xhigh` ladder — Claude deliberately speaks model names here, per the
"Model names" move above):

| plan judgment | model  | effort                    |
| ------------- | ------ | ------------------------- |
| `sonnet`      | sonnet | operator default (absent) |
| `opus`        | opus   | operator default (absent) |
| `opus-high`   | opus   | **high**                  |

- **Effort precedence mirrors the model's.** `resolve_exec_effort` resolves, in
  order: the operator's `--exec-effort` (or persisted `claude.exec_effort`) →
  the plan-derived effort (`opus-high` → `high`) → absent (Claude's own default).
  An explicit operator flag still wins on every issue — the operator is never
  denied. Only the *unset* case now lets the plan raise effort.
- **Minimal coupling, on purpose.** Only the new `opus-high` rung derives an
  effort; `sonnet` and `opus` keep running at whatever the operator's effort
  resolves to today (a full per-rung effort table was rejected as unearned
  surface — the same call ADR-0004 made). This is the smallest change that lets
  the plan ask for opus:high.
- **Storage is unchanged.** `opus-high` is carried in the same opaque
  `Plan.recommended_model` token the core already passes across without
  interpreting; `resolve_exec_model` normalizes `opus-high` → the `--model opus`
  argv, and `resolve_exec_effort` reads the same token for the effort. No change
  to the core `Plan` shape, and the `grep` guard above still holds (the token
  lives only in the adapter and the plan artifact).
- **Both execution paths honour it.** The interactive PTY session and the
  headless `-p` loop both resolve effort through `resolve_exec_effort(plan)`, so
  the rung reaches `--effort high` whichever path runs.
- **Codex parity.** This is the Claude-vocabulary twin of ADR-0004's
  `xhigh`→`gpt-5.6-sol` at high effort; the `## Execution model` parser and the
  neutral `Plan.recommended_model` seam are otherwise untouched.
