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
- **English is the canonical written language.** ADRs, docs, GitHub issues and
  PRs, commit messages and code comments are written in English, whatever
  language the request arrived in. A conversation with a maintainer may be in
  any language; the artifact is English. Issues in particular are work orders an
  agent consumes, and they quote English ADRs, identifiers and paths — prose in
  a second language makes one document speak two per sentence.
- **Contributing to this repo:** commit on a branch; a human reviews and merges.
  Do not push or open a PR unless explicitly asked. (This mirrors Ralphy's own
  product ethos — it never pushes and never opens PRs.)

## Where things live

`crates/ralphy-cli` (the `ralphy` binary + composition root) ·
`crates/ralphy-core` (queue lifecycle, git/GitHub, run reporting) ·
`crates/ralphy-agent-*` (**the vendor adapters — one crate per vendor**;
`claude`, `codex`, `kimi`, `opencode` today, more arriving) ·
`crates/ralphy-adapter-support` (vendor-neutral child-driving plumbing) ·
`crates/ralphy-daemon` (the supervised launcher + workbench) ·
`crates/ralphy-usage-scan` (stateless reads of the vendors' session stores) ·
`crates/ralphy-pty` · `crates/ralphy-proc-util` ·
`assets/prompts` (plan/execute charters) ·
`assets/plugin` (bundled skills, embedded into the binary).

Adding a vendor is not just a new crate: follow
[ADR-0040](./docs/adr/0040-agent-adapter-onboarding-contract.md), whose wiring
inventory lists every edit site across five tiers. **Do not enumerate the vendor
crates anywhere a list can go stale** — that list has already drifted once
(Kimi was missing from this section and is still missing from the daemon's
agent enum).