# AGENTS.md

Operational guide for agents working **on Ralphy's own codebase**. It holds the
rules an agent gets wrong without being told, and it points to the documents
that hold the details. When a rule here and its source document disagree, the
source document is correct — fix this file in the same change. For the gate,
the source is `.github/workflows/ci.yml`, and it wins over any ADR or doc.

- **[docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md)** — the architecture map:
  the owner of each fact, who may call whom, and where an outside product or
  platform may be used. Its §1 names the section each kind of change needs;
  read that section, not the whole file. Read its fact index (§7) before you
  add an outside call
  (`gh`, an HTTP API, a vendor CLI), a new path between browser, daemon and
  CLI, a watcher or timer, a new panel or other place in the workbench that
  shows a fact, or a second computation of a fact the product already knows.
  Get the fact from its owner. Its §4 is the map of crates.
- **[ADR-0072](./docs/adr/0072-the-security-model.md)** — the security
  model: trust zones and the premises every change keeps. Read it before you
  add a vendor CLI flag, an environment variable passed to a child, a secret,
  a setting that turns a protection off, or a vendored library.
- **[CONTEXT.md](./CONTEXT.md)** — the ubiquitous language. Every domain term
  (run, queue label, adapter, planner/executor, event sink…) is defined there.
  Use these words and only these words. Before you name a new type, module,
  flag, or term a user sees, search it for the word: each term starts a line
  as `**Term**:`. Read the whole file only to learn the domain.
- **[docs/adr/](./docs/adr/)** — architecture decisions. Read the relevant ADR
  before you change or add a boundary between crates. A boundary that no ADR
  covers needs an ADR before any code, and the change is probably in the wrong
  place. A new ADR starts from [docs/adr/TEMPLATE.md](./docs/adr/TEMPLATE.md).
- **[docs/BUILDING.md](./docs/BUILDING.md)** — build, CI workflows, releases.
  Read it before you change a workflow, a build profile, or the release.
- **[docs/TESTING.md](./docs/TESTING.md)** — how to write a test that fails
  only when the behavior breaks. Read it before you add, change, or review a
  test, or before a browser check of the workbench page.

## Architecture — ports & adapters, ubiquitous-language-first

Ralphy is **hexagonal (ports & adapters)** at the crate boundary. It uses DDD
only in the **tactical** sense: the [CONTEXT.md](./CONTEXT.md) glossary *is* the
ubiquitous language, and each crate is roughly one bounded context.

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
  npx -y -p typescript@5.9.3 tsc --noEmit -p crates/ralphy-daemon/assets/ui
  npx -y oxlint@1.85.0 --deny-warnings crates/ralphy-daemon/assets/ui crates/ralphy-daemon/ui-tests
  cargo run -q -p xtask -- ui-copy --check
  ```

  New workbench code is a `.ts` module, never a new `.js` file
  ([ADR-0075](./docs/adr/0075-the-workbench-script-is-written-in-typescript.md)).
  Import a type with `import type`: the build removes types without
  knowing which imports are types.
  A new `*.test.mjs` file runs only when `ui-tests/index.mjs` imports it.
  Before a change there, read
  [docs/WORKBENCH-BUILD-GUIDE.md](./docs/WORKBENCH-BUILD-GUIDE.md): vendored
  libraries, touch rules, the clipboard contract.

  **On a pull request, CI also fails on a new duplicated block.** The local
  form, with the settings in `.jscpd.json`:

  ```sh
  npx -y jscpd@5.4.0 --baseline-from-ref main --fail-on-new-clones 0 crates
  ```

  A clone is new when the base branch does not have it, and an edit inside
  an old clone makes it new too. When the two copies are one rule, merge
  them. When they are two rules, put the copy between
  `// jscpd:ignore-start` and `// jscpd:ignore-end`, with a comment that
  says why they change for different reasons.
- **A change to the shipped surface needs a changelog fragment:** one file per
  PR, `changelog.d/<pr-or-issue-number>.md`, or a short name for the work when
  there is no number: one short sentence that names the *capability*, not
  the diff. A refactor, and a fix to a feature that no release has shipped
  yet, take `kind: internal`. CI fails a PR that
  touches the shipped surface (`crates/*/src/`, the UI assets, `assets/`)
  without one. Kinds, length limits, `headline:` and `topic:` are in
  [changelog.d/README.md](./changelog.d/README.md). Check with
  `cargo run -q -p xtask -- changelog --check`. `CHANGELOG.md` and
  `changelog.json` belong to the `changelog` xtask: edit only fragments.
- **Cross-platform, always.** CI builds and tests on **Windows, Linux and
  macOS**. Build paths with `std::path`, spawn a program directly with its
  arguments, and take temporary directories from `tempfile`. Subprocess and
  PTY behavior is tested against a Rust helper binary, never a shell script
  (docs/TESTING.md → *What may be faked*). Measured platform differences are
  in [docs/TESTING-TRAPS.md](./docs/TESTING-TRAPS.md).
- **The public crate API is stable by default.** Moving code inside a crate must
  not change the `pub` surface or its import paths; re-export from the parent
  module. A change to the public API is a design decision, not a side effect.
- **File size is measured by tools, not by you.** Write the code, then run
  the gate. An inline `#[cfg(test)] mod tests { … }` block over 500 lines
  fails an `xtask` test; its message says how to move it, and the move is
  part of your change. Production code over 500 lines of code is a
  recommendation, not a gate: `cargo run -q -p xtask -- oversized` lists those
  files. If it lists a file your change touches, keep working, and write in
  your final report the file and the responsibilities you see that a split
  could follow; a human decides when to split it, by
  [ADR-0022](./docs/adr/0022-file-split-conventions.md).
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
  to integration tests (docs/TESTING.md → *What may be faked*).
- **Every new test is seen red, alone.** Before you commit it, apply one
  mutation to the production code, watch the test fail, revert, and write the
  mutation in the commit body as `red: <file> <mutation> fails <test>`. If
  another test already fails under that mutation, extend that test, or pick a
  mutation only the new test catches. The rules are in
  [docs/TESTING.md](./docs/TESTING.md).
- **Make the smallest change that fits the existing crate boundaries.** A new
  trait, generic, crate, or layer of indirection needs a real second caller or
  an ADR that decides it — never "for flexibility" (`anti-over-abstraction`).
- **Duplication: merge one rule, keep two rules apart.** DRY is about
  knowledge, not text. Before two pieces of code become one helper, ask if
  they change for the *same reason*. An age limit and a stock quantity can
  both be "an integer ≥ 0" today, but they are two rules: they stay two
  functions. One rule written in two places (a limit, a format, a path rule)
  becomes one function or constant, even inside one crate.
- **Your task sets the scope of your fixes.** The task is what you were asked
  to do, plus any extra work the person asking names. Fix a code defect inside
  the code your change edits; the Rust baseline below applies there. For a
  code defect you see elsewhere, write in your final report its `file:line`,
  what is wrong, and the rule it breaks; a human decides whether it becomes an
  issue. A ratchet (docs/ARCHITECTURE.md §8) is part of your task whenever
  your change moves its count.
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
`/rust-skills <name>` shows the bad and good examples. Clippy in the gate
already denies a `std::sync` lock guard held across an `.await`.

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

## Where things live

- `assets/prompts` — the plan and execute charters. They run in **every
  project** Ralphy works on, so a rule only for Ralphy's own code goes in this
  file, not there. Plan prompts are generated from `assets/prompts/plan/` (see
  its README).
