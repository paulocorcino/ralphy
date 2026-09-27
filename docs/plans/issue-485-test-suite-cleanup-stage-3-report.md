# Stage 3 — P0 source-cut scans — Post-stage report

**Backlog items:** P0-2, P0-2 siblings
**Commit:** _filled by parent_
**Plan:** issue-485-test-suite-cleanup.md

## Files changed
- `crates/ralphy-agent-gemini/src/tests.rs`, `crates/ralphy-agent-cursor/src/tests.rs`,
  `crates/ralphy-agent-copilot/src/tests.rs` - new `pub(crate) fn production_text(src: &str) -> &str`.
  It returns the text before the first `#[cfg(test)]` line whose next non-empty line starts
  with `mod `, the rule of `production()` in `crates/xtask/tests/user_text_cites_no_adr.rs`.
  Two unit tests per crate: `production_text_reads_past_a_test_item` (CRLF input, a blank line
  between the attribute and `mod tests;`) and `production_text_drops_the_test_module` (also:
  a file without a test module is read whole). In gemini, `execute_is_plan_agnostic_and_bounds_the_commit`
  now uses `production_text` instead of `.split("\nmod tests {")`, which never matched
  (`lib.rs` has `mod tests;`) and read the whole file under a comment that said otherwise.
- gemini: `src/auth.rs` (2 cuts), `src/command/tests.rs` (3 cuts, plus the whole-file `lib.rs`
  read in `the_child_is_pointed_at_the_owned_root_and_never_the_operators`, whose comment
  explained the old workaround and is removed), `src/outcome/tests.rs` (1), `src/revocation.rs` (2),
  `src/root.rs` (1), `src/tasks.rs` (3) - each cut now calls `crate::tests::production_text`.
- cursor: `src/tests.rs` (1), `src/auth.rs` (1), `src/command/tests.rs` (2), `src/model.rs` (1),
  `src/outcome/tests.rs` (2) - the same replacement.
- copilot: `src/outcome.rs` (1) - the same replacement.
- `docs/audit-tests-2026-09-27.md` §7 - rows `P0-2` and `P0-2 siblings`.

Changes in `auth.rs`, `revocation.rs`, `root.rs`, `tasks.rs`, `model.rs` (cursor) and
`outcome.rs` (copilot) are all inside the file's `#[cfg(test)] mod tests` block. No
production line changed.

## Evidence
Step 1 (blind files): in all three crates the only file with an item-level `#[cfg(test)]`
above its test module is `lib.rs` (`fn issue_deadline`: gemini :163, cursor :182, copilot :234).
So a cut was blind exactly when its scan read `lib.rs`. Copilot has no scan that reads `lib.rs`.

- P0-2 gemini: mutation `ralphy-agent-gemini/src/lib.rs` `fn plan`, added `let _creds = std::path::Path::new("oauth_creds.json");` | before fix: PASS | after fix: FAIL (`the_auth_probe_reads_no_credential`) | reverted: PASS
- P0-2 gemini: mutation `ralphy-agent-gemini/src/lib.rs` `fn execute`, added `let _argv = ["--approval-mode", "auto_edit"];` | before fix: PASS | after fix: FAIL (`autonomy_argv_is_never_downgraded`) | reverted: PASS
- P0-2 cursor: mutation `ralphy-agent-cursor/src/lib.rs` `fn execute`, wrapped `let outcome: Outcome = if outcome::cursor_limit_note(..)` in `loop { break … }` | before fix: PASS | after fix: FAIL (`no_adapter_side_retry_of_a_quota_stop`) | reverted: PASS
- P0-2 sibling cursor `model.rs`: mutation `ralphy-agent-cursor/src/lib.rs` `fn execute`, added `let _m = "composer-2.5";` | before fix: PASS | after fix: FAIL (`no_default_model_id_is_baked_in`) | reverted: PASS
- P0-2 sibling cursor `outcome/tests.rs`: mutation `ralphy-agent-cursor/src/lib.rs` `fn execute`, added `let _c = std::process::Command::new("x");` | before fix: PASS | after fix: FAIL (`every_spawn_site_in_the_crate_is_gated_or_neutralized`) | reverted: PASS

Each "after fix" run was the full suite of the three adapter crates (309 tests) under that one
mutation. In every case exactly one test failed: the named one.

The helper's own tests (gemini, test code): changing `&& lines…` to `|| lines…` and changing
`return &src[..offset]` to `return src` each make both `production_text_*` tests fail.

Step 6: on the clean tree all 309 tests of the three crates pass. The newly visible part of
each `lib.rs` has no banned word (checked by grep before the fix), so no scan found a real hit.

## Gate results
`python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ids P0-2`: 6/6 checks passed
(§7 row for P0-2, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo nextest run --workspace`, `cargo test --workspace --doc`, `xtask changelog --check`).

## Acceptance criteria audit
- [x] `production_text` plus two unit tests in each of the three crates.
- [x] Every `.split("#[cfg(test)]")` cut replaced: `grep -rn 'split("#\[cfg(test)\]")' crates/ralphy-agent-gemini crates/ralphy-agent-cursor crates/ralphy-agent-copilot` returns no match.
- [x] Three P0-2 evidence lines and one per blind sibling site (2) are in the commit body.
- [x] §7 rows `P0-2` and `P0-2 siblings`, with "was blind: yes/no" for every site.
- [x] No production line changed; all mutations reverted.

## Deviations from plan
- The plan listed gemini `auth.rs` and `command/tests.rs` cuts, but not the two whole-file
  `lib.rs` reads (gemini `src/tests.rs` `execute_is_plan_agnostic_and_bounds_the_commit` and
  gemini `command/tests.rs` `the_child_is_pointed_at_the_owned_root_and_never_the_operators`).
  Both files are in the declared list. Both reads now use `production_text`, and the comments
  that explained the old workaround are removed. Neither read was blind; the text they see is
  the same except for the trailing `#[cfg(test)] mod tests;`.
- The plan listed cursor `src/outcome/tests.rs` sites :493 and :593; :493 is the walker in
  `every_spawn_site_in_the_crate_is_gated_or_neutralized`, which was blind (it reads `lib.rs`).

## Surprises / notes
- The working tree checks out with CRLF on Windows (`core.autocrlf=true`). `production_text`
  compares `line.trim()`, so it handles CRLF; one unit test uses CRLF input.
- Copilot `lib.rs` also has an item-level `#[cfg(test)] fn issue_deadline` (:234), but no scan
  in copilot reads `lib.rs`, so nothing there was blind.
