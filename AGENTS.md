# AGENTS.md

Operational guide for agents working **on Ralphy's own codebase**. It holds the
rules an agent gets wrong without being told, and it points to the documents
that hold the details. When a rule here and its source document disagree, the
source document is correct — fix this file.

- **[docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md)** — the architecture map:
  the owner of each fact, who may call whom, and where an outside product or
  platform may be used. Read its fact index before you add an outside call
  (`gh`, an HTTP API, a vendor CLI), a new path between browser, daemon and
  CLI, a watcher or timer, a new panel or other place in the workbench that
  shows a fact, or a second computation of a fact the product already knows.
  Get the fact from its owner. Its §4 is the map of crates.
- **[CONTEXT.md](./CONTEXT.md)** — the ubiquitous language. Every domain term
  (run, queue label, adapter, planner/executor, event sink…) is defined there.
  Use these words and only these words.
- **[docs/adr/](./docs/adr/)** — architecture decisions. Read the relevant ADR
  before you change or add a boundary between crates. A boundary that no ADR
  covers needs an ADR before any code, and the change is probably in the wrong
  place. A new ADR starts from [docs/adr/TEMPLATE.md](./docs/adr/TEMPLATE.md).
- **[docs/BUILDING.md](./docs/BUILDING.md)** — build, CI workflows, releases.
- **[docs/TESTING.md](./docs/TESTING.md)** — how to write a test that fails
  only when the behavior breaks. Read it before you add, change, or review a
  test.

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
  vendor-*neutral* code that the adapters share. Its one `Outcome` comes from
  the shared classifier ([ADR-0023](./docs/adr/0023-shared-outcome-classifier.md)).
- **`ralphy-cli` is the composition root** — the one place that connects every
  vendor. A list of vendors lives only there and in the places the
  [ADR-0040](./docs/adr/0040-agent-adapter-onboarding-contract.md) inventory
  names (for example the daemon roster and the usage scan) — never in a doc,
  including this file. A new place that needs the list is added to that
  inventory first. Adding a vendor means changing every place in that
  inventory, across its five tiers, not only adding a crate.

## Hard rules (an agent will get these wrong without being told)

- **Run CI's gate before you call a change done.** The source of the gate is
  [.github/workflows/ci.yml](./.github/workflows/ci.yml); an ADR or doc that
  lists other commands is out of date. These are the local forms of what CI
  runs, and all of them must pass:

  ```sh
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo nextest run --workspace
  cargo test --workspace --doc
  ```

  `--all-targets` matters: without it, clippy does not check test code, and CI
  fails on warnings you never saw. Nextest does not run doctests, so the last
  command is not optional. Install nextest with
  `cargo install cargo-nextest --locked`. Why nextest, and why the suite is
  slow on Windows: [docs/BUILDING.md](./docs/BUILDING.md).

  **If you change `crates/ralphy-daemon/assets/ui/` or `ui-tests/`,** these
  must pass too:

  ```sh
  node --test crates/ralphy-daemon/ui-tests
  npx -y oxlint@1.85.0 --deny-warnings crates/ralphy-daemon/assets/ui crates/ralphy-daemon/ui-tests
  cargo run -q -p xtask -- ui-copy --check
  ```

  A new `*.test.mjs` file runs only when `ui-tests/index.mjs` imports it.
- **A change a user can see needs a changelog fragment:** one file per PR,
  `changelog.d/<n>.md`, whose sentence names the *capability*, not the diff. A
  refactor that a user cannot see takes `kind: internal`. CI fails a PR that
  touches the shipped surface (`crates/*/src/`, the UI assets, `assets/`)
  without one. Kinds, length limits, `headline:` and `topic:` are in
  [changelog.d/README.md](./changelog.d/README.md). Check with
  `cargo run -q -p xtask -- changelog --check`. `CHANGELOG.md` and
  `changelog.json` belong to the `changelog` xtask: edit only fragments.
- **Cross-platform, always.** CI builds and tests on **Windows, Linux and
  macOS**. Make no POSIX-only assumptions. Test children are never shell
  scripts: subprocess and PTY behavior is tested against a Rust helper binary
  (docs/TESTING.md → *Platform traps*).
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
- **Text a user reads never cites an ADR.** An ADR number or a `docs/adr` path
  means something only to a developer of Ralphy. This covers UI text, `--help`
  (clap prints the `///` doc comments of the CLI structs), log lines, error
  messages, comments posted to GitHub, and files Ralphy generates. Cite the
  ADR in a `//` comment next to the code instead. CI checks Rust string
  literals, `--help` text, and UI text for this.
- **Tests live next to the code they test, inside `#[cfg(test)]`,** not in a
  parallel source tree. Unit tests stay in the same crate: an inline
  `#[cfg(test)] mod tests`, or, after a split, a sibling file such as
  `foo/tests.rs`. Integration tests (public API only) go in the crate's
  `tests/`, with data in `tests/fixtures/`. A **test helper child binary** goes
  in `src/bin/<name>_test_child.rs`, because `CARGO_BIN_EXE_*` is only visible
  to integration tests (docs/TESTING.md → *Platform traps*).
- **Every new test is seen red, alone.** Before you commit it, apply one
  mutation to the production code, watch the test fail, revert, and write the
  mutation in the commit message. If another test already fails under that
  mutation, extend that test instead of adding one. The rules are in
  [docs/TESTING.md](./docs/TESTING.md).
- **Make the smallest change that fits the existing crate boundaries.** A new
  trait, generic, crate, or layer of indirection needs a real second caller or
  an ADR that decides it — never "for flexibility" (`anti-over-abstraction`).
- **English is the written language of the repo.** ADRs, docs, GitHub issues
  and PRs, commit messages, and code comments are in English, whatever language
  the conversation is in. Issues matter most: an agent executes them, and they
  quote English ADRs, identifiers, and paths, so an issue in another language
  mixes two languages in every sentence.
- **Plain English, literal sentences.** Everything written in this repo is read
  by people for whom English is a second language: UI text, changelog, docs,
  ADRs, issues, commits, and comments. Use common words and short, direct,
  literal sentences. Idioms and figures of speech (*reads at a glance*, *for
  free*, *out from under*) are the thing to replace. Technical terms from
  [CONTEXT.md](./CONTEXT.md) are fine; slang is not. UI text goes further: no
  tool jargon either (*scrollback* → "the text in this console"), by
  [ADR-0065](./docs/adr/0065-the-workbench-written-voice.md) §10.
- **Commit on the current branch.** Create a branch, push, or open a PR only
  when someone asks you to. A human reviews and merges.

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
- **Signatures.** A fixed set of values or a domain identity is an `enum` or
  newtype, not a `String` — this is how the CONTEXT.md vocabulary shows up in
  the types (`anti-stringly-typed`).
- **Async (daemon only).** Never hold a lock guard across an `.await`. Use
  `tokio::sync` primitives and drop the guard first (`anti-lock-across-await`).

## Where things live

- `assets/prompts` — the plan and execute charters. They run in **every
  project** Ralphy works on, so a rule only for Ralphy's own code goes in this
  file, not there. Plan prompts are generated from `assets/prompts/plan/` (see
  its README).
