# Stage 11 — Daemon asset pins — Post-stage report

**Backlog items:** §3.2 LIES-FP (`:3564`, `:5122`, `:6328`, `:6900`), the §3.9 list of Rust pins made redundant, G-8, the KEEP defects `:1125` and `:2112`, `wb_session_owner_351.js`, `wb_fleet_label.js`, the stale comments
**Commit:** _filled by parent_
**Plan:** issue-485-test-suite-cleanup.md

Line numbers below are the audit's (tree at `63b6a930`); the tests are named.

## Files changed

- `crates/ralphy-daemon/src/tests.rs`
  - `security_state_reflects_the_stores` (`:1125`): re-reads the state after both flags are set.
  - `api_agents_serves_the_roster` (`:2112`): expected ids from `session::Agent::ALL` through `dispatch::agent_flag`.
  - `the_explorer_opens_a_note_as_a_card` (`:3564`): the `isNoteInNotesDir` definition and clause pins go; the wiring and `wb-notes.js` pins stay.
  - `the_changes_section_renders_a_status_marked_list` (`:5122`): markup pins only.
  - `a_detached_file_comes_home_when_its_popup_closes` (`:6328`): the popup half only.
  - `shell_navigates_the_plane` (`:6900`): the pan teardown and lost-mouseup pins are found inside each gesture's own body (whitespace removed).
  - `a_console_can_take_the_whole_screen` (`:7081`), `the_label_editor_is_unclipped_and_closed_under_a_live_run` (`:7762`), `a_refused_branch_change_reports_in_the_projects_panel` (`:8176`): loosened (see Evidence).
  - Fold lines dropped where a node test drives them: `the_release_badge_and_panel_are_pinned_in_the_served_assets` (`get releaseUnread()`), `monaco_replaced_codemirror_in_the_embedded_ui` (`function createOver(`), `the_discard_control_is_pinned_in_the_markup`, `the_workbench_never_titles_a_repo_with_its_routing_head`, `shell_draws_fences_below_the_windows` (incl. the cap refusal), `shell_drags_only_past_a_threshold`, `a_note_card_is_stacked_and_wears_the_console_chrome` (the look's closed sets), `shell_locks_consoles_and_fences`, `shell_fences_are_a_group`, `shell_arranges_into_the_fence`, `shell_lists_the_fences`, `shell_detaches_a_fence` (incl. the cap inside `detachFold`), `shell_survives_a_reload_with_its_detach`, `shell_has_no_clamp_and_carries_the_stage`, `shell_stores_only_the_view_in_the_browser`, `the_run_picker_names_the_model_and_clocks_the_phase` (the whole `wb-runs.js` half), `the_board_surfaces_the_plan_the_next_run_would_execute`, `the_runs_feed_is_contained_in_the_markup`, `a_refused_change_act_reports_in_the_changes_panel` (the per-act loop and helper pins), `a_remote_act_in_flight_locks_the_bar_and_shows_a_ring` (the `syncBusy` counts), `the_peer_wake_is_wired_through_the_ui_assets` (`stateFault`), `a_remoteless_project_is_labelled_by_its_directory` (keeps the `includes("/")` clause, see Surprises).
  - `workbench_session_assets_preserve_composite_repo_identity` (`:6779`): the `node --test` spawn, the definition pins and the name-fold pins go; the call-site pins stay.
  - Deleted: `the_plan_prose_is_keyed_to_the_issue_the_plan_names` (`:7562`; its `wb-runs.js` half is `wb-runs.test.mjs`'s, its `app.js` half is the new G-8 test and `planHeadings drops Steps…`), `the_tree_folder_predicate_reads_wunderbaums_data_bag` (G-8).
  - Doc comments that said the node suite does not run in CI, or that a fold pin was the only check, now name the node test that drives the fold (`presence_staleness_is_derived_on_a_clock_not_inside_the_binding` included).
- `crates/ralphy-daemon/ui-tests/app.test.mjs`: 5 new tests (G-8): `isFolder` (5 rows); the notes carve-out through `showMenu` (7 rows); a refused act lands in `changesError` and still flashes, for all 7 Changes acts; the `syncBusy` slot; `planProseIsCurrent`/`renderPlanSection`.
- `crates/ralphy-daemon/ui-tests/wb-session-route.test.mjs`: the 5 tests of `tests/wb_session_owner_351.js`, case by case (names kept; `PEER_REPO` moved with them).
- `crates/ralphy-daemon/tests/wb_session_owner_351.js`, `crates/ralphy-daemon/tests/wb_fleet_label.js`: deleted.
- `docs/audit-tests-2026-09-27.md`: 15 §7 rows.

## Evidence

Rule 1 (LIES-FP). "Before fix" ran the old test at `HEAD`; "after fix" the named test; each with the mutation applied.
- §3.2 :3564: mutation app.js `isNoteInNotesDir`, `parts.length === 3 &&` → `||` | before fix: PASS | after fix: FAIL (`app.test.mjs` "the Move item is withheld…") | reverted: PASS
- §3.2 :5122: mutation wb-changes.js `MARKS`, `added: "A"` deleted | before fix: PASS | after fix: FAIL (`wb-changes.test.mjs` "the list model carries a status marker per row") | reverted: PASS
- §3.2 :6328: mutation app.js `pollDetached`, `reattachFile(win, desc)` → `detachedWindows.delete(win)` | before fix: PASS | after fix: FAIL (`wb-detach-file.test.mjs` "a popup that dies without a message comes home…") | reverted: PASS
- §3.2 :6900: mutation wb-console.js `makeDraggable` `stopPan`, `if (panRaf != null) cancelAnimationFrame(panRaf);` deleted | before fix: PASS | after fix: FAIL (`shell_navigates_the_plane`) | reverted: PASS
- §3.2 :1125: mutation routes/api_security/store.rs `token_set: … .is_some() || auth::require_login_enabled_in(dir)` | before fix: PASS | after fix: FAIL | reverted: PASS
- §3.2 :2112: mutation roster.rs `id: agent_flag(a)` → `agent_flag(Agent::Claude)` | after fix: FAIL | reverted: PASS. No "before fix" PASS exists: see Deviations.

Rule 4 (LIES-FN), harmless edit then real mutation, both on the new test:
- :7081: harmless, `startResize` guard operands reversed | PASS; real, `|| isFull(win)` dropped from `startResize` | FAIL | reverted: PASS
- :7762: harmless, `labelLockReason` call re-laid with a local `run` | PASS; real, `labelLockReason` returns its own sentence without `writeLockReason` | FAIL | reverted: PASS
- :8176: harmless, a comment naming `_branchRefused(msg)` (the old count 3 becomes 4) | PASS; real, the throw-arm `_branchRefused(…)` in `_mutateBranch` deleted | FAIL | reverted: PASS

Rule 5 (G-8), each on `app.test.mjs`:
- G-8 isFolder: mutation app.js `isFolder`, `node.data?.folder` → `node.folder` | FAIL ("isFolder reads Wunderbaum's data bag…", and "emitCreate sends the directory…") | reverted: PASS
- G-8 isNoteInNotesDir: the :3564 mutation above | FAIL | reverted: PASS
- G-8 _changesRefused: mutation app.js `syncPush` refusal arm `this._changesRefused(` → `this._flashAction(` | FAIL; mutation `_changesRefused` without `this._flashAction(msg)` | FAIL | reverted: PASS
- G-8 syncBusy: mutation app.js `syncPull` without `if (this.syncBusy) return;` | FAIL | reverted: PASS
- G-8 planProseIsCurrent: mutation app.js `renderPlanSection` without `!this.planProseIsCurrent(run)` | FAIL | reverted: PASS

Move of `wb_session_owner_351.js` (5 cases → 5 tests): mutation wb-session-route.js `matchesRepo` also accepts a ref ending in `/<slug>` | FAIL ("a local slug never marks the peer composite repo live") | reverted: PASS.
Delete of `wb_fleet_label.js` (never run): mutation wb-fleet.js `refLabel` without `!isPeerRef(ref)` | FAIL (`wb-fleet.test.mjs` "refLabel appends the environment only for a peer") | reverted: PASS.

§3.9 trimmed pins, one fold mutation per group, the node test that owns the removed lines FAILS (all reverted: PASS):
- `:1934` `releaseUnread` ignores `releaseSeen` → app "loadRelease shows a newer release again…"
- `:5033` `createOver` drops `model` → wb-monaco "createOver puts a second editor over the SAME model…"
- `:5122` see above; `:5266` `discardConfirm` `unrecoverable: false` → wb-changes "discardConfirm is more emphatic…"
- `:5371` see `wb_fleet_label.js` above
- `:5435` `createFence` cap `if (atFenceCap() && false)` → wb-console "the fence cap is a number…"
- `:5622` `DRAG_THRESHOLD.mouse` 5 → wb-console "dragThreshold is 4px for a mouse…"
- `:5704` `withStyle` writes a default fill → wb-notes "the front-matter block has exactly one shape per look"
- `:5814` `fenceOf` ignores `rectHolds` → wb-geometry "fenceOf answers the fence holding the rect's centre…"
- `:5912` `fenceFits` compares a fence with itself → wb-geometry "fenceFits: a fence compared against ITSELF…"
- `:5994` `tileIntoRect` `cols = n` → wb-geometry "tileIntoRect: three members take a 2x2 grid…"
- `:6098` `fenceSummaries` `repos: []` → wb-console "fenceSummaries: two members read their count and their repos…"
- `:6359` `detachFold` `>=` → `>` → wb-console "detachFold: the cap is four…"
- `:6636` `peerFold` loss not terminal → wb-console "peerFold: loss is terminal…"
- `:6779` see the move above; the name fold: `announcement` drops `payload?.name` → wb-session-route "the vendor session name arrives…"; `sessionPresentation` passes `null` for the name → wb-console "sessionPresentation puts the full ref, the environment and the name in the tooltip"
- `:6854` `stageExtent` ignores width → wb-geometry "stageExtent: a window well inside the viewport…"
- `:6900` `panNudge` uncapped → wb-console "panNudge: PAST the right edge is capped…"
- `:7185` `viewLanding` does not clamp → wb-console "viewLanding: a stored offset past the extent is clamped…"
- `:7562` `planHeadings` without the prose gate → app "planHeadings drops Steps…"; `renderPlanSection` see G-8
- `:7600` `fmtClock` without `padStart` → wb-runs "fmtClock mirrors ui::render::fmt_clock"; `fromSnapshot` `model: null` → wb-runs "fromSnapshot carries the render facts…" and "runTitle names the model…"; `budget >= 0` → wb-runs "phaseClock counts from the document's anchor…"
- `:7762` `writeLockReason` sentence shortened → wb-changes "writeLockReason speaks only when a run holds the lock"
- `:7844` `infeasible: false` → wb-runs "a plan with no open steps is infeasible…"
- `:8110` see G-8 syncBusy
- `:8696` `stateFault` drops the `asleep` exemption → wb-fleet "the state glyph…"
- `:8996` `repoLabel` keeps trailing separators → wb-project "repoLabel re-labels only a remoteless repo…"

## Gate results

- `npx oxlint@1.85.0 --deny-warnings crates/ralphy-daemon/assets/ui crates/ralphy-daemon/ui-tests` (the CI step, through npx): exit 0.
- `PYTHONIOENCODING=utf-8 python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ui --ids G-8`: 8/8 checks passed (§7 row for G-8, fmt, clippy, nextest, doctests, changelog check, node ui-tests, ui-copy).

## Acceptance criteria audit

- [x] `crates/ralphy-daemon/tests/wb_session_owner_351.js` and `wb_fleet_label.js` are absent.
- [x] `grep -n 'wb_session_owner_351' crates/ralphy-daemon/src/tests.rs` returns nothing.
- [x] `grep -rn 'only gate in CI' crates/ralphy-daemon/src` returns nothing.
- [x] Each §3.2 LIES-FP row passed its mutation before the fix and fails after it.
- [x] Each G-8 function has a node test that fails under a mutation; the Rust substring pins are gone.
- [x] Markup, CSS and call-site pins are kept; every test under "Keep as they are" is untouched.
- [x] No asset or production file is in this commit.

## Deviations from plan

- `:2112`: the plan's rule-1 mutation ("the hard-coded list passes when a vendor is missing from `ALL`") does not hold. With a vendor missing from `ALL`, the served roster is 6 rows and the old 7-name list fails; the new set, built from `ALL`, follows it. The change is still made because the written vendor list breaks the repo rule. The evidence is a real mutation the new test catches (`roster_with` serving one id). Removing a vendor from `ALL` needs `[Agent; 6]`, so it is not a harmless edit either.
- `:8996`: the `includes("/")` needle is kept. Removing `!p.slug.includes("/")` from `repoLabel` passes every `wb-project.test.mjs` row (no row has an owner that starts with `path-`), so dropping the needle would lose a guarded case. `wb-project.test.mjs` is not in this stage's file list; a row there would let the needle go.
- `:8266` (`the_worktree_row_remove_action_stops_the_selecting_click`) and `:9097` (`the_branch_chip_carries_the_change_count_on_the_project_row`) are kept as they are: no node test reaches `checkoutMenu`'s trash handler, `removeWorktree`'s confirm order, `rowTitle` or `branchChipClick`.
- `:8663` needed nothing: stage 4 already left only the export and the filter call.
- The G-8 `modeFor` half is left to stage 12, as the stage block says.

## Surprises / notes

- `n_discard`: the first `discardConfirm` mutation tried (`confirmLabel: "Delete permanently"` → `"Discard"`) survived `wb-changes.test.mjs`, which only asserts the two labels differ. The `unrecoverable` mutation is what the report uses. Not changed here (outside this stage's file list).
- The mutation helper could not undo a deletion whose remaining text is not unique; twice a revert was done by hand with an exact inverse edit (`wb-changes.js` `added: "A"`, `app.js` `_mutateBranch` throw arm). `git diff --stat -- crates/ralphy-daemon/assets` was empty after each.
- `wb-geometry.test.mjs`'s header note from stage 8 is still not reworded (not in this stage's file list).
