# CLAUDE.md

Operational guide for agents working **on Ralphy's own codebase**. It holds the
rules an agent gets wrong without being told, and it points to the documents
that hold the details. When a rule here and its source document disagree, the
source document is correct — fix this file.

- **[CONTEXT.md](./CONTEXT.md)** — the ubiquitous language. Every domain term
  (run, queue, adapter, planner/executor, event sink, blocked-by, stop-before…)
  is defined there. Use these words; don't invent synonyms.
- **[docs/adr/](./docs/adr/)** — architecture decisions. Check for a relevant
  ADR before you change a boundary between crates. That boundary was probably
  decided on purpose (for example, ADR-0002 on the core/adapter boundary).
- **[docs/BUILDING.md](./docs/BUILDING.md)** — build, CI, crate layout.

## Architecture — ports & adapters, ubiquitous-language-first

Ralphy is **hexagonal (ports & adapters)** at the crate boundary. It uses DDD
only in the **tactical** sense: the [CONTEXT.md](./CONTEXT.md) glossary *is* the
ubiquitous language, and each crate is roughly one bounded context. There are no
aggregates, repositories, or domain-event buses. Don't add them.

- **`ralphy-core` is the center and depends on no vendor.** It defines the agent
  contract (the *port*) and owns the queue lifecycle, git/forge, and run
  reporting. It must never depend on a `ralphy-agent-*` crate or on
  `ralphy-adapter-support`. Dependencies point *inward*, toward core
  ([ADR-0002](./docs/adr/0002-core-agnostic-adapter-boundary.md)). If core seems
  to need something vendor-specific, the design is wrong: put it behind the
  contract instead.
- **Each `ralphy-agent-*` crate is an adapter** that implements that port. There
  is one crate per vendor, and it holds everything that is vendor-specific
  (execution mode, completion protocol). **`ralphy-adapter-support`** is the
  vendor-*neutral* code that the adapters share. It produces no `Outcome`
  (CONTEXT.md → *Adapter support*).
- **`ralphy-cli` is the composition root** — the one place that names every
  vendor and connects them. The list of vendors lives *only there* (plus the
  [ADR-0040](./docs/adr/0040-agent-adapter-onboarding-contract.md) inventory).
  Do not copy it anywhere else.

## Hard rules (an agent will get these wrong without being told)

- **Run CI's gate before you call a change done.** These are the commands CI
  runs; all of them must pass:

  ```sh
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo nextest run --workspace
  cargo test --workspace --doc
  ```

  `--all-targets` matters: without it, clippy does not check test code, and CI
  fails on warnings you never saw. Nextest does not run doctests, so the last
  command is not optional. Install nextest with
  `cargo install cargo-nextest --locked`. It is about 50% faster than
  `cargo test` here, because `cargo test` runs this workspace's ~50 test
  binaries one at a time. On Windows the suite is limited by process creation,
  not by Rust — see [docs/BUILDING.md](./docs/BUILDING.md).
  **If you change `crates/ralphy-daemon/assets/ui/` or `ui-tests/`, also run**
  `node --test crates/ralphy-daemon/ui-tests`. It needs no `npm install`
  ([ADR-0057](./docs/adr/0057-the-workbench-asset-contract.md) D3). A new
  `*.test.mjs` file must be imported by `ui-tests/index.mjs`, or the runner never
  opens it; a Rust test fails if you forget. **UI text also needs**
  `cargo run -q -p xtask -- ui-copy --check`: it applies
  [ADR-0065](./docs/adr/0065-the-workbench-written-voice.md) and CI fails on a
  violation.
- **A change a user can see needs a changelog fragment.** Add one file per PR,
  `changelog.d/<n>.md`, with a `kind:` from the closed set (`feature`, `fix`,
  `breaking`, `security`, `internal`), a `headline:` for the release page, and a
  sentence that names the *capability*, not the diff. The sentence is at most
  280 characters. A fragment that follows up a feature from the same release
  takes that feature's `topic:`, so it stays off the release page. Use
  `internal` for a refactor that a user cannot see; it is never printed. On pull
  requests, CI fails when the fragment is missing. Check that fragments parse
  with `cargo run -q -p xtask -- changelog --check`. Format:
  [changelog.d/README.md](./changelog.d/README.md); decision:
  [ADR-0056](./docs/adr/0056-release-communication-and-the-update-watch.md).
  Never edit `CHANGELOG.md` or `changelog.json` — the `changelog` xtask owns
  them.
- **Cross-platform, always.** CI builds and tests on **Windows, Linux and
  macOS**. Make no POSIX-only assumptions. Test children are never shell
  scripts: subprocess and PTY behavior is tested against a Rust helper binary
  (CONTEXT.md → *Testing conventions*).
- **The public crate API is stable by default.** Moving code inside a crate must
  not change the `pub` surface or its import paths; re-export from the parent
  module. A change to the public API is a design decision, not a side effect.
- **Files over 500 production lines are split by
  [ADR-0022](./docs/adr/0022-file-split-conventions.md).** Only lines above
  `#[cfg(test)]` count. Use the `foo.rs` + `foo/` layout (never `mod.rs`), move
  tests with their code, and split only along responsibilities that already
  exist. **Separately, an inline `#[cfg(test)] mod tests { … }` block over 500
  lines moves to `foo/tests.rs`** (`src/tests.rs` for a crate root), whatever the
  production size, and the move touches no production code. If a test you add
  pushes the block over 500 lines, the move is part of your change. A test in
  `xtask` enforces this.
- **Comments state what the code cannot show:** an invariant, a measured fact
  (with the tool and version), a limit, or the ADR/issue that decided it. A
  comment does not describe the previous diff, the bug report, or a rejected
  alternative. When code moves, its comment moves with it or is deleted.
- **Tests live next to the code they test, inside `#[cfg(test)]`,** not in a
  parallel source tree. `#[cfg(test)]` removes the code from release builds, so
  nothing test-only ships. Unit tests stay in the same crate: an inline
  `#[cfg(test)] mod tests`, or, after a split, a sibling file such as
  `foo/tests.rs`. Integration tests (public API only) go in the crate's
  `tests/`, with data in `tests/fixtures/`. A **test helper child binary** goes
  in `src/bin/<name>_test_child.rs`, because `CARGO_BIN_EXE_*` is only visible
  to integration tests (CONTEXT.md → *Testing conventions*).
- **Make the smallest change that fits the existing crate boundaries.** A new
  trait, generic, crate, or layer of indirection needs a real second caller or
  an ADR that decides it — never "for flexibility" (`anti-over-abstraction`).
  Cross a crate boundary only where an ADR allows it. If no ADR covers the
  boundary you are about to add, the change is probably in the wrong place, or
  the boundary needs an ADR before any code.
- **English is the written language of the repo.** ADRs, docs, GitHub issues
  and PRs, commit messages, and code comments are in English, whatever language
  the request came in. A conversation with a maintainer can be in any language;
  what you write into the repo is English. Issues matter most: an agent executes
  them, and they quote English ADRs, identifiers, and paths, so an issue in
  another language mixes two languages in every sentence.
- **Plain English, no idioms.** Everything written in this repo is read by
  people for whom English is a second language: UI text, changelog, docs, ADRs,
  issues, commits, and comments. Use common words and short, direct sentences.
  Don't use idioms or figures of speech (*reads at a glance*, *for free*, *out
  from under*, *a trip through*). A clear, literal sentence is better than a
  clever one. Technical terms from [CONTEXT.md](./CONTEXT.md) are fine; slang is
  not.
- **Commit on the current branch.** Do not create a branch, push, or open a PR
  unless someone asks you to. A human reviews and merges. (Ralphy itself works
  the same way: it never pushes and never opens PRs.)

## Rust baseline (applies to every change)

The full `/rust-skills` set (179 rules) is for reviewing non-trivial code or a
specific concern; invoke it when you need it. The rules below apply to every
change without invoking anything. Each one names its rule, so
`/rust-skills <name>` shows the bad and good examples.

- **Errors — Ralphy drives subprocesses, so most code paths can fail.** No
  `.unwrap()`/`.expect()` on anything recoverable (spawn, I/O, git, network,
  parse). `expect()` is allowed *only* for a broken invariant that would be a
  bug, and its message says why the invariant holds (`anti-unwrap-abuse`,
  `anti-panic-expected`, `err-expect-bugs-only`). Never ignore an error — no
  `let _ = result`, no bare `.ok()`, no empty `if let Err(_)`: handle it or
  return it (`anti-empty-catch`). Return errors with `?` and add
  `.context()`/`.with_context()` at each boundary, so the chain reads
  "what failed: why" (`err-context-chain`). Error messages start lowercase and
  have no final punctuation, because they are chained (`err-lowercase-msg`).
  Use `anyhow` in the application and composition code, and a `thiserror` type
  where callers must match on the error (`err-anyhow-app`, `err-custom-type`).
- **Signatures.** Take `&str`, not `&String`, and `&[T]`, not `&Vec<T>`
  (`anti-string-for-str`, `anti-vec-for-slice`). A fixed set of values or a
  domain identity is an `enum` or newtype, not a `String` — this is how the
  CONTEXT.md vocabulary shows up in the types (`anti-stringly-typed`).
- **Idiom and restraint.** Use iterators, not manual `for i in 0..len`
  indexing, and don't `.collect()` in the middle of a chain
  (`anti-index-over-iter`, `anti-collect-intermediate`). Prefer `impl Trait` to
  `Box<dyn Trait>` when the type is concrete. Start concrete and generalize only
  when a real second use appears (`anti-type-erasure`,
  `anti-over-abstraction`). Don't optimize without a profile
  (`anti-premature-optimize`).
- **Async (daemon only).** Never hold a lock guard across an `.await`. Use
  `tokio::sync` primitives and drop the guard first (`anti-lock-across-await`).

## Where things live

- `crates/ralphy-cli` — the `ralphy` binary and the composition root.
- `crates/ralphy-core` — queue lifecycle, git/GitHub, run reporting.
- `crates/ralphy-agent-*` — the vendor adapters, one crate per vendor. List the
  directory to see the current set.
- `crates/ralphy-adapter-support` — vendor-neutral code for driving agent child
  processes.
- `crates/ralphy-daemon` — the supervised launcher and the workbench.
- `crates/ralphy-usage-scan` — stateless reads of the vendors' session stores.
- `crates/ralphy-pricing` — the price table read at report time. It depends on
  neither core nor an adapter, so both sides may depend on it (ADR-0034 D6).
- `crates/ralphy-release` — version identity and the published-release read.
  A leaf crate for the same reason (ADR-0056).
- `crates/ralphy-run-snapshot`, `crates/ralphy-pty`, `crates/ralphy-proc-util`
  — supporting crates.
- `crates/xtask` — repo tooling: the changelog, and the checks that enforce this
  file's rules.
- `assets/prompts` — the plan and execute charters. Plan prompts are generated
  from `assets/prompts/plan/` (see its README).
- `assets/plugin` — bundled skills, embedded into the binary.

Adding a vendor takes more than a new crate: follow
[ADR-0040](./docs/adr/0040-agent-adapter-onboarding-contract.md). Its inventory
lists every place that must change, across five tiers. **Do not write a list of
vendors anywhere it can go out of date**, including this file.
