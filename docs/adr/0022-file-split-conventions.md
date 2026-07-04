# File-split conventions: guardrails for the >500-line refactor series

Status: accepted (convention interview 2026-07-04). Anchor for the series of
refactoring PRs that split source files over 500 lines. Changes no production
code — it exists so that N independent split PRs converge on one style instead
of diverging into spaghetti. Each split issue links here; each split PR is
judged against it.

The files in scope at the time of writing (largest first):
`ralphy-cli/src/init.rs` (3418), `ralphy-core/tests/queue.rs` (3136),
`ralphy-agent-claude/src/lib.rs` (2839), `ralphy-core/src/runner.rs` (2791),
`ralphy-cli/src/ui.rs` (2307), and the rest of the `> 500`-line set.

## Decision

### 1. Submodule layout: `foo.rs` + `foo/`, never `foo/mod.rs`

When `foo.rs` grows a directory of submodules, the module root stays a sibling
file `foo.rs` and the submodules live under `foo/` — the Rust 2018 path style,
no `mod.rs`:

```
src/
  init.rs        # module root: `mod` declarations + re-exports, thin
  init/
    diagnosis.rs
    scaffold.rs
    labels.rs
```

`mod.rs` is **not** used for new splits: a tree full of identically-named
`mod.rs` tabs is hostile to editors and to grep. The two pre-existing
`mod.rs` directories (`ralphy-cli/src/events/mod.rs`,
`ralphy-cli/src/telegram/mod.rs`) migrate to `events.rs` / `telegram.rs`
**opportunistically** — when a split PR already touches that module — not as a
forced standalone task. A `git mv foo/mod.rs foo.rs` is the whole migration.

### 2. Public API is unchanged; splits are internal reorganization

A split is a pure internal move. The crate's public surface (every `pub` item
reachable from outside the crate, and the import paths callers use) does **not**
change. When code moves into a submodule, the parent re-exports it so existing
paths keep resolving:

```rust
// init.rs
mod diagnosis;
pub use diagnosis::{diagnose_repo, RepoDiagnosis};   // path stays `crate::init::diagnose_repo`
```

If a PR cannot preserve a path without an API change, that is a design change,
not a split — it belongs in its own issue, not smuggled into a split PR.

### 3. Tests migrate with the code they exercise

`#[cfg(test)]` modules move together with the code under test. A split must not
leave an orphaned test module in the parent file testing code that now lives in
a child — the tests go into (or alongside) the child module. Integration tests
under `tests/` follow the same rule: a `tests/queue.rs` split groups cases by
the behaviour they cover, and the
[subprocess/PTY helper-bin convention](../../CONTEXT.md#testing-conventions)
still holds for any child-process cases.

### 4. Per-PR gate — the "no regression" definition

Every split PR is green only when all three pass, and the PR description says
so:

- **`/rust-skills`** run over the diff (ownership, error handling, the
  anti-anti-patterns pass) — the split must not degrade any of these.
- **`cargo test`** green (workspace, or at least every crate the PR touches).
- **`cargo clippy`** green with no new warnings.

Because the public API is unchanged (§2), a red `cargo test` after a split is a
mechanical mistake in the move, not an expected behavioural delta — it is the
signal that the split broke something.

### 5. Anti-overengineering: split by existing responsibility only

Split along responsibility seams that **already exist** in the file. Do not
invent traits, generics, or indirection layers whose only purpose is to justify
a file boundary. If a clean seam is not already present, the file is a
design-refactor candidate (its own issue), not a mechanical split. A split that
adds abstraction is out of scope for this series.

## Consequences

- The series is N small, reviewable, behaviour-preserving PRs that a human
  merges by hand, each independently revertable.
- Reviewers judge a split PR against this ADR: right layout, unchanged public
  API, tests carried along, three-way gate green, no new abstraction.
- "Convention divergence between PRs" is designed out — the failure mode this
  anchor exists to prevent.
- The two legacy `mod.rs` dirs are the one accepted inconsistency until a PR
  naturally retires them; §1 records the intent so it is not mistaken for a
  counter-example.
