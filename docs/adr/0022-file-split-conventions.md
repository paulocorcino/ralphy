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

```text
src/
  init.rs        # module root: `mod` declarations + re-exports, thin
  init/
    diagnosis.rs
    scaffold.rs
    labels.rs
```

`mod.rs` is **not** used: a tree full of identically-named
`mod.rs` tabs is hostile to editors and to grep. The two older
`mod.rs` directories (`ralphy-cli/src/events/`, `ralphy-cli/src/telegram/`)
have migrated to `events.rs` / `telegram.rs`. A `git mv foo/mod.rs foo.rs` was
the whole migration.

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

*Replaced by §9 (2026-11-02) once the gate of §9 is on.*

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
   *Replaced by §9 (2026-11-02) once the gate of §9 is on.*
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

### 8. The width of a line of Rust is a gate (amendment, 2026-10-30)

The limit of §7 counts lines, so code joined onto one long line passes it,
and a file of 500 lines can still be very large. `cargo fmt` in the gate
wraps Rust code at 100 characters, the default `max_width` of rustfmt. It
does not wrap a string literal or a comment: the options that would
(`error_on_line_overflow`, `format_strings`, `wrap_comments`) exist only
in nightly rustfmt, and the gate runs stable.

1. **The limit.** A line of a `.rs` file under `crates/` may have 100
   characters, the same number as rustfmt's `max_width`. Characters are
   counted, not bytes, so an accented letter counts once. Every line
   counts: code, comments, string literals and test code.
2. **The base branch is the baseline, as in §7.3.** A pull request fails
   when a file has more lines over 100 characters than on the base branch.
   An old wide line may stay or go; a new one fails.
3. **How a change fits.** Wrap the comment. Continue a long string on the
   next line with `\` or `concat!`. Move long test data, such as captured
   output of a tool, to `tests/fixtures/`.
4. **Rust only, for now.** The workbench TypeScript gets a formatter
   (issue #806). HTML is not measured.
   *Replaced by §9 (2026-11-02) once the gate of §9 is on.*

### 9. Every line counts (amendment, 2026-11-02)

The count of §7 leaves out comment lines, blank lines and the inline test
module, so files grew to 900 lines while the gate passed: `agent_state.rs` has
921 lines and `restart.rs` has 971. An agent reads the whole file, comments and
tests included, so the count must see the whole file.

1. **The count.** A source file has at most 500 lines, every line counted: the
   number `wc -l` shows (code, comments, blank lines, test code, and an inline
   `#[cfg(test)]` module). This replaces the counting rule of the §5 amendment
   and of §7 item 1, and §6: a file of 500 lines cannot hold a longer test
   module.
2. **The width.** §8 covers `.ts` files too: a TypeScript line has at most 100
   characters. oxfmt formats TypeScript at `printWidth: 100`, so its wide lines
   are strings and comments. HTML is measured for size, not for width: its long
   lines are Alpine attributes, and no formatter checks them.
3. **Fix all, then a plain rule.** The files over the new rule are fixed first,
   in one pull request per area (issue #853). Meanwhile
   `cargo run -p xtask -- oversized --strict` lists them and never fails, and
   §7 and §8 hold as written. Then the gate holds the new rule with no base
   branch: any file over 500 lines, or any Rust or TypeScript line over 100
   characters, fails, on every push. §7 item 3 and §8 item 2 end then.

Measured on `9c360eb7` with every line counted: 134 files are over 500 lines
(107 source files and 27 test files), with 14,747 lines above the limit. 659
Rust and TypeScript lines are over 100 characters, in 273 files. By size: 70
files have 501 to 600 lines, 59 have 601 to 800, and 5 have 801 to 1,000.

`cargo run -p xtask -- oversized`, which holds §7, holds this rule too,
in the same CI job. Measured on `9fd3c9c1`: 591 lines of Rust are wider
than 100 characters, in 247 files. Almost all of them are string literals
or comments.

In force since 2026-11-02, when issue #853 closed.

## Consequences

- The series is N small, reviewable, behaviour-preserving PRs that a human
  merges by hand, each independently revertable.
- Reviewers judge a split PR against this ADR: right layout, unchanged public
  API, tests carried along, the CI gate of §4 green, no new abstraction.
- "Convention divergence between PRs" is designed out — the failure mode this
  anchor exists to prevent.
- The legacy `mod.rs` dirs were the one accepted inconsistency; both have
  migrated (`events.rs`, `telegram.rs`), and no `mod.rs` remains.

## Compliance

- §7: replaced by §9.
- §8: replaced by §9.
- §9: checked by `cargo run -p xtask -- oversized --check`, which the
  `oversized` job runs on every push; tested in
  `crates/xtask/src/oversized/tests.rs`.

- §1: not checked by code: reviewed in the PR. No `mod.rs` remains.
- §2: not checked by code: reviewed in the PR.
- §3: not checked by code: reviewed in the PR.
- §4: checked by the CI gate in `.github/workflows/ci.yml` (`cargo fmt --all
  --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo nextest run --workspace`, `cargo test --workspace --doc`). The
  `/rust-skills` review is not checked by code: it is reviewed in the PR.
- §5: not checked by code: reviewed in the PR.
- §6: replaced by §9.
