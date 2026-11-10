# Lint transition plan

The plan that takes the lint gate from where it is to the target: every rule an
error, pinned tools, one local command that CI mirrors, each exception inline
with its reason, and every count only down. It is read cold: an agent that
works a phase needs only this file, the configs it names, and the scripts in
`docs/lint/scripts`. How the plan was cut (the measure, the debt table, every
row of the references) is in [lint-evidence.md](./lint-evidence.md).

Measured on 13045ccb (2026-11-09): 158 rules with sites, 14,649 sites. The
state is in `docs/lint/lint-record.json` and its history.

## Working this plan

- Work one phase: `python docs/lint/scripts/progress.py show docs/lint/lint-record.json <phase id> --plan docs/lint/lint-transition.md` prints its section with its state: the phases it waits for, its counts at the last point, the decisions that hold it.
- Record each change of state with its command, which appends to the history: a decision answered, `progress.py decide docs/lint/lint-record.json <Dn> "<answer>" --date <date>`; an issue opened, `progress.py issue docs/lint/lint-record.json <phase or Dn> <url> --date <date>`; a slice landed or a rule flipped, `progress.py note docs/lint/lint-record.json "<what>" --date <date>`. `progress.py log docs/lint/lint-record.json` prints the state.
- The plan's text changes only when the plan changes. `progress.py check docs/lint/lint-record.json --plan docs/lint/lint-transition.md` runs in the gate: it fails on a section missing, a phase heading with no phase in the record or the reverse, and a plan whose last line is not the end marker.
- Measure again: `python docs/lint/scripts/progress.py commands docs/lint/lint-record.json` prints each command of the record. Set `SC` to a scratch folder outside the repo first. Then `progress.py point docs/lint/lint-record.json --run <name> <tally> ... --sha <sha> --date <date> --config Cargo.toml --config .oxlintrc.json --append` records the new counts.

## Target

The end state: every lint rule of the target is an error in the real configs,
with no debt list left; the tools run at pinned versions; `cargo run -q -p
xtask -- gate` runs every check and CI runs the same ones; each remaining
exception is inline, names its rule and says why, and a ratchet counts it; every
count only goes down.

| Knob | End state | Pace | Curve at the end state | Reason (or `default taken`) |
|---|---|---|---|---|
| Function complexity | 10 (Rust: `xtask complexity`, cognitive, from the source text; TS: oxlint `complexity`, modified) | on | 0 over it | ADR-0081; already on |
| Nesting depth | 3 (TS `max-depth`); Rust: cognitive complexity counts nesting | on | 0 | already on |
| Arguments | Rust 5 (`too-many-arguments-threshold`), TS 4 (`max-params`) | Phase 4, Phase 12 | Rust 77 functions at 5 (0 at 6); TS 14 at 4, 1 at 5 | answered in step 3 (D2) |
| File size | 500 lines, every line counted | on | 0 | `xtask oversized`, stricter than the reference (non-blank lines) |
| Line width | 100 | on | 0 | `xtask oversized` |
| Clone size | 75 tokens and 5 lines | Phase 14 (count ratchet) | 41 clones (37 Rust, 4 TS); 6 at 100/10, 225 at 50/5 | answered in step 3 (D2) |
| Inline test block | none set | none | not measured | no reference value |
| Coverage of the diff | none set | none | not measured: needs `cargo llvm-cov` in CI | D12 |
| Optional families | `clippy::pedantic` on with its debt list; anti-slop not taken; mutation testing not taken | on / D14 / D10 | pedantic: 1,542 sites | pedantic: `default taken` |

The target configs as they will be when the last phase closes:

- `Cargo.toml` `[workspace.lints]`: the lints of `docs/lint/lint-drafts/clippy-lints.txt` at `deny` (each `-W` there), the groups `rust_2018_idioms`, `clippy::all` and `clippy::pedantic` at `deny` with `priority = -1`, `must_use_candidate` and `missing_errors_doc` at `allow` (D3), and no `debt:` entry left. Every crate keeps `[lints] workspace = true`.
- `clippy.toml`: `docs/lint/lint-drafts/clippy.toml` (the repo's settings, plus the `allow-*-in-tests` settings and `too-many-arguments-threshold = 5`). `cognitive_complexity` and `cognitive-complexity-threshold` stay out: `xtask complexity` is the repo's scorer, and one tool holds one property (ADR-0081); clippy scored 135 functions over 10 where it finds none. `disallowed-types` stays empty: no type is wrong everywhere in this code.
- `.oxlintrc.json`: `docs/lint/lint-drafts/oxlintrc.json` with the repo's `ignorePatterns`: the categories `correctness`, `suspicious` and `pedantic` at `error`, the plugins `eslint`, `import`, `oxc`, `promise`, `typescript`, `unicorn`, the rules of D4 `off` with their reasons, `curly` at `multi-line` (user, 2026-11-10), and no `debt:` entry left. `no-restricted-properties` keeps its options (`innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write`) and `max-params` its `max: 4`.
- `crates/ralphy-daemon/assets/ui/tsconfig.json`: today's flags plus `exactOptionalPropertyTypes` and `noPropertyAccessFromIndexSignature`.
- `.jscpd.json`: `minTokens: 75`, `minLines: 5`.
- The oxlint steps of the gate run with `--report-unused-disable-directives-severity=error`.

## Decisions for the human

- **D1 — Python.** Python is out of the gate: 130 files, 124 of them the browser checks in `tests/browser` that a person runs by hand. ruff with every rule found 11,690 sites, mypy strict 10,221. Answered 2026-11-09: out, with this reason. Holds nothing.
- **D2 — The knobs of step 3.** Answered 2026-11-09: arguments Rust 5 and TS 4; clones 75 tokens and 5 lines; ast-grep for navigation and the error-message rule. Holds nothing.
- **D3 — The clippy lints off for good.** `must_use_candidate` (fires on every pure function; `#[must_use]` goes where ignoring the value is a bug) and `missing_errors_doc` (docs on the public API of a published library only; no crate here is published). Default taken. To change it, remove the `noise:` entry and measure the lint. Holds nothing.
- **D4 — The oxlint rules off for good.** `typescript/prefer-readonly-parameter-types` (2,820 sites; the Alpine components and stores change their own state), `max-lines-per-function` (complexity is the limit), `max-lines` (file size is `xtask oversized`), `no-underscore-dangle` (850; a leading underscore marks a private field of a component), `import/no-unassigned-import` (the test index imports files for their side effect), `no-inline-comments`, `import/max-dependencies`, `sort-vars`, and `no-negated-condition` (the same check as `unicorn/no-negated-condition`). Default taken. Holds nothing.
- **D5 — Who prints in Rust.** `print_stdout` (275 sites, 211 in `ralphy-cli`), `print_stderr` (37) and `exit` (9). Options: (a) one output module per binary (`ralphy-cli`, `xtask`, each `src/bin/*`) carries the one exception and every other module calls it: the most work, one place to change the output; (b) each binary crate allows the lints at its root with a reason, and library crates keep them denied: little work, a library cannot print; (c) keep both lints off. Recommendation: (b), then (a) for `ralphy-cli` alone. Holds Phase 7.
- **D6 — Who writes HTML and logs in the workbench.** `no-restricted-properties` on `innerHTML` and the like (46 sites) and `no-console` (38). Options: (a) one module that sanitizes HTML (it uses DOMPurify) carries the exception, every other site builds nodes or calls it; one module owns the console; (b) an exception per site with a reason. Recommendation: (a). It also decides the `no-restricted-imports` layers, which the workbench has none of today. Holds Phase 13.
- **D7 — Where `indexing_slicing` applies.** 430 sites (264 production), 125 of them in `xtask`. The reference turns it on in crates that parse outside data. Options: (a) the crates that parse outside data only (the agent adapters, `ralphy-usage-scan`, `ralphy-pricing`, the daemon's routes), by a crate-root `#![deny]` once each is at 0; (b) every crate. Recommendation: (a). Holds the `indexing_slicing` slice of Phase 3 only.
- **D8 — Type-only imports in `wb-types.d.ts`.** `typescript/consistent-type-imports` has 41 sites; 40 are deliberate `import()` types in `wb-types.d.ts`, which the ratchet `peer_reply_pin` reads. Options: (a) the rule turns on and `wb-types.d.ts` carries one exception with its reason; (b) rewrite the file to `import type` and teach `peer_reply_pin` the new form. Recommendation: (a). Holds the `consistent-type-imports` slice of Phase 11 only.
- **D9 — The Markdown rules.** markdownlint finds 4,665 sites in 222 files; 3,491 are `MD013` (line length), which wraps prose. Options: (a) markdownlint with `MD013` off and the rest on; (b) markdownlint fully; (c) no Markdown lint. Recommendation: (a). Holds Phase 15.
- **D10 — Mutation testing on the diff.** `cargo mutants --in-diff` (Rust) and Stryker (TS). It was measured in #809 and closed as not planned. The rule "every new test is seen red" in AGENTS.md is advice until a tool runs it. Options: run it in CI on the diff; keep it advice. Holds nothing.
- **D11 — An agent hook for this repo.** A hook that refuses `--no-verify`, a force push, and an edit nobody asked for to a lint config, a debt list or a ratchet constant, and runs the fast linters on each edited file. The repo has none for its own agents (Ralphy's guard is for the runs it drives). Holds nothing.
- **D12 — Coverage of the diff.** Not measured: it needs `cargo llvm-cov` in CI. Holds nothing.
- **D13 — Commit message format.** No check today. Options: a CI check of the Conventional Commits form the history already uses; none. Holds nothing.
- **D14 — The anti-slop rules for TypeScript.** An optional family of oxlint JS plugin rules, vendored. Not measured. Options: measure it in a phase of its own; leave it. Holds nothing.

## Phases

Phase 0 completes the gate itself; every lane starts after it. The lanes run side
by side; inside a lane the order is fixed. In each lane the phase made by a
command comes first (a mass change, landed when no long branch is open, and
redone by the command instead of merged by hand), and the phase held by a
decision comes last, so the decision holds nothing after it.

- **Rust lane** (`Cargo.toml`, `clippy.toml`, `crates/**/*.rs`): 1, 2, 3, 4, 5, 6, 7, in that order.
- **TypeScript lane** (`.oxlintrc.json`, `tsconfig.json`, `crates/ralphy-daemon/assets/ui`, `crates/ralphy-daemon/ui-tests`): 8, 9, 10, 11, 12, 13, in that order.
- **Clones lane** (`.jscpd.json`, a ratchet test): 14.
- **Text lane** (the files no lint reads yet: YAML, Markdown, HTML, CSS, shell, PowerShell): 15.

Integration is serial: one change lands at a time, rebased on the last. The debt
lists in `Cargo.toml` and `.oxlintrc.json` are the hotspots: take them from the
base branch at each rebase.

In every phase:

- **Measure first**: `python docs/lint/scripts/progress.py commands docs/lint/lint-record.json` prints the measure commands; run the ones of the phase's tool. The code has moved since the plan was written: the slices follow the new count.
- **Flip**: a rule at 0 sites loses its `debt:` entry in the config, in the slice that removed its last site; the rule is then an error. `python docs/lint/gate.py shrink` checks that the list only shrank.
- **Done when** (each phase adds its own): each rule of the phase is an error in the real config, the re-measure shows 0 sites (or only marked exceptions, counted), and `cargo run -q -p xtask -- gate` is green.

### Phase 0 — The gate itself

- **Lane**: gate; waits for nothing.
- **Rules**: no lint rule. The items are distinct tasks, with no tracer:

| Slice | What | Count today |
|---|---|---|
| 0.1 | shellcheck over every tracked `.sh` file, in the gate and in CI (the release binary, pinned by checksum, as actionlint) | 0 sites on 8 files (shellcheck 0.11.0) |
| 0.2 | PSScriptAnalyzer over every tracked `.ps1` file (`Invoke-ScriptAnalyzer -Recurse -EnableExit`): measure, then add | not measured: not installed here |
| 0.3 | A scheduled workflow that compares each pin with its latest stable release (`python docs/lint/scripts/drift.py age`) and opens an issue when one is newer | none today |
| 0.4 | A ratchet on the steps that pass when their tool fails: `python docs/lint/scripts/tally.py pattern '\|\|\s*(true\b\|:\|exit 0)\|continue-on-error:\s*true\|command -v \|set \+e' .github crates/xtask/src/gate --ext .yml,.sh,.rs --expect 5`, each match with its reason beside it | 5 (a `grep` that finds nothing, and `command -v sha256sum` with a fallback) |
| 0.5 | A ratchet on the ignore lists (`.oxlintrc.json` `ignorePatterns`, `.jscpd.json` `ignore`, `_typos.toml`): their size only goes down | none today |
| 0.6 | A test that `ralphy-core` depends on no `ralphy-agent-*` crate and not on `ralphy-adapter-support` (ADR-0002) | 0 |
| 0.7 | gitleaks in the local gate too (CI runs it today) | 0 |

- **How a site is fixed**: each item is a new check; it lands at 0 sites, or its first commit fixes what it finds.
- **Ratchet**: 0.4 and 0.5 are ratchets; their constants only go down.
- **Agent docs**: none: the gate command stays the same.
- **Done when**: each item runs in the gate and in CI, and `the_gate_mirrors_every_ci_step` passes.

### Phase 1 — Rust: fix by command

- **Lane**: Rust; after Phase 0.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `clippy::str_to_string` | 903 | 1449 | fix by command |
| `unreachable_pub` | 480 | 28 | fix by command |
| `clippy::doc_markdown` | 171 | 37 | fix by command |
| `unused_qualifications` | 19 | 186 | fix by command |
| `clippy::assert_is_empty` | 0 | 157 | fix by command |
| `clippy::redundant_closure_for_method_calls` | 86 | 60 | fix by command |
| `clippy::map_unwrap_or` | 65 | 13 | fix by command |
| `clippy::semicolon_if_nothing_returned` | 26 | 13 | fix by command |
| `clippy::borrow_as_ptr` | 28 | 1 | fix by command |
| `clippy::single_match_else` | 19 | 3 | fix by command |
| `clippy::duration_suboptimal_units` | 7 | 14 | fix by command |
| `clippy::cast_lossless` | 7 | 14 | fix by command |
| `clippy::needless_raw_string_hashes` | 0 | 18 | fix by command |
| `clippy::uninlined_format_args` | 9 | 2 | fix by command |
| `clippy::unnested_or_patterns` | 5 | 2 | fix by command |
| `clippy::ignored_unit_patterns` | 5 | 0 | fix by command |
| `clippy::if_not_else` | 5 | 0 | fix by command |
| `clippy::range_plus_one` | 0 | 3 | fix by command |
| `clippy::manual_assert_eq` | 0 | 3 | fix by command |
| `clippy::manual_assert` | 0 | 3 | fix by command |
| `clippy::unchecked_time_subtraction` | 0 | 3 | fix by command |
| `clippy::ptr_as_ptr` | 3 | 0 | fix by command |
| `clippy::manual_is_variant_and` | 2 | 1 | fix by command |
| `clippy::implicit_clone` | 2 | 0 | fix by command |
| `clippy::wildcard_imports` | 2 | 0 | fix by command |
| `clippy::inconsistent_struct_constructor` | 0 | 1 | fix by command |
| `clippy::option_as_ref_cloned` | 1 | 0 | fix by command |
| `clippy::unnecessary_join` | 1 | 0 | fix by command |
| `clippy::filter_map_next` | 1 | 0 | fix by command |
| `clippy::manual_midpoint` | 1 | 0 | fix by command |
| `clippy::explicit_iter_loop` | 1 | 0 | fix by command |
| `clippy::unnecessary_semicolon` | 1 | 0 | fix by command |
| `clippy::enum_glob_use` | 1 | 0 | fix by command |
| `clippy::stable_sort_primitive` | 0 | 1 | fix by command |
| `clippy::manual_string_new` | 0 | 1 | fix by command |
| **35 rules** | **1851** | **2013** | |

- **Slices**: one commit per rule, with its command in the message: `CLIPPY_CONF_DIR=docs/lint/lint-drafts cargo clippy --fix --allow-dirty --workspace --all-targets --all-features -- -A clippy::all -W clippy::<lint>`, then `cargo fmt --all`. `unreachable_pub` and `unused_qualifications` are rustc lints: `-W <lint>` in place of the clippy one. Fixes interact: measure again after each, and take the new sites as the next command's. `map_unwrap_or` has one site the tool cannot fix: fix it by hand in the same slice.
- **How a site is fixed**: Surface: the tool's own command. `unreachable_pub` makes an item `pub(crate)`: read each item the tool changes in a crate's `lib.rs` re-exports, because AGENTS.md keeps the public API stable.
- **Ratchet**: none: each rule leaves the debt list.
- **Agent docs**: none.
- **Done when**: the rules of the table are errors, and no long branch was open when the commits landed.

### Phase 2 — Rust: dropped values

- **Lane**: Rust; after Phase 1.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `clippy::let_underscore_untyped` | 133 | 104 | fix then flip |
| `let_underscore_drop` | 83 | 96 | fix then flip |
| `clippy::mem_forget` | 0 | 38 | fix then flip |
| **3 rules** | **216** | **238** | |

- **Slices**: by crate, the smallest first as the tracer bullet: `ralphy-agent-claude` (16 sites), then `ralphy-core`, `ralphy-adapter-support`, `ralphy-cli`, the daemon's tests.
- **How a site is fixed**: Dropped value: `let _ = expr;` names what it drops. A `Result` reaches its caller with `?` and `.context(..)`, or a log line; a guard that must live to the end of the scope is bound to a name (`let _guard = ...`). `mem_forget` in tests: hold the value (a `TempDir`, a child) in a binding that lives to the end of the test. The commit body names each site where a failure became visible.
- **Ratchet**: none.
- **Agent docs**: none: the line on dropped values in the lint gate block applies.
- **Done when**: the three rules are errors.

### Phase 3 — Rust: escape hatches

- **Lane**: Rust; after Phase 2.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `clippy::indexing_slicing` | 264 | 166 | fix then flip |
| `clippy::string_slice` | 58 | 158 | fix then flip |
| `clippy::as_conversions` | 125 | 70 | fix then flip |
| `clippy::panic` | 0 | 146 | fix then flip |
| `clippy::cast_possible_truncation` | 23 | 15 | fix then flip |
| `clippy::cast_precision_loss` | 20 | 2 | fix then flip |
| `clippy::cast_sign_loss` | 17 | 4 | fix then flip |
| `clippy::cast_possible_wrap` | 5 | 4 | fix then flip |
| `clippy::undocumented_unsafe_blocks` | 0 | 5 | fix then flip |
| **9 rules** | **512** | **570** | |

- **Slices**: by rule, then by crate. Tracer bullet: `cast_possible_wrap` (9 sites). The `indexing_slicing` slice waits for D7; the other slices do not.
- **How a site is fixed**: Escape hatch: parse, don't validate. A cast with `as` becomes `From` or `TryFrom` with the failure handled at the boundary; a slice by byte index (`string_slice`) uses `get(..)`, `split_once` or `char_indices`; an index uses `get(..)` or a pattern; `panic!` in a test helper becomes an assertion with its message, or the helper returns a `Result`; an `unsafe` block gets its `// SAFETY:` comment. An exception (`#[expect(clippy::<lint>, reason = "...")]`) only where the value is proven in range on that line, the reason saying how.
- **Ratchet**: the `#[expect]` count of each lint only goes down after the phase.
- **Agent docs**: none: the escape-hatch line of the block applies.
- **Done when**: the rules are errors, `indexing_slicing` in the crates D7 names.

### Phase 4 — Rust: shape

- **Lane**: Rust; after Phase 3.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `clippy::format_push_string` | 90 | 13 | fix then flip |
| `clippy::too_many_arguments` | 72 | 5 | fix then flip |
| `clippy::needless_pass_by_value` | 57 | 11 | fix then flip |
| `clippy::too_many_lines` | 6 | 29 | fix then flip |
| `clippy::items_after_statements` | 6 | 16 | fix then flip |
| `clippy::match_same_arms` | 14 | 3 | fix then flip |
| `clippy::needless_continue` | 7 | 9 | fix then flip |
| `clippy::unused_async` | 15 | 0 | fix then flip |
| `clippy::unnecessary_wraps` | 11 | 3 | fix then flip |
| `clippy::trivially_copy_pass_by_ref` | 13 | 0 | fix then flip |
| `clippy::ref_option` | 10 | 2 | fix then flip |
| `clippy::assigning_clones` | 12 | 0 | fix then flip |
| `clippy::option_option` | 11 | 1 | fix then flip |
| `clippy::struct_excessive_bools` | 7 | 1 | fix then flip |
| `clippy::format_collect` | 5 | 2 | fix then flip |
| `clippy::manual_let_else` | 4 | 1 | fix then flip |
| `clippy::unused_self` | 4 | 0 | fix then flip |
| `clippy::match_wildcard_for_single_variants` | 1 | 2 | fix then flip |
| `clippy::needless_for_each` | 2 | 0 | fix then flip |
| `clippy::nonminimal_bool` | 1 | 0 | fix then flip |
| `clippy::mod_module_files` | 1 | 0 | fix then flip |
| `clippy::implicit_hasher` | 1 | 0 | fix then flip |
| `clippy::fn_params_excessive_bools` | 1 | 0 | fix then flip |
| **23 rules** | **351** | **98** | |

- **Slices**: by crate; tracer bullet `ralphy-agent-claude` (14 sites). `too_many_arguments` flips with `too-many-arguments-threshold = 5` in `clippy.toml`, in the slice that removes the last function with 5 arguments.
- **How a site is fixed**: Shape: let the data structure the program. Arguments travel in a named struct; a long function is split by the responsibilities it carries; `format_push_string` becomes `write!` on the `String`; a `needless_pass_by_value` takes a reference. Don't patch: rewrite.
- **Ratchet**: the `#[expect(clippy::too_many_arguments` count (28 today) only goes down.
- **Agent docs**: none.
- **Done when**: the rules are errors and the threshold is 5.

### Phase 5 — Rust: surface by hand

- **Lane**: Rust; after Phase 4.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `clippy::clone_on_ref_ptr` | 127 | 75 | fix then flip |
| `elided_lifetimes_in_paths` | 92 | 9 | fix then flip |
| `clippy::missing_panics_doc` | 41 | 50 | fix then flip |
| `missing_debug_implementations` | 70 | 10 | fix then flip |
| `clippy::return_self_not_must_use` | 54 | 0 | fix then flip |
| `clippy::case_sensitive_file_extension_comparisons` | 17 | 28 | fix then flip |
| `clippy::unreadable_literal` | 0 | 33 | fix then flip |
| `clippy::default_trait_access` | 9 | 14 | fix then flip |
| `clippy::similar_names` | 4 | 9 | fix then flip |
| `clippy::used_underscore_binding` | 1 | 10 | fix then flip |
| `clippy::unnecessary_debug_formatting` | 2 | 4 | fix then flip |
| `clippy::struct_field_names` | 4 | 0 | fix then flip |
| `clippy::many_single_char_names` | 2 | 0 | fix then flip |
| **13 rules** | **423** | **242** | |

- **Slices**: by crate; tracer bullet `ralphy-pricing`.
- **How a site is fixed**: Surface: `Arc::clone(&x)` in place of `x.clone()` on a shared pointer; `#[derive(Debug)]`, or a manual `Debug` that hides a secret such as a token; a `# Panics` section where a public function can panic; the elided lifetime written `'_`; a long literal with `_` separators. No behavior changes.
- **Ratchet**: none.
- **Agent docs**: none.
- **Done when**: the rules are errors.

### Phase 6 — Rust: tests in their module

- **Lane**: Rust; after Phase 5.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `clippy::tests_outside_test_module` | 0 | 500 | fix then flip |
| **1 rules** | **0** | **500** | |

- **Slices**: one: each crate under `tests/` gets `#![expect(clippy::tests_outside_test_module, reason = "an integration test crate is the test module")]` at its root; the lint then checks that unit tests live in `#[cfg(test)]`.
- **How a site is fixed**: a unit test outside `#[cfg(test)]` moves into the module (AGENTS.md: tests live next to the code).
- **Ratchet**: none.
- **Agent docs**: none.
- **Done when**: the rule is an error, and `ADDED_IGNORE` in `docs/lint/gate.py` no longer names it.

### Phase 7 — Rust: who prints

- **Lane**: Rust; after Phase 6.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `clippy::print_stdout` | 275 | 0 | decision first |
| `clippy::print_stderr` | 32 | 5 | decision first |
| `clippy::exit` | 9 | 0 | decision first |
| **3 rules** | **316** | **5** | |

- **Slices**: wait for D5. Then by binary crate: `ralphy-pty`, `ralphy-adapter-support`, the daemon's `src/bin`, `xtask`, `ralphy-cli`.
- **How a site is fixed**: Capability: the call goes through the module D5 names, which carries the one exception with its reason. `exit` becomes a return from `main` with an exit code.
- **Ratchet**: the count of exceptions for the three lints only goes down.
- **Agent docs**: the line "output goes through `<module>`" joins the lint gate block, with the module D5 names.
- **Done when**: the rules are errors.

### Phase 8 — TypeScript: fix by command

- **Lane**: TypeScript; after Phase 0.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `eslint(curly)` | 2218 | 319 | fix by command |
| `unicorn(no-useless-undefined)` | 11 | 54 | fix by command |
| `unicorn(no-hex-escape)` | 9 | 55 | fix by command |
| `unicorn(escape-case)` | 11 | 52 | fix by command |
| `unicorn(prefer-string-replace-all)` | 31 | 11 | fix by command |
| `unicorn(prefer-import-meta-properties)` | 0 | 20 | fix by command |
| `unicorn(prefer-at)` | 5 | 8 | fix by command |
| `eslint(no-var)` | 12 | 0 | fix by command |
| `unicorn(prefer-code-point)` | 5 | 0 | fix by command |
| `eslint(prefer-const)` | 2 | 3 | fix by command |
| `typescript(no-unnecessary-template-expression)` | 2 | 0 | fix by command |
| `unicorn(no-useless-switch-case)` | 2 | 0 | fix by command |
| `unicorn(prefer-single-call)` | 1 | 0 | fix by command |
| `unicorn(prefer-dom-node-append)` | 1 | 0 | fix by command |
| `eslint(no-useless-return)` | 1 | 0 | fix by command |
| `eslint(no-useless-concat)` | 1 | 0 | fix by command |
| `unicorn(new-for-builtins)` | 0 | 1 | fix by command |
| **17 rules** | **2312** | **523** | |

- **Slices**: one commit per rule: turn the rule on in `.oxlintrc.json` (with its options), run `npx -y oxlint@1.93.0 --fix crates/ralphy-daemon/assets/ui crates/ralphy-daemon/ui-tests`, then the oxfmt step of the gate without `--check`. The whole config stays on: the fix only narrows what it touches.
- **How a site is fixed**: Surface: the tool's own command. `curly` adds braces to every one-line `if`: a mass change, landed when no long branch is open. By the user's decision of 2026-11-10, the end state of `curly` is `multi-line` (braces only where the body spans more than one line), because `all` would push 17 modules past 500 lines.
- **Ratchet**: none.
- **Agent docs**: none.
- **Done when**: the rules are errors.

### Phase 9 — TypeScript: truth and null

- **Lane**: TypeScript; after Phase 8.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `typescript(strict-boolean-expressions)` | 1089 | 0 | fix then flip |
| `typescript(prefer-nullish-coalescing)` | 648 | 0 | fix then flip |
| `typescript(no-unnecessary-condition)` | 429 | 0 | fix then flip |
| `eslint(no-implicit-coercion)` | 183 | 8 | fix then flip |
| `typescript(no-unnecessary-type-conversion)` | 117 | 0 | fix then flip |
| `unicorn(explicit-length-check)` | 74 | 1 | fix then flip |
| `unicorn(prefer-number-coercion)` | 10 | 5 | fix then flip |
| `typescript(no-unnecessary-boolean-literal-compare)` | 10 | 0 | fix then flip |
| **8 rules** | **2560** | **14** | |

- **Slices**: by module, the smallest first; tracer bullet: the `wb-*-store.ts` module with the fewest sites. The rules go on per module group with an `overrides` entry in `.oxlintrc.json`, and lose the entry when the last group is at 0.
- **How a site is fixed**: Fail fast: a condition the types already decide goes; a value that may be missing is checked once, where it enters (`apiFetch` replies, `socketFrame`), into a type with no `null`; `??` in place of `||` for a default; an explicit comparison (`=== ""`, `.length === 0`) in place of a truthy test.
- **Ratchet**: none.
- **Agent docs**: the fail-fast line of the block names the boundary: `apiFetch` and `socketFrame` in `wb-api.ts`.
- **Done when**: the rules are errors.

### Phase 10 — TypeScript: promises and void

- **Lane**: TypeScript; after Phase 9.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `typescript(no-confusing-void-expression)` | 242 | 0 | fix then flip |
| `eslint(require-await)` | 12 | 167 | fix then flip |
| `eslint(no-promise-executor-return)` | 3 | 62 | fix then flip |
| `promise(always-return)` | 38 | 0 | fix then flip |
| `typescript(strict-void-return)` | 20 | 0 | fix then flip |
| `typescript(consistent-return)` | 10 | 0 | fix then flip |
| `promise(no-callback-in-promise)` | 1 | 0 | fix then flip |
| **7 rules** | **326** | **229** | |

- **Slices**: the tests first (`require-await` and `no-promise-executor-return` are mostly in `ui-tests`), then the modules.
- **How a site is fixed**: Dropped value: a function with no `await` is not `async`; a Promise executor returns nothing; a `.then` returns its value; a void expression is not used as a value.
- **Ratchet**: none.
- **Agent docs**: none.
- **Done when**: the rules are errors.

### Phase 11 — TypeScript: escape hatches

- **Lane**: TypeScript; after Phase 10.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `TS4111` | 213 | 0 | fix then flip |
| `typescript(no-unsafe-type-assertion)` | 102 | 0 | fix then flip |
| `typescript(consistent-type-imports)` | 41 | 0 | fix then flip |
| `TS2379` | 36 | 0 | fix then flip |
| `TS2345` | 14 | 0 | fix then flip |
| `TS2375` | 7 | 0 | fix then flip |
| `TS2412` | 6 | 0 | fix then flip |
| `typescript(no-unnecessary-type-parameters)` | 3 | 0 | fix then flip |
| `TS2322` | 3 | 0 | fix then flip |
| **9 rules** | **425** | **0** | |

- **Slices**: `exactOptionalPropertyTypes` (the TS2379, TS2345, TS2375, TS2412 and TS2322 sites), then `noPropertyAccessFromIndexSignature` (TS4111), each flag turned on in `tsconfig.json` in the slice that removes its last site; then `no-unsafe-type-assertion`. The `consistent-type-imports` slice waits for D8.
- **How a site is fixed**: Escape hatch: parse, don't validate. An optional property is left out, not set to `undefined`; a record read by key uses `["key"]` or a typed map; a cast becomes a type predicate at the boundary.
- **Ratchet**: none.
- **Agent docs**: none.
- **Done when**: the rules are errors and both flags are in `tsconfig.json`.

### Phase 12 — TypeScript: shape and the rest

- **Lane**: TypeScript; after Phase 11.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `eslint(require-unicode-regexp)` | 167 | 294 | fix then flip |
| `unicorn(consistent-function-scoping)` | 50 | 83 | fix then flip |
| `unicorn(prefer-add-event-listener)` | 43 | 2 | fix then flip |
| `unicorn(no-array-sort)` | 22 | 22 | fix then flip |
| `unicorn(prefer-query-selector)` | 23 | 0 | fix then flip |
| `unicorn(no-array-callback-reference)` | 11 | 5 | fix then flip |
| `eslint(max-params)` | 14 | 0 | fix then flip |
| `unicorn(no-negated-condition)` | 13 | 0 | fix then flip |
| `eslint(no-param-reassign)` | 9 | 1 | fix then flip |
| `eslint(max-classes-per-file)` | 0 | 6 | fix then flip |
| `eslint(no-new-func)` | 0 | 6 | fix then flip |
| `unicorn(prefer-dom-node-dataset)` | 5 | 0 | fix then flip |
| `typescript(no-extraneous-class)` | 0 | 4 | fix then flip |
| `eslint(no-script-url)` | 0 | 4 | fix then flip |
| `unicorn(no-this-assignment)` | 1 | 2 | fix then flip |
| `unicorn(no-object-as-default-parameter)` | 0 | 3 | fix then flip |
| `eslint(no-redeclare)` | 1 | 0 | fix then flip |
| `oxc(no-accumulating-spread)` | 0 | 1 | fix then flip |
| `import(no-named-as-default)` | 1 | 0 | fix then flip |
| `unicorn(prefer-type-error)` | 0 | 1 | fix then flip |
| `unicorn(no-array-reverse)` | 0 | 1 | fix then flip |
| `eslint(no-unmodified-loop-condition)` | 0 | 1 | fix then flip |
| `unicorn(no-immediate-mutation)` | 0 | 1 | fix then flip |
| `unicorn(no-static-only-class)` | 0 | 1 | fix then flip |
| `eslint(accessor-pairs)` | 0 | 1 | fix then flip |
| **25 rules** | **360** | **439** | |

- **Slices**: by rule; tracer bullet `eslint(max-params)` (14 sites, with `max: 4` in `.oxlintrc.json`).
- **How a site is fixed**: Shape: arguments in a named object; a function that captures nothing moves to module scope; `toSorted` in place of `sort`; a regex gets the `u` flag where its pattern stays the same. The `no-new-func` and `no-script-url` sites are in tests: a test that builds a function from text keeps an exception with its reason.
- **Ratchet**: none.
- **Agent docs**: none.
- **Done when**: the rules are errors.

### Phase 13 — TypeScript: who writes HTML and logs

- **Lane**: TypeScript; after Phase 12.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `eslint(no-restricted-properties)` | 40 | 6 | decision first |
| `eslint(no-console)` | 24 | 14 | decision first |
| `unicorn(require-post-message-target-origin)` | 1 | 0 | decision first |
| **3 rules** | **65** | **20** | |

- **Slices**: wait for D6. Then `no-console`, then `no-restricted-properties` with its options, by module.
- **How a site is fixed**: Capability: the call goes through the module D6 names. `require-post-message-target-origin` reports a `BroadcastChannel`, which takes no origin: an exception with that reason.
- **Ratchet**: the oxlint steps of the gate gain `--report-unused-disable-directives-severity=error` here: today the one `no-console` disable hides nothing, because the rule is on the debt list.
- **Agent docs**: the line "HTML and logs go through `<module>`" joins the block.
- **Done when**: the rules are errors and an unused disable fails the gate.

### Phase 14 — Clones

- **Lane**: clones; after Phase 0.
- **Rules**:

| Rule | Production | Test | Strategy |
|---|---|---|---|
| `jscpd:rust` | 37 | 0 | count ratchet |
| `jscpd:typescript` | 4 | 0 | count ratchet |
| **2 rules** | **41** | **0** | |

- **Slices**: one: `.jscpd.json` gets `minTokens: 75` and `minLines: 5`; the CI job and the gate step keep `--fail-on-new-clones 0` against the base branch; a new ratchet runs `npx -y jscpd@5.4.0 --reporters json --output <dir> crates` and `python docs/lint/scripts/tally.py jscpd <dir>/jscpd-report.json --expect 41`. A change that removes a clone lowers 41. A second ratchet counts the `jscpd:ignore-start` markers (23 today), each with its reason.
- **How a site is fixed**: Shape: one rule written twice becomes one function named for the rule; two rules that only look alike stay two, with the marker and its reason.
- **Ratchet**: the clone count (41) and the marker count (23).
- **Agent docs**: none: the duplication rule of AGENTS.md already says it.
- **Done when**: both ratchets run in the gate and in CI.

### Phase 15 — Text and files

- **Lane**: text; after Phase 0.
- **Rules**: no lint rule of the record. The items:

| Slice | Tool | Count today |
|---|---|---|
| 15.1 | yamllint over the tracked `.yml` (`line-length` 102, `comments` 66, `document-start` 7, `truthy` 6) | 181 |
| 15.2 | markdownlint over the tracked `.md` files, with the rules D9 sets (`CHANGELOG.md` and vendored files out) | 4,665 with every rule; 1,174 without `MD013` |
| 15.3 | html-validate over the repo's own `.html`, vendored files out (`no-implicit-button-type` 215, `void-style` 109, `text-content` 92) | 480 |
| 15.4 | stylelint over the repo's own `.css` | not measured |
| 15.5 | taplo over the tracked `.toml` | not measured: taplo 0.7 found no file on Windows; measure on Linux |
| 15.6 | lychee over the links of the docs | not measured |
| 15.7 | editorconfig-checker, and `.gitattributes` for every text type | not measured |

- **How a site is fixed**: Surface: each tool's own fix where it has one, by hand otherwise; a rule that is pure noise for this repo is turned off in the tool's config with its reason, listed in D9.
- **Ratchet**: each tool lands at 0, or with its debt per rule in its own config.
- **Agent docs**: none.
- **Done when**: each tool runs in the gate and in CI at a pinned version. 15.2 waits for D9.

## One-way doors

- **The debt list** is the group on and each rule with sites `allow` or `off` under a `# debt: N sites` (TOML) or `// debt: N sites` (JSON) comment on its own line, held by `python docs/lint/gate.py shrink`. A rule off for good is `noise:` with a reason that cites a decision of this plan. Rejected: a baseline file of sites, which goes stale at each edit and hides which rule a site breaks.
- **New lines meet the whole target**: `python docs/lint/gate.py added` runs the drafts in `docs/lint/lint-drafts` and fails on a site on a line the change adds, debt rule or not. Rejected: checking only the rules that are on, which lets the debt grow unseen.
- **The exception form**: Rust `#[expect(<lint>, reason = "...")]` (never `#[allow]`, which `allow_attributes` refuses); TypeScript `// oxlint-disable-next-line <rule> -- <reason>`; ast-grep `// ast-grep-ignore: <rule-id> -- <reason>`; jscpd `// jscpd:ignore-start` with the reason in the comment. Rejected: a file-level disable.
- **Where the configs live**: `Cargo.toml` `[workspace.lints]` and `clippy.toml` at the root, `.oxlintrc.json` at the root, `crates/ralphy-daemon/assets/ui/tsconfig.json`, `.jscpd.json`, `sgconfig.yml` with `ast-grep/`. The plan, its record, the drafts and the scripts are in `docs/lint/`.
- **The ratchets** are tests in `crates/xtask/tests/ratchets/`, a constant each, exact: a count that goes down lowers its constant in the same change.

## Fix discipline

- A fix keeps the behavior. A Dropped value or Fail fast fix changes only the failure path, by design: a failure that was hidden now reaches its caller, and the commit body names each such site. Any other behavior change is its own commit, outside the lint work.
- Each rule has a class, and the class gives the fix:
  - **Dropped value** (an ignored error or `Result`, a floating Promise, a blind `except`, a value never read): the value reaches its caller or a log line. The commit body names each site where a failure became visible.
  - **Fail fast** (a fallback that hides a failure): the fallback goes; the failure surfaces at the boundary that can act on it.
  - **Escape hatch** (`any`, a cast, `!`, `unwrap`, a bare ignore): parse, don't validate: narrow the value once, at the boundary, into a type the checker reads.
  - **Capability** (`print`/`console`, a spawn, `exit`, an environment write outside the module that owns it): the call goes through the owning module, which carries the one exception with its reason.
  - **Shape** (complexity, depth, arguments, size): let the data structure the program: guard clauses and early returns, a table of cases, a `match`/`switch` over a tuple, an iterator in place of a loop with state, or a split along a responsibility the code already has. A file over the size limit is a god file, a function over the complexity limit a god function: split each by the responsibilities it already carries, one module or function per responsibility. Don't patch bad code, rewrite it: helpers that follow no responsibility are not a fix. A clone: one rule written twice becomes one function or constant named for the rule; two rules that only look alike stay two, and the clone carries the ignore marker with the reason they change for different reasons.
  - **Surface** (format, naming, imports): the tool's own command, in a commit of its own.
- An exception is the last resort: inline, the rule named, and a reason that names where the rule's approximation is wrong here (the false positive), or the design fact that makes compliance impossible. "To pass the check" is not a reason. Each exception is counted by a ratchet.
- A test is never weakened, skipped or deleted to turn the gate green, and a hook is never bypassed.
- A baseline or a ratchet constant only goes down, in the commit that removed the sites. It rises only in a move: the commit takes at least as many sites off a debt list as it adds exceptions, and its message gives both counts.
- Each slice leaves the whole gate green.

<!-- end of the plan: progress.py check reads this line -->
