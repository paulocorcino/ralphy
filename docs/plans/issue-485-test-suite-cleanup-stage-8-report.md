# Stage 8 — Fix the remaining LIES-FP tests — Post-stage report

**Backlog items:** A-1, A-5, A-7, A-8, D-1, D-2, D-3, D-4, D-5, D-6, D-7, C-A4, C-A6, C-A7, C-A12, C-A14, C-B6, C-B7, C-B8, C-B9, C-B10, S-1, S-2, S-3, S-5, S-6, S-7, S-8, S-9, S-10, S-11, J-1, J-4, J-5, J-6, J-9
**Commit:** _filled by parent_
**Plan:** issue-485-test-suite-cleanup.md

## Files changed

Test code only. Every change in a production file is inside its `#[cfg(test)] mod tests` block (checked: each file's first diff hunk is below the test module line).

- `crates/ralphy-adapter-support/src/assets.rs` (test module) — A-7.
- `crates/ralphy-agent-gemini/src/tasks.rs` (test module) — A-5.
- `crates/ralphy-agent-opencode/src/events.rs` (test module) — A-8.
- `crates/ralphy-daemon/src/autostart/tests.rs` — D-1, D-2.
- `crates/ralphy-daemon/src/peer/nudge/tests.rs` — D-3.
- `crates/ralphy-daemon/src/totp.rs` (test module) — D-4.
- `crates/ralphy-daemon/src/usage/tests.rs` — D-6.
- `crates/ralphy-daemon/ui-tests/wb-spend.test.mjs` — D-7 (new `floorNote` test).
- `crates/ralphy-cli/src/runstate/snapshot.rs` (test module) — C-A4.
- `crates/ralphy-cli/src/runstate/state/tests.rs` — C-A6.
- `crates/ralphy-cli/src/init/gate.rs` (test module) — C-A7.
- `crates/ralphy-cli/src/events/sink/poller.rs` (test module) — C-A12.
- `crates/ralphy-cli/src/ui/tests.rs` — C-A14.
- `crates/ralphy-cli/src/telegram/notifier/render/tests.rs` — C-B7.
- `crates/ralphy-cli/src/update.rs` (test module) — C-B8 (test deleted).
- `crates/ralphy-cli/src/usage.rs` (test module) — C-B9.
- `crates/ralphy-cli/src/triage/tests.rs` — C-B10.
- `crates/ralphy-proc-util/src/tests.rs` — S-1.
- `crates/ralphy-proc-util/src/pid.rs` (test module) — S-2.
- `crates/ralphy-proc-util/src/cursor.rs` (test module) — S-3.
- `crates/ralphy-pricing/src/fetch.rs` (test module) — S-5.
- `crates/ralphy-run-snapshot/src/read.rs` (test module) — S-6.
- `crates/ralphy-usage-scan/src/recovery.rs` (test module) — S-8.
- `crates/ralphy-usage-scan/src/claude.rs` (test module) — S-9.
- `crates/xtask/src/main.rs` (test module) — S-10.
- `crates/xtask/src/ui_copy/tests.rs` — S-11.
- `crates/ralphy-daemon/ui-tests/wb-encoding.test.mjs` — J-1.
- `crates/ralphy-daemon/ui-tests/wb-console.test.mjs` — J-5; J-9 (duplicate `TILES` table removed).
- `crates/ralphy-daemon/ui-tests/wb-runs.test.mjs` — J-6.
- `crates/ralphy-daemon/ui-tests/wb-geometry.test.mjs` — J-9 (exact table, comment moved in).
- `docs/plans/issue-485-followup-draft.md` — sections for A-1 and J-4.
- `docs/audit-tests-2026-09-27.md` — §7 rows for all 36 IDs.

## Evidence

Format: `<ID>: mutation <file, what changed> | before fix: PASS | after fix: FAIL | reverted: PASS`. Each run targeted one test (`cargo nextest run -p <crate> -E 'test(<name>)'` or `node --test <file>`).

- A-5: mutation gemini `tasks.rs` `triage_issues`, `attachment_dirs(req.image_paths)` -> `&[]` | before fix: PASS | after fix: FAIL | reverted: PASS
- A-7: mutation adapter-support `assets.rs` `materialize_assets`, `"*\n"` -> `"*.log\n"` | before fix: PASS | after fix: FAIL | reverted: PASS
- A-8: mutation opencode `events.rs` `parse_opencode_limit`, hint `.or_else(|| Some(msg.clone()))` | before fix: PASS | after fix: FAIL | reverted: PASS
- D-1: mutation daemon `autostart.rs` Run value, `'{exe}' daemon *>>` -> `'{exe}' *>>` | before fix: PASS (both tests) | after fix: FAIL (both) | reverted: PASS
- D-2: mutation daemon `autostart.rs` `systemd_unit`, `ExecStart={} daemon` -> `ExecStart={}` | before fix: PASS | after fix: FAIL | reverted: PASS
- D-3: mutation daemon `peer/nudge.rs` `is_distro_running`, `--running` dropped from the argv | before fix: PASS | after fix: FAIL | reverted: PASS
- D-4: mutation daemon `totp.rs` `secret_base32`, `BASE32_NOPAD` -> `HEXUPPER` | before fix: PASS | after fix: FAIL | reverted: PASS
- D-6: mutation daemon `usage.rs` `interactive_records`, kimi scan and its `.chain` commented out | before fix: PASS | after fix: FAIL | reverted: PASS
- D-7: mutation daemon `wb-spend.js` `floorNote`, always `return ""` | before fix: PASS (Rust pin, and all 7 wb-spend tests) | after fix: FAIL (new JS test) | reverted: PASS
- C-A4: mutation `wb-runs.js` `LABEL`, `blocked` entry deleted | before fix: PASS | after fix: FAIL | reverted: PASS
- C-A6: mutation cli `runstate/state.rs`, `deadline reached before #N` -> `stopped before #N` | before fix: PASS | after fix: FAIL | reverted: PASS
- C-A7: mutation cli `init/gate.rs` `Agent::ALL`, Claude and Codex swapped | before fix: PASS | after fix: FAIL | reverted: PASS
- C-A12: mutation cli `events/sink/poller.rs` `reset_from_written` body emptied | before fix: PASS (first test) | after fix: the claim is trimmed; the sibling `a_fold_seeded_baseline_suppresses_an_already_checked_step` FAILS | reverted: PASS
- C-A14 (queue bar): mutation cli `ui.rs` `queue_bar_label`, pending list not cut | before fix: PASS (60-column and 10-column tests) | after fix: FAIL | reverted: PASS
- C-A14 (active line): mutation cli `ui/render/line.rs`, title left whole when `width <= overhead` | before fix: PASS | after fix: FAIL | reverted: PASS
- C-B7: mutation cli `runstate.rs` `header_face`, always `HEADER_FACES[0]` | before fix: PASS | after fix: FAIL | reverted: PASS
- C-B8 (deleted): mutation `ralphy-release/src/lib.rs` `standing`, `v > current` -> `v >= current` | cli test: PASS, also with the report's fix applied | covering `ralphy-release` `the_newest_build_is_level`: FAIL | reverted: PASS
- C-B9: mutation cli `usage.rs` `render_table`, `usd_for_rows(&all[..1], …)` | before fix: PASS | after fix: FAIL | reverted: PASS
- C-B10: mutation cli `triage.rs` `presence_gate`, HeldAlive and Stale warnings swapped | before fix: PASS (both) | after fix: FAIL (both) | reverted: PASS
- S-1: mutation proc-util `lib.rs` `locate_program_with`, `~/.local/bin` checked before PATH | before fix: PASS | after fix: FAIL | reverted: PASS
- S-2: mutation proc-util `pid.rs` Windows `pid_is_alive`, `return true` for any pid | before fix: PASS | after fix: FAIL | reverted: PASS
- S-3: mutation proc-util `cursor.rs` `indexing_gate`, no repository -> `work_dir` used as a root | before fix: PASS | after fix: FAIL | reverted: PASS
- S-5: mutation pricing `lib.rs` `PriceTable::load`, parsed cache dropped | before fix: PASS | after fix: FAIL (`110.25` vs `36.75`) | reverted: PASS
- S-6: mutation run-snapshot `read.rs` `list_runs`, sort by runid only | before fix: PASS | after fix: FAIL | reverted: PASS
- S-7 (fixed in Stage 7): mutation run-snapshot `document.rs` `snapshot_dir`, `runstate` -> `runs` | current test: FAIL | reverted: PASS
- S-8: mutation usage-scan `recovery.rs` `resolve_models`, opencode candidate filtered out | before fix: PASS | after fix: FAIL | reverted: PASS
- S-9: mutation usage-scan `claude.rs` `scan_claude`, unmatched workspace falls back to `keys.first()` | before fix: PASS | after fix: FAIL | reverted: PASS
- S-10: mutation xtask `main.rs` `refresh`, providers emitted in input order | before fix: PASS | after fix: FAIL | reverted: PASS
- S-11: mutation xtask `ui_copy/html.rs` `scan`, text before a tag with no `>` after it dropped | before fix: PASS | after fix: FAIL | reverted: PASS
- J-1: mutation `wb-viewer.js` `saveFailed`, `rec.dirty = true` and the class add deleted | before fix: PASS | after fix: FAIL | reverted: PASS
- J-5: mutation `wb-console.js` `atFenceCap`, never at the cap | before fix: PASS | after fix: FAIL | reverted: PASS
- J-6: mutation `wb-runs.js` `exitNote`, `Could not ${verb}` -> `Could not run` | before fix: PASS (both) | after fix: FAIL (both) | reverted: PASS
- J-9: mutation `wb-geometry.js` `tileIntoRect`, every tile in the first column | before fix: PASS (geometry relation test) | after fix: FAIL (8 of 10 rows) | reverted: PASS
- A-1 (follow-up): mutation codex `skills.rs`, stale clear (`remove_path`) removed | PASS; no test-only fix exists
- J-4 (follow-up): mutation `wb-console.js` `setStaleProbe` made a no-op | PASS; no test-only fix exists

## Gate results

- Per-item red runs: targeted `cargo nextest run -p <crate> -E 'test(<name>)'` and `node --test <file>`, listed above.
- `cargo fmt --all --check`: pass (after `cargo fmt -p xtask` for one long line).
- `oxlint 1.85.0 --deny-warnings crates/ralphy-daemon/assets/ui crates/ralphy-daemon/ui-tests` (via `npx`): exit 0.
- Full gate `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ui --ids <all 36>`: 8/8 checks passed (§7 rows for 36 IDs, fmt, clippy `--all-targets -D warnings`, nextest workspace, doctests, changelog check, `node --test ui-tests`, `ui-copy --check`). Run once, with every mutation reverted.

## Acceptance criteria audit

- [x] Every ID has a §7 row (checked by `--ids`).
- [x] Every `fixed` ID has an evidence line.
- [x] Each mutation was reverted by an exact inverse edit, right after its run; `git diff` shows no production change.
- [x] D-5 and C-B6 are `kept` with the caveat, no change.
- [x] A-1 went to follow-up and has a section in `docs/plans/issue-485-followup-draft.md`.

## Deviations from plan

- **C-B8 deleted, not fixed.** The report's fix (pass a release with the build's tag) stays green under the inverted comparator: `Build::parse(env!("RALPHY_VERSION"))` is `ahead` on every dev and CI build (`v0.1.0-rc.28-129-g…`), and `standing` returns `Ahead` before it compares. Seen: the fixed variant passed under the mutation. The test also depends on the host tree. `ralphy-release` `the_newest_build_is_level` covers the comparator and fails under the mutation.
- **J-4 went to follow-up** (the plan named only A-1/A-5 as possible follow-ups). The probe is read only by the private `isStale`, and no window can join `wins` in the node harness, so no test-only change can see the probe. The section is in the follow-up draft.
- **C-A14, second half:** the report's `display_width <= 10` is false for `render_active_line`, whose tail (model and clock) is never cut. The test asserts the exact line instead.
- **C-A12 and S-5 (first half) are trims:** the part of the test that cannot fail was removed; the covering test is named in §7.
- **D-7** adds a JS behavior test for `floorNote`, which also closes the `floorNote` half of G-9.
- **J-6** changes the verb of the first test from `run` to `triage`: with `run`, an exact assert cannot see a hard-coded `run`.
- **S-7** was already fixed in Stage 7; its row keeps `stage 7` in the Commit column.

## Surprises / notes

- The `wb-geometry.test.mjs` header still says no assertion was rewritten in the extraction; that is now not true for `tileIntoRect`. Left as is (outside this item's fix); Stage 11 or 12 may reword it.
- Every edit kept each file's own line endings (some working-tree files are LF, most CRLF); git normalizes on commit.
