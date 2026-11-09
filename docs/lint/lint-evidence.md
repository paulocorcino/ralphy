# Lint transition: evidence

How [the plan](./lint-transition.md) was cut: what was measured, with which
tools and commands, and a verdict for every row of the lint-gate references.
The agent that works a phase reads the plan, not this file.

## Measured

Commit 13045ccb05b98b892a76a4a323a498e4eaea985f, 2026-11-09, on Windows 11.

| Tool | Version | Pinned where |
|---|---|---|
| Rust and clippy | 1.99.1 (clippy 0.1.99) | `rust-toolchain.toml`, since this run |
| oxlint | 1.93.0 | `ci.yml` `OXLINT_VERSION`, since this run (was 1.85.0) |
| oxlint-tsgolint | 7.0.2006 | `ci.yml` `TSGOLINT_VERSION`, since this run (was 7.0.2005) |
| typescript | 7.0.2 | the `tsc` step of the gate |
| jscpd | 5.4.0 | the `jscpd` step and the CI job |
| ast-grep | 0.51.1 | `ci.yml` `ASTGREP_VERSION`, since this run |
| ruff, mypy (not tracked: D1) | 0.17.4, 2.4.1 | none |
| shellcheck, yamllint, markdownlint-cli2, html-validate (not tracked) | 0.11.0, 1.38.0, 0.18.1, 9.7.1 | none; Phase 0 and X1 pin them |

The record's runs read the drafts in `docs/lint/lint-drafts`; `python
docs/lint/scripts/progress.py commands docs/lint/lint-record.json` prints them.
Set `SC` to a scratch folder outside the repo, and run them from the repo root:

- clippy: `CLIPPY_CONF_DIR=docs/lint/lint-drafts cargo clippy --locked --workspace --all-targets --message-format=json -- $(cat docs/lint/lint-drafts/clippy-lints.txt)`, and the same without `--all-targets` for the production sites; `tally.py diff` of the two gives the test sites. 3,669 production and 3,666 test sites.
- oxlint: `-c docs/lint/lint-drafts/oxlintrc.json --ignore-pattern '**/vendor/**'` over `assets/ui` and `ui-tests`, then with `--type-aware` over `assets/ui` (`OXLINT_TSGOLINT_PATH` set to the tsgolint binary when it is not in `node_modules`). 4,322 and 5,769 sites (the type-aware run counts the plain rules of `assets/ui` again).
- tsc: `--exactOptionalPropertyTypes --noPropertyAccessFromIndexSignature` on top of the repo's `tsconfig.json`: 279 sites. Each flag alone: 66 and 213. The six other target flags: 0 each.
- jscpd: `--min-tokens 75 --min-lines 5 crates`: 41 clones. At 30/5: 729; 50/5: 225; 100/10: 6.
- ast-grep: the rule `error-message-style` read 929 strings and found 1 site (fixed in this run). In 0.51.1 a pattern matches inside a macro's arguments: `assert_eq!($A.unwrap(), $B)` found a site.
- ruff `select = ["ALL"]` with the reference's ignores, over the 130 tracked `.py`: 11,690 sites. mypy strict: 10,221. Python is out (D1).
- Rust suppressions: 0 `#[allow(`; 347 `#[expect(`, every one with a reason (170 `let_underscore_must_use`, 69 `disallowed_methods`, 28 `too_many_arguments`, 28 `unsafe_code`, 23 `wildcard_enum_match_arm`, 18 `dead_code`, 13 `unused_result_ok`, 6 `unused_imports`, 2 `type_complexity`, 2 `zombie_processes`, 1 `duplicate_mod`); 8 `complexity-exception` lines.
- TypeScript suppressions: 3 disable comments (2 `typescript/no-deprecated`, 1 `no-console`); 0 `as unknown as`, `@ts-ignore`, `@ts-expect-error` or `@ts-nocheck`; 0 `complexity` exceptions.
- Python suppressions: 51 `# noqa` or `type: ignore` (out: D1).
- jscpd markers: 23 `jscpd:ignore-start` blocks in 16 files.
- The exit pattern of the gate scripts and CI: 5 matches, each legitimate.
- Text tools: shellcheck 0 sites on 8 `.sh`; yamllint 181; markdownlint 4,665 in 222 files (3,491 `MD013`); html-validate 480 in 30 files; taplo found no file on Windows; PSScriptAnalyzer is not installed.

The debt table, largest first:

| Rule | Tool | Class | Production | Test | Scopes | Strategy | Phase |
|---|---|---|---|---|---|---|---|
| `eslint(curly)` | oxlint | Surface | 2218 | 319 | ui-tests 319, ui 2218 | fix by command | 8 |
| `clippy::str_to_string` | clippy | Surface | 903 | 1449 | ralphy-daemon 1071, ralphy-cli 563, ralphy-core 286 … | fix by command | 1 |
| `typescript(strict-boolean-expressions)` | oxlint | Fail fast | 1089 | 0 | ui-tests 0, ui 1089 | fix then flip | 9 |
| `typescript(prefer-nullish-coalescing)` | oxlint | Fail fast | 648 | 0 | ui-tests 0, ui 648 | fix then flip | 9 |
| `unreachable_pub` | clippy | Surface | 480 | 28 | ralphy-cli 328, ralphy-daemon 143, xtask 29 … | fix by command | 1 |
| `clippy::tests_outside_test_module` | clippy | Surface | 0 | 500 | ralphy-daemon 262, ralphy-core 87, xtask 59 … | fix then flip | 6 |
| `eslint(require-unicode-regexp)` | oxlint | Shape | 167 | 294 | ui-tests 294, ui 167 | fix then flip | 12 |
| `clippy::indexing_slicing` | clippy | Escape hatch | 264 | 166 | xtask 196, ralphy-daemon 156, ralphy-cli 36 … | fix then flip | 3 |
| `typescript(no-unnecessary-condition)` | oxlint | Fail fast | 429 | 0 | ui-tests 0, ui 429 | fix then flip | 9 |
| `clippy::print_stdout` | clippy | Capability | 275 | 0 | ralphy-cli 211, xtask 36, ralphy-daemon 15 … | decision first | 7 |
| `typescript(no-confusing-void-expression)` | oxlint | Dropped value | 242 | 0 | ui-tests 0, ui 242 | fix then flip | 10 |
| `clippy::let_underscore_untyped` | clippy | Dropped value | 133 | 104 | ralphy-cli 70, ralphy-core 35, ralphy-adapter-support 31 … | fix then flip | 2 |
| `clippy::string_slice` | clippy | Escape hatch | 58 | 158 | ralphy-daemon 122, xtask 30, ralphy-cli 25 … | fix then flip | 3 |
| `TS4111` | tsc | Escape hatch | 213 | 0 | ui 213 | fix then flip | 11 |
| `clippy::doc_markdown` | clippy | Surface | 171 | 37 | ralphy-cli 60, ralphy-daemon 51, ralphy-agent-opencode 46 … | fix by command | 1 |
| `unused_qualifications` | clippy | Surface | 19 | 186 | ralphy-daemon 120, ralphy-cli 27, ralphy-agent-codex 13 … | fix by command | 1 |
| `clippy::clone_on_ref_ptr` | clippy | Surface | 127 | 75 | ralphy-daemon 158, ralphy-cli 37, ralphy-agent-claude 3 … | fix then flip | 5 |
| `clippy::as_conversions` | clippy | Escape hatch | 125 | 70 | ralphy-daemon 118, ralphy-cli 33, ralphy-usage-scan 13 … | fix then flip | 3 |
| `eslint(no-implicit-coercion)` | oxlint | Fail fast | 183 | 8 | ui-tests 8, ui 183 | fix then flip | 9 |
| `let_underscore_drop` | clippy | Dropped value | 83 | 96 | ralphy-cli 65, ralphy-adapter-support 31, ralphy-daemon 26 … | fix then flip | 2 |
| `eslint(require-await)` | oxlint | Dropped value | 12 | 167 | ui-tests 167, ui 12 | fix then flip | 10 |
| `clippy::assert_is_empty` | clippy | Surface | 0 | 157 | ralphy-daemon 35, xtask 32, ralphy-core 23 … | fix by command | 1 |
| `clippy::redundant_closure_for_method_calls` | clippy | Surface | 86 | 60 | ralphy-daemon 44, ralphy-cli 23, ralphy-core 19 … | fix by command | 1 |
| `clippy::panic` | clippy | Escape hatch | 0 | 146 | ralphy-daemon 100, xtask 31, ralphy-core 8 … | fix then flip | 3 |
| `unicorn(consistent-function-scoping)` | oxlint | Shape | 50 | 83 | ui-tests 83, ui 50 | fix then flip | 12 |
| `typescript(no-unnecessary-type-conversion)` | oxlint | Fail fast | 117 | 0 | ui-tests 0, ui 117 | fix then flip | 9 |
| `clippy::format_push_string` | clippy | Shape | 90 | 13 | ralphy-cli 34, ralphy-daemon 30, ralphy-core 20 … | fix then flip | 4 |
| `typescript(no-unsafe-type-assertion)` | oxlint | Escape hatch | 102 | 0 | ui-tests 0, ui 102 | fix then flip | 11 |
| `elided_lifetimes_in_paths` | clippy | Surface | 92 | 9 | ralphy-core 30, ralphy-usage-scan 20, ralphy-cli 11 … | fix then flip | 5 |
| `clippy::missing_panics_doc` | clippy | Surface | 41 | 50 | ralphy-daemon 79, ralphy-core 11, ralphy-pricing 1 | fix then flip | 5 |
| `missing_debug_implementations` | clippy | Surface | 70 | 10 | ralphy-daemon 41, ralphy-core 10, ralphy-adapter-support 8 … | fix then flip | 5 |
| `clippy::map_unwrap_or` | clippy | Surface | 65 | 13 | ralphy-cli 18, ralphy-daemon 18, ralphy-core 13 … | fix by command | 1 |
| `clippy::too_many_arguments` | clippy | Shape | 72 | 5 | ralphy-daemon 28, ralphy-cli 15, ralphy-agent-claude 5 … | fix then flip | 4 |
| `unicorn(explicit-length-check)` | oxlint | Fail fast | 74 | 1 | ui-tests 1, ui 74 | fix then flip | 9 |
| `clippy::needless_pass_by_value` | clippy | Shape | 57 | 11 | ralphy-cli 29, ralphy-daemon 20, ralphy-core 3 … | fix then flip | 4 |
| `unicorn(no-useless-undefined)` | oxlint | Surface | 11 | 54 | ui-tests 54, ui 11 | fix by command | 8 |
| `eslint(no-promise-executor-return)` | oxlint | Dropped value | 3 | 62 | ui-tests 62, ui 3 | fix then flip | 10 |
| `unicorn(no-hex-escape)` | oxlint | Surface | 9 | 55 | ui-tests 55, ui 9 | fix by command | 8 |
| `unicorn(escape-case)` | oxlint | Surface | 11 | 52 | ui-tests 52, ui 11 | fix by command | 8 |
| `clippy::return_self_not_must_use` | clippy | Surface | 54 | 0 | ralphy-adapter-support 8, ralphy-agent-copilot 7, ralphy-agent-opencode 6 … | fix then flip | 5 |
| `eslint(no-restricted-properties)` | oxlint | Capability | 40 | 6 | ui-tests 6, ui 40 | decision first | 13 |
| `clippy::case_sensitive_file_extension_comparisons` | clippy | Surface | 17 | 28 | ralphy-daemon 27, xtask 16, ralphy-run-snapshot 1 … | fix then flip | 5 |
| `unicorn(prefer-add-event-listener)` | oxlint | Shape | 43 | 2 | ui-tests 2, ui 43 | fix then flip | 12 |
| `unicorn(no-array-sort)` | oxlint | Shape | 22 | 22 | ui-tests 22, ui 22 | fix then flip | 12 |
| `unicorn(prefer-string-replace-all)` | oxlint | Surface | 31 | 11 | ui-tests 11, ui 31 | fix by command | 8 |
| `typescript(consistent-type-imports)` | oxlint | Escape hatch | 41 | 0 | ui-tests 0, ui 41 | fix then flip | 11 |
| `clippy::semicolon_if_nothing_returned` | clippy | Surface | 26 | 13 | ralphy-cli 28, ralphy-daemon 4, ralphy-core 3 … | fix by command | 1 |
| `clippy::mem_forget` | clippy | Dropped value | 0 | 38 | ralphy-daemon 38 | fix then flip | 2 |
| `clippy::cast_possible_truncation` | clippy | Escape hatch | 23 | 15 | ralphy-daemon 25, ralphy-cli 5, ralphy-proc-util 3 … | fix then flip | 3 |
| `promise(always-return)` | oxlint | Dropped value | 38 | 0 | ui-tests 0, ui 38 | fix then flip | 10 |
| `eslint(no-console)` | oxlint | Capability | 24 | 14 | ui-tests 14, ui 24 | decision first | 13 |
| `clippy::print_stderr` | clippy | Capability | 32 | 5 | ralphy-cli 23, ralphy-adapter-support 7, ralphy-daemon 3 … | decision first | 7 |
| `jscpd:rust` | jscpd | Shape | 37 | 0 | crates | count ratchet | 14 |
| `TS2379` | tsc | Escape hatch | 36 | 0 | ui 36 | fix then flip | 11 |
| `clippy::too_many_lines` | clippy | Shape | 6 | 29 | ralphy-daemon 23, ralphy-cli 9, ralphy-core 2 … | fix then flip | 4 |
| `clippy::unreadable_literal` | clippy | Surface | 0 | 33 | ralphy-usage-scan 28, ralphy-agent-codex 3, ralphy-daemon 1 … | fix then flip | 5 |
| `clippy::borrow_as_ptr` | clippy | Surface | 28 | 1 | ralphy-proc-util 10, ralphy-daemon 9, ralphy-cli 7 … | fix by command | 1 |
| `clippy::default_trait_access` | clippy | Surface | 9 | 14 | ralphy-daemon 12, ralphy-cli 11 | fix then flip | 5 |
| `unicorn(prefer-query-selector)` | oxlint | Shape | 23 | 0 | ui-tests 0, ui 23 | fix then flip | 12 |
| `clippy::single_match_else` | clippy | Surface | 19 | 3 | ralphy-daemon 9, xtask 4, ralphy-cli 3 … | fix by command | 1 |
| `clippy::cast_precision_loss` | clippy | Escape hatch | 20 | 2 | ralphy-daemon 9, ralphy-cli 7, ralphy-pricing 6 | fix then flip | 3 |
| `clippy::items_after_statements` | clippy | Shape | 6 | 16 | ralphy-agent-gemini 6, ralphy-core 5, ralphy-cli 4 … | fix then flip | 4 |
| `clippy::duration_suboptimal_units` | clippy | Surface | 7 | 14 | ralphy-adapter-support 10, ralphy-daemon 5, ralphy-agent-claude 2 … | fix by command | 1 |
| `clippy::cast_sign_loss` | clippy | Escape hatch | 17 | 4 | ralphy-usage-scan 12, ralphy-daemon 7, ralphy-agent-gemini 1 … | fix then flip | 3 |
| `clippy::cast_lossless` | clippy | Surface | 7 | 14 | ralphy-daemon 18, ralphy-cli 1, ralphy-run-snapshot 1 … | fix by command | 1 |
| `typescript(strict-void-return)` | oxlint | Dropped value | 20 | 0 | ui-tests 0, ui 20 | fix then flip | 10 |
| `unicorn(prefer-import-meta-properties)` | oxlint | Surface | 0 | 20 | ui-tests 20, ui 0 | fix by command | 8 |
| `clippy::needless_raw_string_hashes` | clippy | Surface | 0 | 18 | xtask 11, ralphy-daemon 6, ralphy-cli 1 | fix by command | 1 |
| `clippy::match_same_arms` | clippy | Shape | 14 | 3 | ralphy-cli 9, ralphy-daemon 5, ralphy-core 1 … | fix then flip | 4 |
| `clippy::needless_continue` | clippy | Shape | 7 | 9 | ralphy-daemon 12, ralphy-agent-claude 1, ralphy-release 1 … | fix then flip | 4 |
| `unicorn(no-array-callback-reference)` | oxlint | Shape | 11 | 5 | ui-tests 5, ui 11 | fix then flip | 12 |
| `clippy::unused_async` | clippy | Shape | 15 | 0 | ralphy-daemon 15 | fix then flip | 4 |
| `unicorn(prefer-number-coercion)` | oxlint | Fail fast | 10 | 5 | ui-tests 5, ui 10 | fix then flip | 9 |
| `clippy::unnecessary_wraps` | clippy | Shape | 11 | 3 | ralphy-cli 7, ralphy-daemon 5, ralphy-core 1 … | fix then flip | 4 |
| `eslint(max-params)` | oxlint | Shape | 14 | 0 | ui-tests 0, ui 14 | fix then flip | 12 |
| `TS2345` | tsc | Escape hatch | 14 | 0 | ui 14 | fix then flip | 11 |
| `clippy::trivially_copy_pass_by_ref` | clippy | Shape | 13 | 0 | ralphy-cli 8, ralphy-daemon 2, ralphy-agent-cursor 1 … | fix then flip | 4 |
| `clippy::similar_names` | clippy | Surface | 4 | 9 | ralphy-daemon 7, ralphy-core 3, ralphy-cli 1 … | fix then flip | 5 |
| `unicorn(prefer-at)` | oxlint | Surface | 5 | 8 | ui-tests 8, ui 5 | fix by command | 8 |
| `unicorn(no-negated-condition)` | oxlint | Shape | 13 | 0 | ui-tests 0, ui 13 | fix then flip | 12 |
| `clippy::ref_option` | clippy | Shape | 10 | 2 | ralphy-daemon 4, ralphy-usage-scan 3, xtask 2 … | fix then flip | 4 |
| `clippy::assigning_clones` | clippy | Shape | 12 | 0 | ralphy-daemon 6, ralphy-cli 4, ralphy-usage-scan 2 | fix then flip | 4 |
| `clippy::option_option` | clippy | Shape | 11 | 1 | ralphy-agent-opencode 5, ralphy-cli 4, ralphy-agent-gemini 1 … | fix then flip | 4 |
| `eslint(no-var)` | oxlint | Surface | 12 | 0 | ui-tests 0, ui 12 | fix by command | 8 |
| `clippy::uninlined_format_args` | clippy | Surface | 9 | 2 | ralphy-daemon 5, ralphy-core 4, ralphy-agent-claude 2 | fix by command | 1 |
| `clippy::used_underscore_binding` | clippy | Surface | 1 | 10 | ralphy-daemon 10, ralphy-cli 1 | fix then flip | 5 |
| `typescript(consistent-return)` | oxlint | Dropped value | 10 | 0 | ui-tests 0, ui 10 | fix then flip | 10 |
| `eslint(no-param-reassign)` | oxlint | Shape | 9 | 1 | ui-tests 1, ui 9 | fix then flip | 12 |
| `typescript(no-unnecessary-boolean-literal-compare)` | oxlint | Fail fast | 10 | 0 | ui-tests 0, ui 10 | fix then flip | 9 |
| `clippy::exit` | clippy | Capability | 9 | 0 | ralphy-daemon 4, ralphy-cli 3, ralphy-adapter-support 2 | decision first | 7 |
| `clippy::cast_possible_wrap` | clippy | Escape hatch | 5 | 4 | ralphy-daemon 8, ralphy-proc-util 1 | fix then flip | 3 |
| `clippy::struct_excessive_bools` | clippy | Shape | 7 | 1 | ralphy-cli 3, ralphy-adapter-support 2, ralphy-core 2 … | fix then flip | 4 |
| `clippy::format_collect` | clippy | Shape | 5 | 2 | ralphy-daemon 5, ralphy-agent-codex 1, xtask 1 | fix then flip | 4 |
| `clippy::unnested_or_patterns` | clippy | Surface | 5 | 2 | ralphy-agent-claude 2, ralphy-daemon 2, ralphy-agent-gemini 1 … | fix by command | 1 |
| `TS2375` | tsc | Escape hatch | 7 | 0 | ui 7 | fix then flip | 11 |
| `clippy::unnecessary_debug_formatting` | clippy | Surface | 2 | 4 | ralphy-daemon 3, ralphy-pty 3 | fix then flip | 5 |
| `eslint(max-classes-per-file)` | oxlint | Shape | 0 | 6 | ui-tests 6, ui 0 | fix then flip | 12 |
| `eslint(no-new-func)` | oxlint | Shape | 0 | 6 | ui-tests 6, ui 0 | fix then flip | 12 |
| `TS2412` | tsc | Escape hatch | 6 | 0 | ui 6 | fix then flip | 11 |
| `clippy::ignored_unit_patterns` | clippy | Surface | 5 | 0 | ralphy-daemon 2, ralphy-agent-cursor 2, ralphy-agent-copilot 1 | fix by command | 1 |
| `clippy::manual_let_else` | clippy | Shape | 4 | 1 | ralphy-cli 2, ralphy-daemon 2, ralphy-core 1 | fix then flip | 4 |
| `clippy::if_not_else` | clippy | Surface | 5 | 0 | ralphy-core 2, ralphy-agent-codex 1, ralphy-cli 1 … | fix by command | 1 |
| `clippy::undocumented_unsafe_blocks` | clippy | Escape hatch | 0 | 5 | ralphy-cli 5 | fix then flip | 3 |
| `unicorn(prefer-code-point)` | oxlint | Surface | 5 | 0 | ui-tests 0, ui 5 | fix by command | 8 |
| `eslint(prefer-const)` | oxlint | Surface | 2 | 3 | ui-tests 3, ui 2 | fix by command | 8 |
| `unicorn(prefer-dom-node-dataset)` | oxlint | Shape | 5 | 0 | ui-tests 0, ui 5 | fix then flip | 12 |
| `clippy::unused_self` | clippy | Shape | 4 | 0 | ralphy-agent-copilot 2, ralphy-agent-claude 1, ralphy-daemon 1 | fix then flip | 4 |
| `clippy::struct_field_names` | clippy | Surface | 4 | 0 | ralphy-daemon 2, ralphy-usage-scan 1, xtask 1 | fix then flip | 5 |
| `typescript(no-extraneous-class)` | oxlint | Shape | 0 | 4 | ui-tests 4, ui 0 | fix then flip | 12 |
| `eslint(no-script-url)` | oxlint | Shape | 0 | 4 | ui-tests 4, ui 0 | fix then flip | 12 |
| `jscpd:typescript` | jscpd | Shape | 4 | 0 | crates | count ratchet | 14 |
| `clippy::range_plus_one` | clippy | Surface | 0 | 3 | ralphy-daemon 3 | fix by command | 1 |
| `clippy::manual_assert_eq` | clippy | Surface | 0 | 3 | ralphy-release 2, ralphy-core 1 | fix by command | 1 |
| `clippy::match_wildcard_for_single_variants` | clippy | Shape | 1 | 2 | ralphy-core 1, ralphy-cli 1, ralphy-git-read 1 | fix then flip | 4 |
| `clippy::manual_assert` | clippy | Surface | 0 | 3 | ralphy-daemon 2, ralphy-cli 1 | fix by command | 1 |
| `clippy::unchecked_time_subtraction` | clippy | Surface | 0 | 3 | ralphy-daemon 2, ralphy-cli 1 | fix by command | 1 |
| `clippy::ptr_as_ptr` | clippy | Surface | 3 | 0 | ralphy-core 3 | fix by command | 1 |
| `clippy::manual_is_variant_and` | clippy | Surface | 2 | 1 | ralphy-cli 1, ralphy-release 1, ralphy-pricing 1 | fix by command | 1 |
| `unicorn(no-this-assignment)` | oxlint | Shape | 1 | 2 | ui-tests 2, ui 1 | fix then flip | 12 |
| `typescript(no-unnecessary-type-parameters)` | oxlint | Escape hatch | 3 | 0 | ui-tests 0, ui 3 | fix then flip | 11 |
| `unicorn(no-object-as-default-parameter)` | oxlint | Shape | 0 | 3 | ui-tests 3, ui 0 | fix then flip | 12 |
| `TS2322` | tsc | Escape hatch | 3 | 0 | ui 3 | fix then flip | 11 |
| `clippy::implicit_clone` | clippy | Surface | 2 | 0 | ralphy-daemon 1, ralphy-core 1 | fix by command | 1 |
| `clippy::wildcard_imports` | clippy | Surface | 2 | 0 | ralphy-daemon 2 | fix by command | 1 |
| `clippy::needless_for_each` | clippy | Shape | 2 | 0 | ralphy-daemon 2 | fix then flip | 4 |
| `clippy::many_single_char_names` | clippy | Surface | 2 | 0 | ralphy-daemon 2 | fix then flip | 5 |
| `typescript(no-unnecessary-template-expression)` | oxlint | Surface | 2 | 0 | ui-tests 0, ui 2 | fix by command | 8 |
| `unicorn(no-useless-switch-case)` | oxlint | Surface | 2 | 0 | ui-tests 0, ui 2 | fix by command | 8 |
| `clippy::nonminimal_bool` | clippy | Shape | 1 | 0 | ralphy-cli 1 | fix then flip | 4 |
| `clippy::inconsistent_struct_constructor` | clippy | Surface | 0 | 1 | ralphy-daemon 1 | fix by command | 1 |
| `clippy::option_as_ref_cloned` | clippy | Surface | 1 | 0 | ralphy-daemon 1 | fix by command | 1 |
| `clippy::unnecessary_join` | clippy | Surface | 1 | 0 | ralphy-agent-gemini 1 | fix by command | 1 |
| `clippy::filter_map_next` | clippy | Surface | 1 | 0 | ralphy-usage-scan 1 | fix by command | 1 |
| `clippy::manual_midpoint` | clippy | Surface | 1 | 0 | ralphy-daemon 1 | fix by command | 1 |
| `clippy::mod_module_files` | clippy | Shape | 1 | 0 | ralphy-cli 1 | fix then flip | 4 |
| `clippy::explicit_iter_loop` | clippy | Surface | 1 | 0 | xtask 1 | fix by command | 1 |
| `clippy::unnecessary_semicolon` | clippy | Surface | 1 | 0 | ralphy-cli 1 | fix by command | 1 |
| `clippy::enum_glob_use` | clippy | Surface | 1 | 0 | ralphy-cli 1 | fix by command | 1 |
| `clippy::implicit_hasher` | clippy | Shape | 1 | 0 | ralphy-daemon 1 | fix then flip | 4 |
| `clippy::fn_params_excessive_bools` | clippy | Shape | 1 | 0 | ralphy-agent-opencode 1 | fix then flip | 4 |
| `clippy::stable_sort_primitive` | clippy | Surface | 0 | 1 | ralphy-agent-gemini 1 | fix by command | 1 |
| `clippy::manual_string_new` | clippy | Surface | 0 | 1 | ralphy-core 1 | fix by command | 1 |
| `eslint(no-redeclare)` | oxlint | Shape | 1 | 0 | ui-tests 0, ui 1 | fix then flip | 12 |
| `unicorn(require-post-message-target-origin)` | oxlint | Capability | 1 | 0 | ui-tests 0, ui 1 | decision first | 13 |
| `oxc(no-accumulating-spread)` | oxlint | Shape | 0 | 1 | ui-tests 1, ui 0 | fix then flip | 12 |
| `unicorn(prefer-single-call)` | oxlint | Surface | 1 | 0 | ui-tests 0, ui 1 | fix by command | 8 |
| `import(no-named-as-default)` | oxlint | Shape | 1 | 0 | ui-tests 0, ui 1 | fix then flip | 12 |
| `unicorn(prefer-dom-node-append)` | oxlint | Surface | 1 | 0 | ui-tests 0, ui 1 | fix by command | 8 |
| `unicorn(prefer-type-error)` | oxlint | Shape | 0 | 1 | ui-tests 1, ui 0 | fix then flip | 12 |
| `unicorn(no-array-reverse)` | oxlint | Shape | 0 | 1 | ui-tests 1, ui 0 | fix then flip | 12 |
| `eslint(no-useless-return)` | oxlint | Surface | 1 | 0 | ui-tests 0, ui 1 | fix by command | 8 |
| `eslint(no-unmodified-loop-condition)` | oxlint | Shape | 0 | 1 | ui-tests 1, ui 0 | fix then flip | 12 |
| `promise(no-callback-in-promise)` | oxlint | Dropped value | 1 | 0 | ui-tests 0, ui 1 | fix then flip | 10 |
| `eslint(no-useless-concat)` | oxlint | Surface | 1 | 0 | ui-tests 0, ui 1 | fix by command | 8 |
| `unicorn(no-immediate-mutation)` | oxlint | Shape | 0 | 1 | ui-tests 1, ui 0 | fix then flip | 12 |
| `unicorn(no-static-only-class)` | oxlint | Shape | 0 | 1 | ui-tests 1, ui 0 | fix then flip | 12 |
| `eslint(accessor-pairs)` | oxlint | Shape | 0 | 1 | ui-tests 1, ui 0 | fix then flip | 12 |
| `unicorn(new-for-builtins)` | oxlint | Surface | 0 | 1 | ui-tests 1, ui 0 | fix by command | 8 |

## Coverage

| repo: One local command | on — `cargo run -q -p xtask -- gate` |
| repo: CI mirrors it | on — `the_gate_mirrors_every_ci_step` |
| repo: No warn level | on — clippy and rustdoc with `-D warnings`, oxlint with `--deny-warnings` |
| repo: Pinned tools | on — CI pins each tool by version and checksum; `rust-toolchain.toml` pins Rust since this run; the `tool_pins` ratchets |
| repo: New releases of the pinned tools | phase 0 |
| repo: Locked dependencies | on — `every_cargo_command_in_a_workflow_is_locked` |
| repo: No new duplicated block | on — jscpd `--fail-on-new-clones 0` against the base branch |
| repo: The existing duplicated blocks only go down | phase 14 |
| repo: Function complexity at most 10 | on — `xtask complexity` (Rust, from the source text) and oxlint `complexity` (TS), ADR-0081 |
| repo: File size at most N lines | on — `xtask oversized`, 500 lines with every line counted |
| repo: Inline test block at most N lines | n/a — no reference value, and the developer set none |
| repo: Direction of dependencies | phase 0 |
| repo: Ratchets as tests | on — `crates/xtask/tests/ratchets` |
| repo: No automatic retry | on — `.config/nextest.toml` sets no `retries` |
| repo: Doctests run | on — the gate step `doctest` |
| repo: Coverage of the diff at least N% | decision — D12 |
| repo: The test catches the real error | decision — D10 |
| repo: No silent snapshot update | on — the shared replies compare unless `UPDATE_GOLDEN=1`, which CI never sets |
| repo: No focused or skipped test left behind | on — the `skipped_tests` ratchets |
| repo: A secret in the code | phase 0 |
| repo: A vulnerable dependency | on — cargo deny, and osv-scanner for the vendored UI libraries (CI only) |
| repo: Static analysis | on — CodeQL (CI only) |
| repo: An unsafe workflow | on — zizmor and actionlint; actions pinned by SHA |
| repo: Dependency updates | on — Dependabot, weekly, in groups, with a cooldown |
| repo: A typo in code or docs | on — typos |
| repo: Shell scripts | phase 0 |
| repo: PowerShell scripts | phase 0 |
| repo: HTML and CSS | phase 15 |
| repo: TOML, YAML, Markdown | phase 15 |
| repo: A broken link in the docs | phase 15 |
| repo: Line ends and encoding | phase 15 |
| repo: A generated file out of date | on — the prompt-assembly tests compare each generated plan prompt with its sources |
| repo: Commit message format | decision — D13 |
| repo: Changelog | on — the `changelog fragment` CI job and `xtask changelog --check` |
| repo: Internal words in user-facing text | on — `xtask ui-copy` and the checks on ADR citations in user text |
| repo: Protected main branch | on — ruleset 26428842: pull request, the CI checks required, no force push |
| repo: An agent hook | decision — D11 |
| repo: Shift left | decision — D11 |
| repo: Pre-commit hook (optional) | n/a — optional; the local gate and CI are the gate |
| repo: A warn level | closed by every linter's deny-warnings flag |
| repo: A bypass (--no-verify, a force push, a merge with a red check) | open — the ruleset closes a force push and a red merge; `--no-verify` waits for D11 |
| repo: An unasked edit to a lint config, a debt list, an ignore list or a ratchet constant | closed by `gate.py shrink` for the debt lists; the ignore lists are phase 0 (0.5) |
| repo: A suppression with no reason, or one that suppresses nothing | closed by `allow_attributes`, `allow_attributes_without_reason` and `ast-grep scan --error=unused-suppression`; the unused oxlint disables come in Phase 13 |
| repo: A blanket or file-level suppression (@ts-nocheck, a file-level disable, a crate-root allow) | closed by `allow_attributes` (no `#[allow]` anywhere) and `no_escape_hatch_in_the_workbench` |
| repo: A path no check reads (a new directory outside every glob) | open — the UI steps name `crates/ralphy-daemon/assets/ui` and `ui-tests`; a new TypeScript folder elsewhere is read by no linter |
| repo: A skipped or deleted test | closed by the `skipped_tests` ratchets (skipped); a deleted test waits for D10 |
| repo: A weakened test (an assertion that checks nothing) | decision — D10 |
| repo: Code the gate does not build (#[cfg] of another platform, a feature, a build tag) | open — clippy runs `--all-features` since this run, on Windows locally and Linux in CI; macOS-only code is built only by the release workflow, and no lint reads it |
| repo: A split into helpers with no responsibility, to pass a complexity limit | open — a human reads the split |
| repo: Two rules merged into one helper to remove a clone | open — a human reads the merge |
| repo: A clone hidden by an ignore marker (jscpd:ignore-start) | phase 14 |
| repo: A row of a reference that the lint gate run skipped | closed by `rows.py check` on this file |
| repo: An ast-grep-ignore with no rule id, or on the first line of a file | closed by `every_ast_grep_ignore_names_its_rule_and_why` and `ast_grep_ignores_do_not_grow` |
| repo: A force push to the default branch that drops the base commit: shrink against an empty base passes | closed by the ruleset: no force push and no deletion on `main` |
| repo: The skill's own artifacts in the repo (the copied scripts, the plan's code blocks) counted or linted as the repo's code | closed by the linters' paths: oxlint, jscpd and ast-grep read `crates/` only, and no Python lint runs (D1); typos reads `docs/lint` and passes |
| repo: A new noise: entry in a debt list: a debt rule off "for good" | closed by `gate.py shrink`, which needs a decision of the plan in the reason |
| repo: A fake reason on an exception | open — the exceptions are counted; a human reads each new one |
| repo: A new site of a rule on the debt list: the rule is off, so its debt grows unseen | closed by `gate.py added` |
| repo: A step that passes when its tool is missing or fails (\|\| true, command -v x &&, continue-on-error: true, set +e, a skip when not installed) | phase 0 |
| rust: Format | on — `cargo fmt --all --check` with rustfmt's defaults |
| rust: Lints, test code too | on — `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` |
| rust: Docs and intra-doc links | on — the gate step `doc` |
| rust: Advisories, licenses, sources | on — cargo deny (CI only) |
| rust: Declared, unused dependency | on — cargo machete |
| rust: Dead code | phase 1 |
| rust: Leftovers (a TODO, FIXME or commented-out code) | on — `no_rust_comment_leaves_work_behind`, since this run (0 sites) |
| rust: A feature that does not build alone | on — one feature (`scroll-probe`); clippy builds it with `--all-features` |
| rust: Public API break (published library) | n/a — no crate is published |
| rust: Lockfile matches Cargo.toml | on — `--locked` on every cargo command, held by a ratchet |
| rust: Too-new language feature | n/a — `rust-toolchain.toml` pins the one toolchain; the project ships binaries, not a library with a floor |
| rust: Doctests | on — `cargo test --locked --workspace --doc` |
| rust: Mutation on the diff | decision — D10 |
| rust: clippy::expect_used | out — `expect` with a message states an invariant (AGENTS.md) |
| rust: arithmetic_side_effects, float_arithmetic | out — no money or cryptography code |
| rust: the whole clippy::restriction group | out — one restriction lint at a time |
| rust: missing_docs everywhere | out — no published library |
| typescript: Promises | phase 10 |
| typescript: Unsafe types | phase 11 |
| typescript: Truth and null | phase 9 |
| typescript: Strings | on — `restrict-template-expressions`, `restrict-plus-operands` and `no-base-to-string`, 0 sites |
| typescript: Closed sets | on — `switch-exhaustiveness-check` |
| typescript: Other | phase 10 |
| typescript: Logic | phase 8 |
| typescript: Control flow | on — `no-nested-ternary` and `no-else-return` |
| typescript: Size | phase 12 |
| typescript: Imports | on — `import/no-cycle`, `no-duplicates`, `no-self-import`, `first`, `extensions`, and `no-default-export` since this run |
| typescript: Architecture | decision — D6 |
| typescript: Sinks | phase 13 |
| typescript: Leftovers | on — `no-warning-comments`, since this run (0 sites) |
| typescript: Suppressions | phase 13 |
| typescript: unicorn | phase 8 |
| typescript: Tests | on — `no_ui_test_is_only_or_skipped`; the tests run on `node:test`, which no lint plugin reads |
| typescript: Logs | phase 13 |
| typescript: Performance | phase 12 |
| typescript: no-chained-type-assertions, no-widen-then-assert | decision — D14 |
| typescript: no-unknown-returns, no-unknown-type-aliases, no-unsafe-dictionary-type, no-object-parameters, no-reflect-get, no-reflect-apply | decision — D14 |
| typescript: no-known-value-widening | decision — D14 |
| typescript: no-unknown-parameters | decision — D14 |
| typescript: no-runtime-typeof | decision — D14 |
| typescript: require-safety-comment-for-type-assertion | decision — D14 |
| typescript: no-module-mocking | decision — D14 |
| typescript: the anti-slop-effect rules | n/a — the workbench does not use Effect |
| typescript: no-array-filter-map, no-reduce-accumulator-copy | decision — D14 |
| typescript: require-readable-spacing | out — blank lines belong to the formatter |
| typescript: no-shape-in-symbol-names | out — a naming taste |
| typescript: no-conditional-empty-object-spread | out — the way to leave a field out under `exactOptionalPropertyTypes` |
| typescript: Types | on — `tsc --noEmit` (typescript 7.0.2) |
| typescript: Lint | on — oxlint `--deny-warnings`, and `--type-aware` over the modules |
| typescript: Format | on — oxfmt `--check` |
| typescript: Dead code (unused files, exports, dependencies) | n/a — the workbench has no `package.json` for knip; `noUnusedLocals` and `noUnusedParameters` are on |
| typescript: Mutation on the diff | decision — D10 |
| typescript: explicit-function-return-type | out — with `strict`, the inferred type is checked |
| typescript: no-return-await | out — `return-await` covers it |
| typescript: arrow-body-style: always | out — more lines, same behavior |
| typescript: sonarjs/no-duplicate-string | out — it merges two things because their text is the same |
| typescript: max-lines-per-function | out — complexity is the limit (D4) |
| typescript: import path aliases, import sorting | out — every import is a flat `./wb-x.ts` |
| python: Format | out — Python is out of the gate (D1) |
| python: Lint | out — Python is out of the gate (D1) |
| python: Types | out — Python is out of the gate (D1) |
| python: Cognitive complexity | out — Python is out of the gate (D1) |
| python: Declared, unused or missing dependency | out — Python is out of the gate (D1) |
| python: Dead code (functions, classes, imports nobody uses) | out — Python is out of the gate (D1) |
| python: Vulnerable dependency | out — Python is out of the gate (D1) |
| python: Lockfile matches the project | out — Python is out of the gate (D1) |
| python: Import layers | out — Python is out of the gate (D1) |
| python: Tests | out — Python is out of the gate (D1) |
| python: Coverage of the diff | out — Python is out of the gate (D1) |
| python: Mutation | out — Python is out of the gate (D1) |
| python: Python floor | out — Python is out of the gate (D1) |
| python: the D group everywhere | out — Python is out of the gate (D1) |
| python: PLR2004 magic values | out — Python is out of the gate (D1) |
| python: disallow_any_expr | out — Python is out of the gate (D1) |
| python: ruff preview rules in the gate | out — Python is out of the gate (D1) |
