# Borderline 500-line file triage (#110)

Purpose: per #110, triage the files that sit only modestly above 500 lines and
decide, file by file, whether a real multi-responsibility seam justifies a
split, or whether the file is cohesive and should stay as-is (or only have its
tests moved to a sibling file, the [#100](https://github.com/paulocorcino/ralphy/issues/100)
pattern). This doc performs no split itself — see [ADR-0022](adr/0022-file-split-conventions.md)
§5 (split by existing responsibility only, no invented abstraction) and §2
(public API unchanged); every verdict below is judged against those two rules.
Child issues are opened only for files verdict'd `split` or `move-tests`.

Reframing: all 9 files have **production code under 500 lines** (291–483); each
crosses 500 only because of its inline `#[cfg(test)]` module. Counts below were
measured this pass (2026-07-04) directly from the file — `total` = `wc -l`,
`prod` = line before the file's `#[cfg(test)]` marker (or, where the test
module sits mid-file, total minus the test module's line span), `test` = the
rest.

| file | total | prod | test | responsibilities | verdict | rationale | child issue |
|---|---|---|---|---|---|---|---|
| `crates/ralphy-adapter-support/src/lib.rs` | 705 | 416 | 289 | (1) done/blocked sentinel parsing (`done_sentinel`, `blocked_reason`); (2) `materialize_assets`; (3) program resolution/location (`resolve_program`, `locate_program[_with]`, `home_dir`, `is_executable_file`, `find_program`); (4) `run_headless` subprocess driving (`HeadlessOutput`, `run_headless`, `recv_and_join`, `kill_tree`) | split | ADR-0022 §5: 4 real, pre-existing seams with no shared state between them. The `#[cfg(test)]` module also sits **mid-file** (lines 81–369, between concern 1 and concern 2), violating the ADR §3 test-placement convention on its own — this alone would justify a move, and the responsibility boundaries make a full split (not just move-tests) the better call. | #112 |
| `crates/ralphy-cli/src/guard.rs` | 902 | 434 | 468 | one cohesive concern: guard-hook evaluation (bash-deny classifier, recursive-delete classifier, file-write guard, cmd-cost guard, `run_guard_hook` dispatch) — all sub-checks of the same "should this tool call be denied" decision | move-tests | ADR-0022 §5: no independent responsibility seam: every function feeds the single `run_guard_hook` decision. Prod (434) is already under 500; only the trailing 468-line test module pushes it over. | #113 |
| `crates/ralphy-core/src/verify.rs` | 709 | 476 | 233 | parse (`parse_verify`, `tokenize`); run (`run`/`run_one`/`spawn_command`/`resolve_program`); report (`comment`/`repair_brief`) | move-tests | Three loosely-named phases exist, but prod is 476 (under 500) and the phases share the `VerifyCommand`/outcome types tightly — no clean re-export boundary without inventing one. ADR-0022 §5 rules out a split motivated only by phase naming; moving tests removes the only thing pushing this over 500. | #114 |
| `crates/ralphy-cli/src/issues.rs` | 761 | 483 | 278 | one command (`ralphy issues`: list/show + json/text render) | move-tests | Single cohesive CLI command; prod under 500. | #115 |
| `crates/ralphy-cli/src/events/sink.rs` | 882 | 448 | 434 | (1) step polling (`StepPoller`, `parse_checkbox_steps`); (2) the `EventsLayer` tracing layer; (3) the `run_sender`/`deliver` retry loop | split | ADR-0022 §5: 3 genuinely independent seams — polling plan-step checkboxes, the tracing-subscriber shim, and the outbound delivery/retry loop don't share state or call each other directly; each is independently testable. | #116 |
| `crates/ralphy-cli/src/config.rs` | 662 | 314 | 348 | one cohesive `ralphy config` command (get/set/unset) | keep | Prod 314, well under 500; test-only overage; no responsibility seam beyond the single command. Moving tests optional (not requested by acceptance criteria); left as `keep`. | none |
| `crates/ralphy-core/src/blocked.rs` | 587 | 291 | 296 | (1) `#N` ref parsing; (2) `sort_queue`/Kahn topo-sort | keep | Two seams exist, but prod is only 291 lines — splitting a 291-line file to isolate a ~50-line parser from a ~100-line topo-sort adds two file boundaries for negligible readability gain. ADR-0022 §5 split is opt-in on *clear* gain; this doesn't clear that bar. | none |
| `crates/ralphy-cli/src/triage.rs` | 546 | 315 | 231 | one cohesive `ralphy triage` command | keep | Prod 315; single command, no seam. | none |
| `crates/ralphy-cli/src/usage.rs` | 519 | 307 | 212 | one cohesive `ralphy usage` command (usage/export) | keep | Prod 307, barely over 500 only via inline tests; single command, no seam. | none |

## Note on `crates/ralphy-adapter-support/src/lib.rs` and #111

[#111](https://github.com/paulocorcino/ralphy/issues/111) (open, blocked by #106)
lands five new shared helpers (D1–D5) into this same file before any split
would land. The split issue opened below (#112) is `## Blocked by #111` for
that reason — splitting first would make #111 land its helpers into whichever
child file a maintainer guesses, instead of the file #111 already targets.

## Verification of "no production change" (acceptance criterion 4)

```
$ git diff --name-only main..HEAD -- '*.rs'
(empty)
```

Confirmed: this issue's commits touch only `docs/` — no crate `.rs` file
changed. See `## Self-review findings` in `.ralphy/plan.md` for the adversarial
re-read.
