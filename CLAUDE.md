# CLAUDE.md

Operational guide for agents working **on Ralphy's own codebase**. This is a
thin pointer — the substance lives in the docs below. Read them, don't restate
them here.

- **[CONTEXT.md](./CONTEXT.md)** — the ubiquitous language. Every domain term
  (run, queue, adapter, planner/executor, event sink, blocked-by, stop-before…)
  is defined there. Use these words; don't invent synonyms.
- **[docs/adr/](./docs/adr/)** — architecture decisions. Check for a relevant
  ADR before changing a seam; the boundary you're about to cross was probably
  decided on purpose (e.g. ADR-0004 core/adapter boundary).
- **[docs/BUILDING.md](./docs/BUILDING.md)** — build, CI, crate layout.

## Hard rules (an agent will get these wrong without being told)

- **The green gate is CI's gate.** Before considering a change done:
  `cargo fmt --check`, `cargo clippy -- -D warnings` (warnings are errors), and
  `cargo test`. All three must pass.
- **Cross-platform, always.** CI builds and tests on **both Windows and Linux**.
  No POSIX-only assumptions; no shell-script test children — subprocess/PTY
  behaviour is tested against a Rust helper bin (see CONTEXT.md → *Testing
  conventions*).
- **Public crate API is stable by default.** Reorganizing internals must not
  change the `pub` surface or import paths; re-export from the parent module.
  Changing the public API is a deliberate design decision, not a side effect.
- **Splitting files over 500 lines** follows
  [ADR-0022](./docs/adr/0022-file-split-conventions.md): `foo.rs` + `foo/`
  layout (never `mod.rs`), tests migrate with the code, split by existing
  responsibility only.
- **Contributing to this repo:** commit on a branch; a human reviews and merges.
  Do not push or open a PR unless explicitly asked. (This mirrors Ralphy's own
  product ethos — it never pushes and never opens PRs.)

## Where things live

`crates/ralphy-cli` (the `ralphy` binary + composition root) ·
`crates/ralphy-core` (queue lifecycle, git/GitHub, run reporting) ·
`crates/ralphy-agent-{claude,codex,opencode}` (the vendor adapters) ·
`crates/ralphy-adapter-support` (vendor-neutral child-driving plumbing) ·
`crates/ralphy-pty` · `assets/prompts` (plan/execute charters) ·
`assets/plugin` (bundled skills, embedded into the binary).

#teste 1