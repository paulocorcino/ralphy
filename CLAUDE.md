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

## Architecture — ports & adapters, ubiquitous-language-first

Ralphy is **hexagonal (ports & adapters)** at the crate seam, with DDD in its
**tactical** sense only: the [CONTEXT.md](./CONTEXT.md) glossary *is* the
ubiquitous language and each crate is roughly one bounded context. There is no
strategic-DDD ceremony here — no aggregates, repositories, or domain-event
buses. Don't add them.

- **`ralphy-core` is the center and depends on no vendor.** It defines the agent
  contract (the *port*) and owns queue lifecycle, git/forge, run reporting. It
  must never gain a dependency on a `ralphy-agent-*` crate or on
  `ralphy-adapter-support`; the dependency arrow points *inward*, toward core
  (ADR-0004 protects this seam — the codebase cites it by that number, though
  the file currently lives at `docs/adr/0002-core-agnostic-adapter-boundary.md`,
  a known numbering drift). If core seems to need something vendor-specific,
  the design is wrong — lift it behind the contract, don't leak it in.
- **Each `ralphy-agent-*` is an adapter** implementing that port: one crate per
  vendor, holding all that is vendor-specific (execution mode, completion
  protocol). **`ralphy-adapter-support`** is the vendor-*neutral* plumbing the
  adapters share — it produces no `Outcome` (CONTEXT.md → *Adapter support*).
- **`ralphy-cli` is the composition root** — the one place that names every
  vendor and wires them together. Vendor enumeration lives *there and only
  there* (plus the [ADR-0040](./docs/adr/0040-agent-adapter-onboarding-contract.md)
  inventory), never scattered across the tree.

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
- **Tests live next to what they test — separated by `#[cfg(test)]`, not by a
  parallel source tree.** That is this repo's convention *and* idiomatic Rust,
  and `#[cfg(test)]` compiles the code out of release builds, so nothing
  test-only ever ships — that gate is the "don't mix production and tests"
  guarantee, not a separate root. Placement:
  *unit tests* stay in the same crate as the code, either an inline
  `#[cfg(test)] mod tests` or — once a file splits (ADR-0022) — a sibling
  `#[cfg(test)]` submodule file (`foo/tests.rs`, or a named one like
  `runstate/roundtrip.rs`); *integration tests* (black-box, public API only) go
  in the crate's `tests/`, with data under `tests/fixtures/`; a **test helper
  child binary** goes in `src/bin/<name>_test_child.rs`, because its
  `CARGO_BIN_EXE_*` is visible only to integration tests (CONTEXT.md →
  *Testing conventions*).
- **Smallest change that fits the existing seam.** A new trait, generic, crate,
  or layer of indirection needs a real second caller or a deciding ADR — never
  "for flexibility" (`anti-over-abstraction`). Cross a seam only where an ADR
  says to; if no ADR covers the boundary you're about to add, the change is
  probably in the wrong place, or the seam is a design decision that wants an
  ADR first.
- **English is the canonical written language.** ADRs, docs, GitHub issues and
  PRs, commit messages and code comments are written in English, whatever
  language the request arrived in. A conversation with a maintainer may be in
  any language; the artifact is English. Issues in particular are work orders an
  agent consumes, and they quote English ADRs, identifiers and paths — prose in
  a second language makes one document speak two per sentence.
- **Contributing to this repo:** commit on a branch; a human reviews and merges.
  Do not push or open a PR unless explicitly asked. (This mirrors Ralphy's own
  product ethos — it never pushes and never opens PRs.)

## Rust baseline (the always-on floor)

The full `/rust-skills` (179 rules) is a surgical tool — invoke it to review
non-trivial code or a specific concern. These few are the minimum that hold
without invoking anything; they apply to every change. Each names the underlying
rule so `/rust-skills <name>` gives you the bad/good example on demand.

- **Errors — this codebase is a subprocess driver, so errors are the hot path.**
  No `.unwrap()`/`.expect()` on anything recoverable (spawn, I/O, git, network,
  parse); `expect()` is allowed *only* for a violated invariant that is a bug,
  and its message states why the invariant holds (`anti-unwrap-abuse`,
  `anti-panic-expected`, `err-expect-bugs-only`). Never swallow an error — no
  `let _ = result`, no bare `.ok()`, no empty `if let Err(_)`: handle or
  propagate (`anti-empty-catch`). Propagate with `?` and add
  `.context()`/`.with_context()` at each boundary so the chain reads
  "what failed: why" (`err-context-chain`). Error messages start lowercase with
  no trailing punctuation — they get chained (`err-lowercase-msg`). `anyhow` at
  the app/composition boundary; a `thiserror` domain type at a seam callers must
  match on (`err-anyhow-app`, `err-custom-type`).
- **Signatures — free flexibility clippy would flag anyway.** Take `&str` not
  `&String`, `&[T]` not `&Vec<T>` (`anti-string-for-str`, `anti-vec-for-slice`).
  A fixed set of values or a semantic identity is an `enum`/newtype, not a
  `String` — this is the CONTEXT.md ubiquitous language expressed in the type
  system (`anti-stringly-typed`).
- **Idiom & restraint.** Iterators over manual `for i in 0..len` indexing; don't
  `.collect()` mid-chain (`anti-index-over-iter`, `anti-collect-intermediate`).
  `impl Trait` over `Box<dyn Trait>` when the type is concrete; start concrete
  and generalize on a real second use, not "for flexibility" (`anti-type-erasure`,
  `anti-over-abstraction`). No optimization without a profile
  (`anti-premature-optimize`).
- **Async (daemon only).** Never hold a lock guard across an `.await`; use
  `tokio::sync` primitives and drop the guard first (`anti-lock-across-await`).

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