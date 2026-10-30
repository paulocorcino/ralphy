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
[subprocess/PTY helper-bin convention](../TESTING.md#what-may-be-faked-item-2)
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

*Amendment (2026-09-30).* The two commands above are no longer the gate. The
gate of every change, a split included, is the set of commands CI runs, as
defined in `.github/workflows/ci.yml` and listed in AGENTS.md: `cargo fmt`,
`cargo clippy --all-targets`, `cargo nextest run` and `cargo test --doc`. The
`/rust-skills` review of the diff still applies to a split.

### 5. Anti-overengineering: split by existing responsibility only

Split along responsibility seams that **already exist** in the file. Do not
invent traits, generics, or indirection layers whose only purpose is to justify
a file boundary. If a clean seam is not already present, the file is a
design-refactor candidate (its own issue), not a mechanical split. A split that
adds abstraction is out of scope for this series.

*Amendment (2026-10-03).* The 500-line threshold counts lines of code only:
comment and blank lines above the inline test module do not count, so
documenting a file never pushes it over. The threshold is a recommendation,
not a gate: a split is a refactor, and it never blocks a delivery.
`cargo run -p xtask -- oversized` lists the files over it. An agent whose
change touches a listed file reports it with the responsibilities a split
could follow, and a human decides when to split, often as its own issue.
Measured on the day: 22 of the 27 files over 500 production lines were under
500 lines of code. *Replaced by §7 (2026-10-12): the threshold is a gate.*

### 6. The inline-test budget (amendment, 2026-09-23)

The >500-line threshold counts production lines, so it never fires on a file
whose bulk is its inline `#[cfg(test)] mod tests { … }` — and such a module
grows one appended test at a time, each commit reasonably matching the file it
found. `ralphy-daemon/src/lib.rs` reached 178 production lines under an
8,941-line test module that way. So an inline test module has its own budget:
**over 500 lines, it moves to a sibling file** — `foo/tests.rs` next to
`foo.rs`, or `src/tests.rs` for a crate root — as a pure move that touches no
production code. The budget is enforced, not remembered:
`crates/xtask/tests/inline_test_modules.rs` fails CI naming each module over it.

### 7. The size limit is a gate (amendment, 2026-10-12)

The line ratchets of ADR-0073 D8 held `app.ts` and `wb-console.ts` while
they were cut, and they did their work: `app.ts` went from 7,155 to 339
lines, and `wb-console.ts` from 6,789 to 488. After the cut, an exact count
on two small files only makes a change put its code in another file to keep
the count the same. The recommendation of §5 had the opposite problem:
nothing stopped a file from growing, so each split waited for a human.

1. **The limits.** A Rust file may have 500 lines of code, and a TypeScript
   file 700 (the module size of ADR-0073, its 2026-10-11 amendment). Lines
   of code are counted as in §5: comment lines, blank lines and the inline
   test module (it has its own budget, §6) do not count. A line that starts
   a `/* … */` block, and each line inside the block, is a comment too.
2. **Every source file.** Every `.rs` and `.ts` file under `crates/` is
   measured, test code included: `tests/`, `tests.rs`, test child binaries
   and `.d.ts` files. `vendor/`, `node_modules/` and `target/` are not.
3. **The base branch is the baseline.** A pull request fails when a file
   over its limit has more lines of code than on the base branch. So a new
   file over the limit fails, a file that grows past the limit fails, and an
   old large file that grows fails. An old large file may keep its size or
   shrink. No list of large files is kept by hand, so there is no number for
   a change to raise. A file moved to a new path is a new file there.
4. **Build first; the gate decides when to split.** A change is not planned
   around the limit. When the gate fails, the split is part of the change,
   not a change of scope: along a responsibility the file already has (§5),
   in its own commit that changes no behavior.
5. **What it replaces.** The recommendation of the §5 amendment, D8 of
   ADR-0073, and decision 3 of its 2026-10-11 amendment: the exact ratchets
   `APP_TS_LINES`, `WB_CONSOLE_TS_LINES`, `WB_NOTES_TS_LINES` and
   `TS_SIZE_BASELINE` are gone.

Measured on `b199d7e6` with `cargo run -p xtask -- oversized`: 29 files are
over their limit, and 25 of them are test files. The largest are
`ralphy-daemon/src/tests.rs` (7,265 lines of code) and `wb-file-viewer.ts`
(1,231). A test added to one of these files fails the gate, so it goes into
a new file, or the old file is split first.

The count does not see code that is joined onto fewer lines. `cargo fmt` in
the gate stops this in Rust. The TypeScript has no formatter in the gate, so
joined lines there are found in review.

*Amendment (2026-10-30): one limit of 500 for every language.* A
TypeScript file may have 500 lines of code, as a Rust or HTML file does;
the 700 of item 1 is gone. One limit is one rule to remember, and a
workbench module of 500 lines is still one an agent reads whole. The
rest of §7 does not change, so an old large file keeps its size and may
not grow. Measured on `86f5b3b1` with `cargo run -p xtask -- oversized`:
eight workbench modules are over the new limit, from `wb-files.ts` (687
lines of code) to `wb-dbml.ts` (516).

## Consequences

- The series is N small, reviewable, behaviour-preserving PRs that a human
  merges by hand, each independently revertable.
- Reviewers judge a split PR against this ADR: right layout, unchanged public
  API, tests carried along, the CI gate of §4 green, no new abstraction.
- "Convention divergence between PRs" is designed out — the failure mode this
  anchor exists to prevent.
- The legacy `mod.rs` dirs are the one accepted inconsistency until a PR
  naturally retires them; §1 records the intent so it is not mistaken for a
  counter-example. `telegram.rs` has migrated; `events/mod.rs` remains.

## Compliance

- §7: checked by `cargo run -p xtask -- oversized --base <rev>`, which the
  `oversized` job in `.github/workflows/ci.yml` runs on each pull request
  against its base branch. With no argument, the command lists the files
  over their limit. Its rules are tested in `crates/xtask/src/oversized.rs`.

- §1: not checked by code: reviewed in the PR. One legacy `mod.rs` remains
  (`crates/ralphy-cli/src/events/mod.rs`).
- §2: not checked by code: reviewed in the PR.
- §3: not checked by code: reviewed in the PR.
- §4: checked by the CI gate in `.github/workflows/ci.yml` (`cargo fmt --all
  --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo nextest run --workspace`, `cargo test --workspace --doc`). The
  `/rust-skills` review is not checked by code: it is reviewed in the PR.
- §5: not checked by code: reviewed in the PR.
- §6: checked by `crates/xtask/tests/inline_test_modules.rs`
  (no_inline_test_module_outgrows_the_budget).
