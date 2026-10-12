# Function complexity is a gate

Status: accepted
Amended 2026-10-12 (Rust is scored from the source text, not by clippy).
Amended 2026-10-12 (a ceiling on the baseline goes down to 15).
Kind: process
Protects: integrity of change, testability

## Context

The size gate of ADR-0022 §7 measures a file. It does not measure a
function. A file under its limit can still hold one function with many
branches, and nothing in the gate stops that function from growing. Most of
Ralphy's code is written by agents. An agent tends to add one more branch to
the function in front of it instead of splitting the function.

Measured on `70c46c78` (2026-10-12), limit 10, production code only:

| Language | Measure | Tool | Functions over 10 | 11–20 | 21–50 | 51 or more |
|---|---|---|---|---|---|---|
| Rust | cognitive complexity | `clippy::cognitive_complexity`, clippy 0.1.98 | 122 | 81 | 34 | 7 |
| TypeScript | cyclomatic complexity, `variant: "modified"` | oxlint 1.85.0 `eslint(complexity)` | 114, in 61 of 108 modules | 91 | 23 | 0 |

The largest are in Rust: `runner/phases/verify.rs` (99), `runner.rs` (81),
`routes/ws_command.rs` (77) and `routes/ws_tree.rs` (76). `runner.rs` has 10
fix commits; the median source file has 1.

With the `classic` variant, every `case` of a `switch` adds 1, so the
exhaustive `switch` with a `never` default that AGENTS.md requires counts as
complex. A `switch` over 12 literals scores 13 with `classic` and 2 with
`modified` (oxlint 1.85.0, measured). In Rust, cognitive complexity counts a
whole `match` as 1, so an exhaustive `match` has the same treatment.

## Decision

**D1. Two measures, from tools the gate already runs.** Rust functions are
measured by `clippy::cognitive_complexity`. TypeScript functions are
measured by oxlint's `complexity` rule with `variant: "modified"`. No other
complexity tool is installed.

**D2. The limit is 10 in both languages.** A function over 10 is new debt
unless D4 records it.

**D3. Production code only.** Clippy runs without `--all-targets`, so
`#[cfg(test)]` code, `tests.rs` and `tests/` are not measured. oxlint
measures `crates/ralphy-daemon/assets/ui` without `vendor/`, and does not
measure `ui-tests/`.

**D4. A committed baseline is the ratchet.** `docs/complexity-baseline.json`
lists every function over the limit by path, function name, and value. A
function is identified by its path with `/` separators and its name. A Rust
closure and a TypeScript function with no name are named `(anonymous)`. When
a file has several functions of one name, their values are compared as a
list sorted highest first. A line number is not part of the identity. The
entries are sorted, so every platform writes the same file. The check fails
when:

1. a function over 10 has no entry in the baseline;
2. a function's value is higher than its entry;
3. a function's value is lower than its entry, or its entry names a function
   that is no longer over 10, and the baseline was not updated.

Rule 3 makes the ratchet tighten. A renamed or moved function is a new
function, so it must be 10 or less.

**D5. One command.** `cargo run -p xtask -- complexity --check` runs both
tools and applies D4. `--bless` rewrites the baseline from the current code.
A change uses `--bless` only to record a value that went down; a value that
went up has already failed rule 2. Locally the command runs oxlint through
`npx`; `--oxlint <program>` replaces it, and CI passes the binary it pins
by checksum.

**D6. The near-limit list is printed, and does not fail.** With `--base
<rev>`, the command also lists the functions that score 7 to 10 in the
files changed since `<rev>`. This is the warning for the agent that runs
the gate locally. It does not fail CI.

**D7. The tools are pinned in the step.** The CI step pins its Rust
toolchain to an exact version instead of `stable`, and oxlint keeps its
version and checksum pin. A new tool version changes the score only in the
commit that raises the pin, and that commit runs `--bless`.

**D8. AGENTS.md states the behavior, not a number.** It says that function
complexity is measured by the tool, names the command, and says how to split
a function that fails: along a responsibility it already has, in its own
commit that changes no behavior (ADR-0022 §7.4). It gives no target below
the limit; D6 is that target.

**D9. The functions at 51 or more become issues.** The ratchet stops growth
but never forces a fix. The seven Rust functions at 51 or more each get an
issue when this ADR is accepted.

## Consequences

- The `complexity` job runs clippy a second time, with its own pinned
  toolchain and cache. The `lint` job keeps `stable`.
- A change that touches a function on the baseline and makes it larger fails
  even when the new code is correct. The split is part of that change, by
  ADR-0022 §7.4.
- A rename of a function on the baseline fails D4 rule 1 unless the function
  is split to 10 or less in the same change.
- `clippy::cognitive_complexity` is in clippy's `nursery` group. The clippy
  team says its score is not precise. D7 keeps the score stable between
  pins; it does not make it precise.

## Considered options

- **rust-code-analysis (Mozilla).** One tool for both languages, and it
  measures without a build. Its last release is 0.0.25 (2023-01-13); a
  recent version means a git install that builds a dozen tree-sitter
  grammars in C. Its grammars are pinned and older than the compiler, so new
  syntax can fail to parse and a function drops out of the measure without
  an error.
- **lizard.** One cyclomatic measure for both languages, but a new Python
  tool in CI and a token-based parser whose accuracy on Rust was not
  measured.
- **Compare with the base branch, as `oversized` does.** `oversized` reads
  text with `git show`. A complexity score from clippy needs a build of the
  base branch, which doubles the time of the job. The committed baseline
  needs no second build, and the reviewer sees each change of value in the
  diff.
- **A fixed limit with no baseline.** It fails on 236 functions on the first
  day.
- **A lower number in AGENTS.md (for example 6).** An agent cannot measure
  the score while it writes, so the rule would be a guess. The likely result
  is functions split into small helpers before any need. D6 gives the same
  warning from the tool.

## Compliance

- D1, D2, D3, D4: checked by `cargo run -p xtask -- complexity --check` in
  the `complexity` job of `.github/workflows/ci.yml`. Its rules are tested
  in `crates/xtask/src/complexity/tests.rs`.
- D5, D6: checked by the tests in `crates/xtask/src/complexity/tests.rs`.
- D7 (as amended): checked by `ci_installs_the_pinned_oxlint`
  (`crates/xtask/src/complexity/tests.rs`): `ci.yml` pins the oxlint
  version that the xtask names.
- Amendment A1, A3: the scoring rules and the module walk are tested in
  `crates/xtask/src/complexity/cognitive/tests.rs`.
- D8: not checked by code: reviewed in the PR that changes AGENTS.md.
- D9 (as amended by A4–A7): A5 is checked by `complexity --check`, and its
  rule is tested in `crates/xtask/src/complexity/tests.rs`. The issue of
  each step (A6) is opened by hand.

## Amendment 2026-10-12 — Rust is scored from the source text

The first CI run of the gate (PR #710) failed on Linux with a baseline
written on Windows. Clippy scores only the code that `#[cfg]` keeps on the
machine it runs on: `serve` in `ralphy-daemon/src/serve.rs` scored 64 on
Windows and 74 on Linux, and functions under `#[cfg(unix)]` exist only on
Linux. A check for the other targets from one machine does not build: the
build script of `ring` needs a C compiler for the target (measured with
Rust 1.98.0, 2026-10-12). So a baseline from clippy is true on one platform
only.

**A1. Replaces D1 for Rust.** The xtask reads the Rust source with `syn`
and computes the cognitive complexity of the SonarSource specification
("Cognitive Complexity", G. Ann Campbell, 2018). Every `#[cfg]` branch is
read, so a function has one score on every platform. The rules, with the
Rust forms they cover, are in `crates/xtask/src/complexity/cognitive.rs`.
A closure and an `async` block are part of the function around them. The
tokens of a macro are scored when they are expressions separated by commas,
and the arms of `select!` are scored as a `match`; tokens of another form
are not scored. Recursion is not scored.

**A2. Replaces D7.** No Rust toolchain is pinned: the score does not come
from the compiler. oxlint keeps its version and checksum pin.

**A3. Makes D3 exact for Rust.** The production files of a crate are the
modules its roots declare (`src/lib.rs`, `src/main.rs`, each `src/bin/*.rs`
that is not a `*_test_child.rs`), followed through `mod x;` and `#[path]`.
A module or item under `#[cfg(test)]` and a `#[test]` function are left
out. A file no root declares is not measured.

Measured on the rebased branch with `complexity --bless`: 178 Rust
functions over 10 (119 at 11–20, 47 at 21–50, 12 at 51 or more) and 114
TypeScript functions. The order changed: `tree_ws` scores 212, because
its `select!` sits inside a `loop` and every level of nesting costs more;
`verify_gate`, at 99 with clippy, scores 33. D9 now covers the twelve
functions at 51 or more. The `complexity` job runs in seconds instead of
running clippy a second time.

## Amendment 2026-10-12 — a ceiling on the baseline goes down to 15

D9 opens issues for the functions at 51 or more, but nothing stops the work
there, and nothing says where it ends. The baseline on `50f1be4a` lists 292
functions over 10:

| Score | Rust | TypeScript | Total |
|---|---|---|---|
| 51 or more | 12 | 0 | 12 |
| 41–50 | 3 | 1 | 4 |
| 31–40 | 16 | 6 | 22 |
| 21–30 | 28 | 16 | 44 |
| 16–20 | 40 | 29 | 69 |
| 11–15 | 79 | 62 | 141 |

**A4. Replaces D9. The target for code on the baseline is 15, in both
languages.** 15 is the default limit of the SonarSource rule for cognitive
complexity. A function at 11 to 15 stays on the baseline. It goes down to 10
when a change edits it for another reason. D2 does not change: a function
that is not on the baseline must be 10 or less.

**A5. The baseline has a ceiling.** `docs/complexity-baseline.json` records
one ceiling for both languages. The check fails when a function on the
baseline scores more than the ceiling. This is a fourth rule of D4. The
ceiling only goes down, as the values of D4 do.

**A6. The ceiling goes down in four steps, when the work of a step is
done.** Each step is one issue, cut into sub-issues by file:

| Step | Functions | Ceiling when the step closes |
|---|---|---|
| 1 | the 12 at 51 or more (#709) | 50 |
| 2 | the 26 at 31–50 | 30 |
| 3 | the 44 at 21–30 | 20 |
| 4 | the 69 at 16–20 | 15 |

The commit that closes a step lowers the ceiling and runs `--bless`. A step
has no date. A date would push a function into helpers that follow no
responsibility, and ADR-0022 §7.4 asks for a split along a responsibility the
function already has.

**A7. The ceiling is the same number for two measures.** The Rust score is
cognitive complexity and the TypeScript score is modified cyclomatic
complexity, so 15 in one language is not 15 in the other. One number keeps
the rule simple. The measured TypeScript maximum is 43, so TypeScript enters
the work at step 2.
