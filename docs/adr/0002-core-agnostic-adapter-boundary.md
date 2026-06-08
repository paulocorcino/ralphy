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
- The plan artifact may keep emitting Claude model names (`sonnet`/`opus`) at
  parity, confined to a single tier↔model translation point in the Claude adapter;
  moving to an abstract tier is a deliberate later improvement, not part of the port.
