# Function complexity is a gate

Status: proposed
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
lists every function over the limit as path, function name, and value. A
function is identified by its path with `/` separators, its name, and its
position among the functions of the same name in that file. A line number is
not part of the identity. The entries are sorted, so every platform writes
the same file. The check fails when:

1. a function over 10 has no entry in the baseline;
2. a function's value is higher than its entry;
3. a function's value is lower than its entry, or its entry names a function
   that is no longer over 10, and the baseline was not updated.

Rule 3 makes the ratchet tighten. A renamed or moved function is a new
function, so it must be 10 or less.

**D5. One command.** `cargo run -p xtask -- complexity --check` runs both
tools and applies D4. `--bless` rewrites the baseline from the current code.
A change uses `--bless` only to record a value that went down; a value that
went up has already failed rule 2.

**D6. The near-limit list is printed, and does not fail.** The command also
lists each function the current change touches that scores 7 to 10. This is
the warning for the agent that runs the gate locally. It does not fail CI.

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

- The `lint` job runs clippy a second time with other lint flags. The
  dependencies stay cached; the workspace crates are checked again.
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
  the `lint` job of `.github/workflows/ci.yml`. Its rules are tested in
  `crates/xtask/src/complexity.rs`.
- D5, D6: checked by the tests of `crates/xtask/src/complexity.rs`.
- D7: not checked by code: reviewed in the PR that changes the step.
- D8: not checked by code: reviewed in the PR that changes AGENTS.md.
- D9: not checked by code: the issues are opened by hand.
