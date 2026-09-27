# Stage 2 — P0 security and watch tests — Post-stage report

**Backlog items:** P0-1, P0-3, P0-4, P0-5, P0-6
**Commit:** _filled by parent_
**Plan:** issue-485-test-suite-cleanup.md

## Files changed
- `crates/ralphy-daemon/src/tree.rs` (test module only) — P0-1: `read_masks_escape_as_not_found` and `read_image_masks_escape_as_not_found` build `outer/root`, write a real `outer/secret` and a real `outer/secret.png` (`magic(ImageType::Png)`), and read `../secret` / `../secret.png` from the root.
- `crates/ralphy-daemon/src/note.rs` (test module only) — P0-1: `read_masks_a_missing_or_escaping_target_as_a_miss` writes a valid note (`encode("# secret")`) at `outer/outside.note`.
- `crates/ralphy-daemon/tests/observe_read.rs` — P0-1: `serve_repo` (every caller) now registers `outer/repo` and writes a real `outer/secret` and `outer/secret.png`. The in-repo content is unchanged.
- `crates/ralphy-daemon/tests/child_env_hygiene.rs` — P0-3: doc comment reworded. It says the test proves the strip works, and that it does not prove the boot calls it.
- `crates/ralphy-proc-util/src/cursor.rs` (test module only) — P0-4: `the_gate_writes_nothing_when_already_protected` and the inner-opt-out assert in `indexing_gate_creates_the_optout_in_every_enclosing_repository` seed `"# operator\nsecrets/\n"` and assert the bytes are unchanged.
- `crates/xtask/tests/user_text_cites_no_adr.rs` — P0-5: new test `production_cuts_at_the_test_module_and_not_at_a_test_item` (item-level `#[cfg(test)] fn` before production code; a text with no test module is read whole).
- `crates/ralphy-daemon/tests/runs_watch.rs` — P0-6: positive control after the negative window in `runs_unwatch_stops_the_pushes` (re-send `runs.watch`, write a third snapshot, `recv_verb` returns `Some(slug)`).
- `docs/audit-tests-2026-09-27.md` — §7 rows for P0-1, P0-3, P0-4, P0-5, P0-6.

## Evidence
- P0-1 tree: mutation `tree.rs:154` and `:328`, `confine::confine(root, rel).map_err(..)?` -> `Ok(root.join(rel))?` | before fix: PASS | after fix: FAIL (`Ok("token")`, `Ok(Image { .. })`) | reverted: PASS
- P0-1 note: mutation `note.rs:207`, same replacement | before fix: PASS | after fix: FAIL (`Ok("# secret")`) | reverted: PASS
- P0-1 observe_read: mutation `tree.rs:154` and `:328` (the wire path) | before fix: PASS (both tests) | after fix: FAIL (`status` was `"ok"`; file read had no `reason`) | reverted: PASS (all 17 tests of the binary)
- P0-4: mutation `cursor.rs:125`, `.filter(|r| !r.join(OPT_OUT_FILE).exists())` removed | before fix: PASS (both tests) | after fix: FAIL (`"*\n"` != operator body) | reverted: PASS
- P0-5 (a): mutation `user_text_cites_no_adr.rs` `production()` body -> `""` | before fix: PASS (no test of the cut existed; the gate test stays green) | after fix: FAIL | reverted: PASS
- P0-5 (b): mutation `production()` cuts at the first `#[cfg(test)]` line (the `mod ` look-ahead removed) | before fix: PASS | after fix: FAIL (`"use a;\n"`) | reverted: PASS
- P0-6: mutation `routes/ws_tree.rs` `"unwatch" | "runs.unwatch"` arm, `if runs { break; }` at its top (closes the socket instead of releasing) | before fix: PASS (1.3 s: the close returned `None` at once) | after fix: FAIL (the re-send of `runs.watch` to the closed socket panics) | reverted: PASS
- P0-3: no mutation. The test calls the strip itself; reaching the boot path needs a production seam. Status `follow-up`.

## Gate results
`python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ids P0-1,P0-3,P0-4,P0-5,P0-6`:
6/6 checks passed: section 7 rows for 5 IDs, fmt, clippy (--all-targets -D warnings), nextest (workspace), doctests, changelog --check. Logs under docs/plans/logs/gate-stage-x-*-20260927-15*.log.

## Acceptance criteria audit
- [x] P0-1 tree, image, note and observe_read tests fail against the confinement mutation, pass after revert.
- [x] P0-3 doc comment no longer claims to prove the boot-time strip; §7 status `follow-up`.
- [x] P0-4 tests fail when the operator's opt-out is overwritten.
- [x] P0-5 new test fails for both named mutations.
- [x] P0-6 test fails when `runs.unwatch` closes the socket.
- [x] §7 rows for all five IDs.
- [x] Every production mutation reverted; production diffs are inside `#[cfg(test)]` modules only (`tree.rs`, `note.rs`, `cursor.rs`). `routes/ws_tree.rs` has no diff.

## Deviations from plan
- `observe_read.rs`: `serve_repo` was changed for every caller (the plan allowed a variant or a change for all). `repos.toml` stays inside the repo dir, as before, so the other tests see the same tree.
- P0-6: the fixed test fails at the positive control's `send_verb` (`.unwrap()` on a send to a closed socket), not at the `recv_verb` assert. Both are the positive control failing; the test turns red either way.

## Surprises / notes
- A quoted bash heredoc changed `\n` inside Python string literals, so edit scripts were written with the Write tool.
- Files changed by another session during this stage and NOT staged: `AGENTS.md` (modified), `docs/TESTING.md` (untracked).
