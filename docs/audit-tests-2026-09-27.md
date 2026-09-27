# Audit: test-suite quality and relevance — 2026-09-27

A quality audit of every test in the workspace, measured against the `/tdd`
criteria: a good test checks behavior through a public interface, survives an
internal refactor, and mocks only at a system boundary. The audit was requested
before any cleanup, so that no finding is lost between sessions.

**Verdict up front:** most of the suite is sound. The queue suite in core, the
vendor parsers and folds, the auth and cookie tests, and the JS folds under
`node --test` test real behavior with external oracles. About 75–80% of the tests
should stay as they are. The problem is not volume. The problem is about 90 tests
that pass when the behavior they name is broken, and several of them sit on
security guarantees (§2). After those, the suite has about 500 tests that repeat
another test, test a library instead of Ralphy, or pin text that a harmless edit
breaks (§3–§5).

---

## 1. Method and limits

- 9 read-only reviewers, one per slice. Each read every test name, and read the
  body plus the production code for every non-KEEP verdict.
- Verdicts: **KEEP**, **MERGE** (same path with trivially different inputs, or a
  strict subset of another test), **LIES-FP** (passes when the behavior is
  broken), **LIES-FN** (fails when the behavior is fine), **DELETE** (tests a
  library, a constant, prose, or duplicates a better test).
- A test is only called LIES-FP when a concrete production mutation that it does
  not catch is named. **No mutation was executed.** Every mutation below is
  reasoned from the code. 12 claims were re-checked by hand after the reviewers
  finished; 11 held, 1 was overstated (noted at C-B6).
- Line numbers are from the tree at commit `63b6a930` (branch
  `feature/console-name`).

### Size of the suite

| Slice | Tests | LIES-FP | LIES-FN | Proposed change |
|---|---|---|---|---|
| `ralphy-adapter-support` + 7 `ralphy-agent-*` | 652 | ~24 | ~27 | about −150 |
| `ralphy-daemon/src/tests.rs` | 171 | 10 | 29 | −22, and about half the assertions of 39 pin tests |
| `ralphy-daemon/src/*` (other modules) | 490 | 10 | 10 | about −35 |
| `ralphy-daemon/tests/*.rs` | 155 in 55 binaries | 2 | 2 groups | 55 → ~26 binaries, −6 tests |
| `ralphy-cli` (runstate, init, events, ui) | 287 | 15 | 9 | 287 → ~215 |
| `ralphy-cli` (the rest) | 419 | 17 | 13 | 419 → ~300, 9 → 1 integration binary |
| `ralphy-core` | 466 | 3 | ~7 | about −50, −3 binaries |
| small crates + `xtask` | 334 | 15 | 3 | about −25 |
| JS `crates/ralphy-daemon/ui-tests` | 520 call sites (630 at run time) | 9 | 7 | → ~440 |

Rust: about 2,970 → 2,480 test functions (−16%). No coverage is lost; most of
the cut is table-driving of near-identical tests and deletion of tests that test
std, serde, clap or a constant.

`node --test crates/ralphy-daemon/ui-tests` passed during the audit: 630 pass,
0 fail, 4.9 s.

---

## 2. P0 — tests that lie about security or correctness

Fix these; do not delete them. Each fix is 1–5 lines.

| ID | Where | What lies | Mutation that survives today |
|---|---|---|---|
| P0-1 | `ralphy-daemon/src/tree.rs:609` `read_masks_escape_as_not_found`; `tree.rs:578` `read_image_masks_escape…`; `note.rs:460` `read_masks_a_missing_or_escaping_target…`; `ralphy-daemon/tests/observe_read.rs:306` `image_read_masks_traversal_as_not_found`, `:326` `file_read_masks_traversal_as_not_found` | They read `../secret`, but no `secret` file exists outside the root (`serve_repo` never writes one). "not found" comes back either way. | `tree.rs:154` `confine::confine(root, rel)` → `Ok(root.join(rel))` (same at `:328`). The whole suite stays green. Verified by hand. |
| P0-2 | `ralphy-agent-gemini/src/auth.rs:164` `the_auth_probe_reads_no_credential`; gemini `command/tests.rs:108` `autonomy_argv_is_never_downgraded` (source half); cursor `tests.rs` `no_adapter_side_retry_of_a_quota_stop` | They cut each source file at the first `#[cfg(test)]`. gemini `lib.rs:163` and cursor `lib.rs:182` hold a `#[cfg(test)] fn issue_deadline` before `plan`/`execute`. The rest of `lib.rs` is never scanned. | Reading `oauth_creds` in gemini `plan`; `--approval-mode auto_edit` in gemini `execute`; a retry loop around cursor `lib.rs:371`. Verified by hand. |
| P0-3 | `ralphy-daemon/tests/child_env_hygiene.rs:13` | The test calls `auth::strip_token_from_env()` itself (`:26`). Its doc claims it proves the boot-time strip. | Delete `serve.rs:136` `auth::strip_token_from_env();` — every child inherits `RALPHY_DAEMON_TOKEN`. Verified by hand. |
| P0-4 | `ralphy-proc-util/src/cursor.rs:335` `the_gate_writes_nothing_when_already_protected` (and the "inner untouched" assert at `:289`) | `listing()` compares file names only, and the seeded body `"*\n"` is what the gate writes anyway. | Remove `.filter(!r.join(OPT_OUT_FILE).exists())` (`cursor.rs:125`) — the operator's own `.cursorindexingignore` is overwritten. Verified by hand. |
| P0-5 | `xtask/tests/user_text_cites_no_adr.rs:84-98` | The `production()` cut (text above `#[cfg(test)]`) has no test. `files.len() > 100` only counts files. | `fn production(_) -> &str { "" }` — the gate passes on every file. |
| P0-6 | `ralphy-daemon/tests/runs_watch.rs:163-191` `runs_unwatch_stops_the_pushes` | Expects `recv_verb_within(..) == None`, but that helper returns `None` on `Close`/`Err` (`:97`), and no positive control follows. `tree_watch.rs:219` does this correctly. | The `runs.unwatch` handler closes the socket or errors instead of releasing. |
| P0-7 | `ralphy-daemon/src/tests.rs:8696` `the_peer_wake_is_wired…`; `:8663` `the_run_completion_nudge_is_wired…`; `:8794` `the_explorer_can_create…`; `:8826` `naming_a_new_entry…` | Pins match the **definition** (`wakePeerFor(ref) {`, `mountChangesSub() {`, `createDir(node) {`, `askPrompt(opts = {}) {`), not the call. | Delete `app.js:3461` (the only `wakePeerFor` call) or `app.js:3483-3484`. Verified by hand for `wakePeerFor`. |
| P0-8 | `ralphy-daemon/src/tests.rs:7337` `the_destructive_console_clicks_confirm_first` | Pins the dialog titles only. | `if (ok) arrangeFence(f.id)` → `arrangeFence(f.id)` (`wb-console.js:2581`): the dialog shows, its answer is ignored. |
| P0-9 | `ralphy-daemon/src/tests.rs:3915` `the_console_clipboard_is_write_only…` | `"replaying = false;"` matches anywhere. | Replace the write callback (`wb-console.js:5303-5310`) with `term.write(a.subarray(9)); replaying = false;` — the clobber the test comment warns about. |
| P0-10 | `ralphy-agent-claude/src/headless.rs:610` `loop_exhaustion_yields_maxcalls`, `:616` `maxcalls_outcome_is_stuck` | `MaxCalls` comes from the test helper `run_headless_steps` (`:564`), not from production. | `headless.rs:173` `HeadlessReason::MaxCalls` → `Done`. |
| P0-11 | `ralphy-agent-copilot/src/tests.rs` (~5 tests, `plan_phase_uses_plan_model_in_argv` … `both_phases_omit_effort_when_unset`) | The test rebuilds the wiring: `build_copilot_command(.., agent.phase_model(Phase::Plan) ..)`. | `lib.rs:282` `Phase::Plan` → `Phase::Execute`. |
| P0-12 | `ralphy-cli/src/config/tests.rs:624` `config_set_refuses_under_held_lock` | Calls `runlock::guard_run_lock(&ws, "config set", ..)` directly. | Delete the guard in `config.rs` `run()` (`:81`/`:85`). |
| P0-13 | `ralphy-cli/src/run/wiring/tests.rs:195` `check_agents_present_probes_cursor_by_agent_not_by_selector_name` | Passes its own locator closure. | Delete `wiring.rs:395` `CliAgent::Cursor => locate_cursor()`. |
| P0-14 | `ralphy-cli/src/schedule/spec.rs:204` `timer_spec_run_with_triage_chains_triage_first` | Sets `spec.pre_invocation = Some(triage_prelude())` itself, then asserts it. | Delete the wiring at `schedule.rs:158`. |
| P0-15 | `ralphy-cli/src/init/run.rs:667` `init_git_safety_branch_and_scaffold_end_to_end` | The test does the orchestration itself (decision → `commit_all_snapshot` → `checkout_new_branch`). | The real init git stage skips the snapshot or names the branch wrong. |

---

## 3. Findings by slice

Actions: **fix** (make the test able to fail), **delete**, **merge**, **loosen**
(keep the behavior, drop the incidental text).

### 3.1 `ralphy-adapter-support` and `ralphy-agent-*` (652 tests)

| Crate | KEEP | MERGE | DELETE | LIES-FP | LIES-FN |
|---|---|---|---|---|---|
| adapter-support | 78 | 3 | 2 | 1 | 0 |
| claude | 85 | 20 | 0 | 2 | 1 |
| codex | 34 | 11 | 2 | 3 | 1 |
| kimi | 26 | 9 | 1 | 2 | 3 |
| opencode | 42 | 12 | 3 | 3 | 0 |
| gemini | 81 | 13 | 9 | 4 | 7 |
| cursor | 73 | 7 | 8 | 1 | 11 |
| copilot | 59 | 12 | 9 | 8 | 4 |

**LIES-FP (beyond P0-2, P0-10, P0-11):**

- A-1 codex `skills.rs:83` `the_dance_is_not_reimplemented_locally`: only checks
  that `fn link_or_copy_dir` is not *defined* locally. The whole loop is copied in
  codex/copilot/cursor `skills.rs`. Any change to a local copy survives.
- A-2 codex `tests.rs:343`, kimi `lib.rs:414`, opencode `lib.rs:494`
  `plan_charter_file_carries_full_prompt`: `fs::write` then read back — tests
  std::fs. Deleting the charter write in `adapter-support/src/scaffold.rs:78`
  survives. Delete, and add one scaffold test (see G-2).
- A-3 copilot `outcome.rs:304` `classify_done_ignores_zeroed_code_changes`:
  `committed` hard-coded `true`, and the ladder ignores it for Done. Delete.
- A-4 opencode `resolved_effort_does_not_become_variant_on_argv`, opencode
  `command.rs:139`, kimi `lib.rs:326` `resolved_effort_never_appears_on_argv`: the
  builder has no effort parameter. Delete.
- A-5 gemini `tasks.rs` `the_triage_command_includes_every_attachment_directory`:
  calls `add_include_directories` itself; `triage_issues` passing `&[]` survives.
- A-6 copilot `tasks.rs:285` `preflight_or_bail_rejects…`: only `is_ok()`, and the
  result depends on the host. Delete.
- A-7 adapter-support `assets.rs:138`: `gi_contents.contains('*')` — use
  `assert_eq!(gi, "*\n")`.
- A-8 opencode `events.rs:394` `parse_limit_zen_usage_limit_error`: only
  `is_some()` — assert `Some(None)`.

**LIES-FN:** about 45 `include_str!("*.rs")` tests (gemini 20, cursor 14,
copilot 8, codex 2, kimi 1, claude 1). Absence lints ("no hardcoded model
table", "no credential read") are acceptable. The wiring/order pins are brittle:
cursor `both_phases_report_stream_usage`, `every_run_notes_the_credit_unit_mismatch`,
`execute_notes_the_degraded_calls`, `the_plan_path_routes…`,
`the_resume_path_is_gated…`, `locate_cursor_delegates…`; copilot `tasks.rs:265`,
`tests.rs:107`, `:156`; gemini `tests.rs:28`, `:157`, `:217`, `each_verb_roots…`;
codex `tests.rs:266`; claude `status.rs:313`; kimi/gemini
`resolved_effort_is_stored_for_documented_discard` (pins the no-op
`let _ = self.plan_effort.as_deref();`). Also cursor `command/tests.rs:127`
(`args[i+1] == "--model"`, argv order) and kimi `command.rs:285`, `:321` (whole
argv vec equality).

**DELETE:**
- `*_agent_is_a_dyn_agent` ×7 (compile-only; `ralphy-cli/src/run/wiring.rs:228`
  already boxes each agent).
- `mint_session_id_is_a_fresh_uuid` ×3 (tests uuid).
- `plan_charter_exceeds_argv_safe_size` (cursor, gemini), copilot
  `exec_charter_exceeds_argv_safe_size`, gemini
  `the_roundtrip_fixture_carries_the_whole_charter` (asset size).
- ADR prose: opencode `adr_0005_d3_amendment…`, cursor
  `the_limit_stance_is_documented`, gemini `the_limit_stance_is_documented_as…`.
- serde/Default: gemini `settings.rs:37`, cursor `settings.rs:47`, copilot
  `settings.rs:51`, `:64`.
- Constants: gemini `accepts_images_is_true`,
  `the_auth_message_reproduces_the_vendor_sentence`; cursor
  `the_credit_note_names_both_units`.
- copilot `usage.rs:106`, `:122`, `:141` (duplicates of `ralphy-usage-scan`);
  copilot `every_effort_model_supports…` (asserts fixture data).
- opencode `resolved_model_label_returns_model_or_unknown`; opencode
  `events.rs:319` `…takes_precedence_over_done_sentinel` (its name claims a
  precedence that production does not have, `scaffold.rs:172`).
- adapter-support `detect_limit_maps_the_three_states` (tests `bool::then`),
  `issue_budget_new_seeds_the_default_cap` (getter).
- codex `xhigh_tier_effort_is_a_codex_accepted_word`,
  `effort_does_not_alter_the_tier_routed_model` (subsets of `command.rs:384`,
  `:265`).
- cursor `the_pinned_path_is_a_shape…`.

**MERGE within a crate:** claude `headless_reason_*` 5→1, `headless_step_*` (3,
covered by the loop tests), `parse_reset_hhmm_*` 5→1, `is_claude_auth_error_*`
3→1, `staged_plan_env_*` 2→1, `plan_prompt_for_not_staged_with_no_labels`
(subset), `settings_have_stop_hook…` (subset of `status_hooks_ride_both_phases…`);
gemini exit→Blocked 4→1, `usage.rs` 3→1, `root.rs` 3→1; codex
`build_command_threads_the_effort_through` (subset), `usage.rs:172` (subset of
`tests.rs:47`); cursor `every_one_shot_gates_before_it_spawns` (subset of
`every_spawn_site…`); copilot 3 "no hardcoded model" pins → 1.

**Cross-adapter duplication (shared path proven):**

| Tests | Copies | Proof | Action |
|---|---|---|---|
| `*_honours_max_minutes_per_issue`, `*_zero_minutes_disables…` | 13 in 7 crates | Each `issue_deadline()` is `self.budget.deadline(UNBOUNDED_ISSUE_HORIZON)` (codex `lib.rs:142`, copilot `:235`, cursor `:183`, gemini `:164`, kimi `:134`, opencode `:173`; claude calls `ralphy_adapter_support::issue_deadline`). `budget.rs:98-150` covers it. | Delete all. |
| `classify_{done,blocked,timeout_wins,done_on_no_commit,stuck_on_no_sentinel}` | codex 5, kimi 4, copilot 3, opencode 9 | Pass-through to `adapter_support::classify` (`classify.rs:60` tests the ladder). | Keep vendor-specific cases only (non-zero exit → Stuck, vendor limits, kimi exit 75, opencode error event). Delete about 21. |
| `prompt_plan_*_carries_finalize_trailer` | 8 asserts in 7 crates | The trailer lives in `assets/prompts/plan/template.md`; `ralphy-core/tests/prompt_assembly.rs` checks every artifact against template + overlay. | One test. |
| Skills dance (`*_preserves_user_skills`, idempotency) | codex, copilot, cursor | Loop copied line for line (codex `skills.rs:38-72`, copilot `:30-62`, cursor `:33-64`). | Production refactor into adapter-support is a separate decision (§6). |
| Skill frontmatter lints | codex, copilot, gemini | Same `assets/plugin/skills`. | One lint. |
| `no_tests_directory` | copilot, cursor, gemini (missing in 4) | Policy, not behavior. | One check over all adapter crates. |
| `plan_pointer…` / `PLAN_CHARTER.len()*50 <` | claude `plan.rs:123`, codex, kimi, opencode | `sentinel.rs:59` asserts `len() < 512`. | Delete the copies. |

**`#[ignore]`:** claude `interactive/tests.rs:171` (real `claude`), copilot
`catalog.rs:440` (network; returns early with `eprintln` when the binary is
absent unless `RALPHY_LIVE_COPILOT` is set — a `--ignored` run can pass
silently), cursor `command/tests.rs:431` (Windows e2e; the "baseline arm" only
prints). CI never runs `--ignored`. All three are legitimate manual probes; only
the copilot silent early return is a defect.

### 3.2 `ralphy-daemon/src/tests.rs` (171 tests, all read)

KEEP 101, MERGE 22 (→ ~9), DELETE 9, LIES-FP 10, LIES-FN 29.

History shows the LIES-FN cost: copy-only commits `96d2c527` and `7355c15c` had
to edit pins, and feature commits `26fc2fa1`, `9c17f949`, `b373b715` had to edit
exact-statement pins.

**DELETE:** `:386` `api_desk_serves_windows_and_fences_together` (identical to
`:372`); `:1242` `xterm_asset_is_served` (subset of `:3524`); `:3134`
`bind_addr_default_is_loopback` (tests `SocketAddr::new`); `:3116`
`build_presence_carries_identity_and_uptime` (covered by `tests/auth_ws.rs:78`);
`:4335` `login_gate_drops_mock_hint` (duplicate of `:4066` + copy pin); `:3986`,
`:4007`, `:4167` `root_serves_wb_{daemon,mode,fail}` (covered by the sweep at
`:4054` and `:4856`); `:4127` `translation_is_gone_from_the_served_ui`.

**MERGE:** `:1222` + `:3454` (GET `/`); `:3143`, `:3169`, `:3201`, `:3231`
(bearer table; add `/api/identity` to the `:4577` list); `:4184` into
`session_policy_login_flow`; `:4215` + `:4281`; `:2753`, `:2813`, `:2877`,
`:2946` (`api_usage_carries_*` → one test seeding four stores); `:2226` +
`:2271`; `:444`, `:890`, `:922` (desk extractor rows into the `:989` loop);
`:8583`, `:8376`, `:8639` (colour-token scans → one table); `:6559` and the six
other "tag X before `wb-console.js`" checks (`:1942`, `:4931`, `:6540`, `:6762`,
`:6818`, `:7247`) → one list in the `:4934` loop.

**LIES-FP (beyond P0-7 to P0-9):** `:3564` `the_explorer_opens_a_note_as_a_card`
(`&&` → `||` in `app.js:67-70` survives); `:5122`
`the_changes_section_renders_a_status_marked_list` (`contains("added")` matches
comments); `:6328` `a_detached_file_comes_home…` (app.js half duplicates
`wb-detach-file.test.mjs:70-108`); `:6900` `shell_navigates_the_plane` (noun pins
in a 6,855-line file).

**LIES-FN:** `:8110` (exact attributes, `.count() == 3`); `:8045`
(`_changesRefused(` count == 15); `:8176`; `:7762`; `:7081`, `:6977`, `:5814`
(full `if (…)` lines); `:4148`, `:6615`, `:7823`, `:7381`, `:5704`, `:7267`
(exact statements, argument order); `:7600`, `:7562`, `:7844`, `:7954`
(duplicate `wb-runs.test.mjs:54-403` and `app.test.mjs:244`); `:8996` (pins the
regex text; `wb-project.test.mjs:25-46` covers it); `:5435`, `:5912`, `:5994`,
`:6098`, `:6854`, `:5622`, `:6359`, `:6636`, `:5371`, `:8266` (`function <fold>(`
pins duplicating node tests — keep the CSS rule pins and the call-order checks
at `:6452` and `:6231`); `:6779` (duplicates `tests/wb_session_owner_351.js`,
which this Rust test spawns).

**Defects inside KEEP tests:** `:1125` `security_state_reflects_the_stores` reads
the snapshot taken at `:1109`, before the flags were set — re-read the state.
`:2112` `api_agents_serves_the_roster` hard-codes the list of 7 vendors, against
the AGENTS.md rule — build it from `session::Agent::ALL`.

**Keep as they are** (real invariants): tag resolution and reachability, cascade
order, sweep floor, barrel import check, `no_selector_sets_one_property_twice`,
CSP hash vs. served inline script, `every_settable_key…`, the Crepe recipe
header, the tree-wide `localStorage` negative, the CSS custom-property check at
`:1988`.

**Out-of-date comments:** several pins say they are "the only gate in CI". CI
runs `node --test` (`ci.yml:52-77`).

### 3.3 `ralphy-daemon/src/*` other modules (490 tests, ~300 read)

**LIES-FP (beyond P0-1):**
- D-1 `autostart/tests.rs:30`, `:97`: needle `"daemon"` also matches
  `TASK_NAME` and the log path. `autostart.rs:140` `'{exe}' daemon *>>` →
  `'{exe}' *>>` survives. Assert `"'/usr/local/bin/ralphy' daemon"`.
- D-2 `autostart/tests.rs:248` `systemd_unit_has_execstart_and_wantedby`:
  `Description=Ralphy daemon` satisfies `"daemon"`. Assert
  `"ExecStart=/usr/local/bin/ralphy daemon\n"`.
- D-3 `peer/nudge/tests.rs:139` `distro_liveness_answers_without_starting_anything`:
  on a host without WSL, both reads are Err and the `if let Ok` skips. The "does
  not start anything" claim is unasserted. Pin the probe argv or delete.
- D-4 `totp.rs:265` `otpauth_uri_carries_secret_and_params`: compares with the
  function under test. `secret_base32` returning hex survives. Compare with the
  literal `GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ`.
- D-5 `session/spec/tests.rs:229`, `:211`, `:280`: source-text pins over another
  crate (a known weak guard, ADR-0032 §10 forbids the dependency). Keep with the
  caveat; the behavior is covered at `:245`, `:574`.
- D-6 `usage/tests.rs:266` `every_launchable_vendor_has_a_store_path_resolver`:
  a commented-out `// scan_kimi(&…)` passes.
- D-7 `spend/tests.rs:381`: `!js.contains("toFixed")` (FN) and
  `html.contains("spendView().floorNote")` (FP). No JS test covers
  `spendView`/`floorNote` (G-9).

**LIES-FN:** `session/spec/tests.rs:147` (source pin; the behavior half compares
the fallback with itself on a host without Cursor); `usage/tests.rs:435`
(tombstone of a finished removal); `dispatch.rs:538`
`app_js_holds_no_vendor_list` (counts `"claude"` literals); `desk/tests.rs:349`,
`:633`, `:917`, `checkout.rs:363` (test prose in `CONTEXT.md` and
`docs/daemon.md`).

**DELETE:** `autostart/tests.rs:66`; `dispatch/spawn/tests.rs:56`
`take_output_yields_child_bytes` (tests `FakeChild`, defined in the test file);
`peer/nudge/tests.rs:28`, `:174` (subsets of the exact-argv tests);
`usage/tests.rs:396` (duplicate of `wb-spend.test.mjs:89`); `usage/tests.rs:291`;
`desk/tests.rs:614`, `:908` (same `rect_is_sane` as `:297`, `:307`);
`cookie.rs:291`; `roster.rs:171`; `epoch.rs:119`; `release.rs:363`.

**MERGE:** `autostart/tests.rs:50`, `:58`, `:86` (one table per platform);
`desk/tests.rs:138`, `:402`, `:419` → `:947`; `desk/tests.rs:164`, `:372`,
`:835` (+ `:478`) → one legacy-load test; `desk/tests.rs:90`, `:126`, `:177`,
`:478` (skip-if-absent table); `protocol.rs:145/153/159`, `:204-219`;
`dispatch/argv/tests.rs:888`, `:910`, `:932`, `:953` (one loop over
`Agent::ALL`); `dispatch/argv/tests.rs:474`, `:483`, `:494`, `:786`;
`session/spec/tests.rs:379`; `watch.rs:430` (subset of `:460`, slow watcher).

**Cost note:** `peer/nudge/tests.rs:41` `nudge_never_waits` runs `ping -n 31` to
completion (~30 s per Windows run). Consider `-n 8` with a `> 5s` bound.

### 3.4 `ralphy-daemon/tests/*.rs` (155 tests in 55 binaries)

41 of the 55 files hold one test. Quality is high; the cost is binary count.

- `command_changes.rs:19`: strict subset of `command_checkout_cwd.rs` legs (a)
  and (e). Delete the file.
- `command_blob.rs`, `command_branch.rs`, `command_config.rs`,
  `command_worktree.rs`, `command_board.rs`: the same ~95-line body five times.
  Add one argv assert per leg in `command_checkout_cwd.rs`, plus a `board.list`
  leg; delete the 5 files.
- `command_config_mutate.rs:17` → a leg in `command_mutate_git.rs`
  (`assert!(reply.get("status").is_some())` cannot fail on its own).
- `command_mutate_ok_message.rs:57` → a leg in `command_changes_mutate.rs:219`.
- `command_run_params.rs` + `command_ws.rs` → one file.
- `command_refusal.rs` → the env-free observe binary.
- `session_ws.rs`, `session_ws_cursor.rs`, `session_ws_checkout.rs`,
  `session_ws_gemini.rs`, `session_persistence.rs`, `console_ws.rs`,
  `console_reattach.rs`, `console_command_ws.rs` → one binary with the
  `static Once` pattern of `session_single_writer.rs:106-115`.
- `fleet_console.rs:196` → `fleet_session.rs` (same env).
- `codec_transport_free.rs` + `session_transport_free.rs` → one file. The bare
  substring scan was already tripped by prose
  (`docs/research/plan_friction/0166…md:27`).
- `observe_read.rs:236` DELETE (weaker than `file_encoding.rs:143`);
  `observe_read.rs:167` fold into `:195`.
- `workspace_write.rs:449`, `:469`, `:492` DELETE (same inputs as
  `clipboard.rs:165`, `:181`; wire path proven by `:425`).
- `tree_watch.rs:115` duplicates the positive control at `:219-243`.
- Env-free binaries (17 files, no `set_var`) → three groups: observe
  (`observe_read`, `runs_list`, `file_encoding`, `workspace_write`,
  `command_refusal`), watch (`tree_watch`, `head_watch`, `runs_watch`,
  `fleet_watch`), ws/peer (`auth_ws`, `ws_presence`, `peer_handshake`,
  `peer_pool`, `fleet_usage`).
- Keep separate (distinct env): `security_routes`, `security_step_up`,
  `security_remote_images`, `network_bind_gate`, `session_colour_env`,
  `session_ws_agent_state`, `session_liveness`, `child_env_hygiene`,
  `repos_rekey`, `command_sync`, `command_changes_mutate`,
  `command_checkout_cwd`, `command_worktree_remove`, `command_stream_teardown`,
  `fleet_command`.

Result: 55 → about 26 binaries. The saving is one release-profile link of the
daemon per binary plus nextest's per-binary `--list`. **Not measured.**

**Flake risk (LIES-FN):** fixed "let the watch attach" sleeps before the write
that must be caught: `tree_watch.rs:121-238`, `head_watch.rs:129/154/183`,
`runs_watch.rs:151/174/184/204`, `fleet_watch.rs:145/152`,
`changes_nudge.rs:113/144`. `runs_watch.rs:144` itself warns against this. No
flake record found; act only if one appears.

**Scripts `tests/*.py` and `*.js` (81 files, 44,288 lines):**
- CI never runs the 79 Playwright `.py` files. They are manual acceptance scripts,
  one per issue. They are maintained (`fe73d571` updated 11 of them;
  `7bd6d958` updated 318/319), referenced from `src/tests.rs`, `assets/ui/*.js`
  and docs, and a static check of the 6 oldest found every selector and route
  still present. **Keep them.**
- Some share a hard-coded port (7443 ×4, 7399 ×4), so they cannot run in
  parallel.
- `wb_consoles_305.py`: its absence checks for `.plan-xlate` and
  `.md-xlate-note` can no longer fail.
- `wb_session_owner_351.js` **does** run in CI, spawned by
  `src/tests.rs:6833-6846`. Move its 5 tests into
  `ui-tests/wb-session-route.test.mjs` and remove the spawn.
- `wb_fleet_label.js`: nothing runs it, and `ui-tests/wb-fleet.test.mjs:233`
  covers it with a stronger negative control. Delete.

### 3.5 `ralphy-cli` — runstate, init, events, ui (287 tests, all read)

KEEP ~195, MERGE 38 (→ ~8), LIES-FP 15, LIES-FN 9, DELETE 30.

**LIES-FP (beyond P0-15):**
- C-A1 `runstate/event/tests.rs:40` `the_idle_reap_is_emitted_below_warn…`: no
  emitter is called. Delete.
- C-A2 `runstate/event/tests.rs:310` `decoder_maps_kimi_planning_and_executing`:
  kimi is never touched. Delete.
- C-A3 `runstate/capture/tests.rs:582` `no_vocabulary_literal_outside_emit`:
  scans a hand-written list of 23 files; `info!("issue started")` in an unlisted
  file survives. Delete, or accept as a partial lint.
- C-A4 `runstate/snapshot.rs:401` `every_issue_status_is_known_to_the_runs_panel`:
  `PANEL.contains("{wire}: \"")` over the whole file; deleting a LABEL entry in
  `wb-runs.js:49` survives. Scope the check per table and merge with `:336`.
- C-A5 `runstate/snapshot.rs:422` `project_is_pure_over_runstate`: two calls in
  the same second. Delete.
- C-A6 `runstate/state/tests.rs:164`: only `contains("#7")`. Assert the full
  summary.
- C-A7 `init/gate.rs:363` `newcomers_go_last_in_all`: pins only the last two
  entries; swapping Claude and Codex survives. Pin the whole prefix or delete.
- C-A8 `init/gate.rs:384`, `:418`: source pins; `let _ = probe_cursor_login();
  return cursor_logged_in(true)` survives. Delete.
- C-A9 `init/issues.rs:506`: the test writes the draft with serde itself. Delete.
- C-A10 `events/emitter.rs:255` `public_ip_falls_back_to_local…`: real HTTP in a
  unit test, and the probes are never forced to fail. Delete. Verified by hand.
- C-A11 `events/envelope/tests.rs:383`: greps `docs/events.md`. Delete.
- C-A12 `events/sink/poller.rs` (last block of the first test): its sibling's
  doc says so; an empty `reset_from_written` survives. Trim.
- C-A13 `ui/presenter/tests.rs:177` `enqueue_is_off_the_run_path…`: builds its
  own `mpsc::channel`, never calls `on_event`. Delete. Verified by hand.
- C-A14 `ui/tests.rs:959` (content already fits at 60 columns), `ui/tests.rs:1045`
  (only `!line.is_empty()`): add `display_width <= 10` or merge into `:988`.

**LIES-FN:** `runstate/capture/tests.rs:163` (exact text of constants),
`:273` (counts lines that start with `"` or `ralphy_`), `:509` (greps 9 adapter
files for variable names — whether adapters test their own `emit::planning`
arguments is unchecked, G-13); `capture/tests.rs:16`, `:81` (`ev.target` pins;
the decoder ignores `target`); `snapshot.rs:336` (2-space indent — still the only
cross-language gate, keep and merge with `:401`); `init/gate.rs:341`
(`ALL.len() == 7`); `ui/tests.rs:1445` (hard-codes `30.0` from the price table).

**DELETE:** `runstate/event/tests.rs:16`, `:61`, `:100`, `:342`, `:375`, `:393`,
`:415`, `:478`, `:498` (9 hand-built decoder tests; `roundtrip.rs` proves the
same arms through the real `emit::*`; `:375` and `:393` are identical — keep one
small table for legacy absent fields); `runstate/fields.rs:329` (doc comment);
`runstate/state/tests.rs:83`, `:506`, `:542`, and the decoder half of
`:121-132`; `init/gate.rs:374`, `:458`; `init/scaffold.rs:246`, `:266`, `:288`
(code is `#[allow(dead_code)] // SUSPENDED`); `init/wizard.rs:194`;
`init/skills.rs:181`; `events/emitter.rs:332`; `events/config.rs:~200`;
`events/sink/delivery.rs:484`, `:508`; `ui/tests.rs:126`, `:932`, `:1436`.

**MERGE:** `roundtrip.rs:96-221` (8 → 2); `event/tests.rs:446` + `roundtrip.rs:686`;
`state/tests.rs:136` + `:318`; `init/gate.rs:477-576` (7 → 1); yes/no decisions
`init/issues.rs:320`, `:331`, `init/run/decisions/tests.rs:4`, `:16`, `:66`,
`init/skills.rs:214`, `init/verify.rs:342` (7 → 1 — `draft_decision`,
`create_repo_decision` and `labels_decision` are byte-identical functions);
`events/envelope/tests.rs:306` + `:339`; envelope mappings `:394`, `:504`, `:518`,
`:561`, `:575`, `:585`, `:610`, `:627`, `:942`, `:979` (10 → 1).

**Cost:** `events/emitter.rs:268`, `:294`, `:311` call `detect()`, which does up
to 4 s of real network probes plus a git spawn. `:294` says "Pure serde … no
env", which is false. Build `Emitter` as a struct literal.

### 3.6 `ralphy-cli` — the rest (419 tests)

KEEP ~268, MERGE ~109, LIES-FP 17, LIES-FN 13, DELETE 12. Strongest files:
`snapshot_engine`, `daemon/restart`, `install`, `update/apply`, `stop`,
`runlock`, `hook`, telegram worker.

**LIES-FP (beyond P0-12 to P0-14):**
- C-B1 `run.rs:646` `no_print_notice_call_remains_in_run_cmd`: the function no
  longer exists in `crates/`. Cannot fail. Delete. Verified by hand.
- C-B2 `run.rs:667` `resolution_byte_for_byte_when_absent`: builds its own
  resolution chain. Delete.
- C-B3 `delivery.rs:321` `layer_enqueue_is_off_the_run_path`: never calls
  `on_event`. Delete.
- C-B4 `pricing.rs:122` `refresh_if_stale_sole_production_call_is_usage_cmd`:
  greps a hand-written list of 6 files. Delete, or move to a crate-wide scan.
- C-B5 `main.rs:301`, `:336` `copilot_one_shots_are_wired`,
  `cursor_one_shots_are_wired`: `contains` over the whole file; swapping the two
  calls survives (the comment at `main.rs:372-375` says so). Delete.
- C-B6 `pricing.rs:105` `cli_manifest_pins_ureq_excludes_reqwest_tokio`: the
  reviewer called it a lie because `ralphy-cli` reaches tokio through
  `ralphy-daemon`. **Overstated:** the rule is that async stays in the daemon
  crate, and the test guards only the direct dependency. It is a weak text lint,
  not a lie. Keep or replace with `cargo metadata`; low priority.
- C-B7 `telegram/notifier/render/tests.rs:260`: the "varies" half is never
  asserted. Add one assert.
- C-B8 `update.rs:269` `this_build_never_reports_itself_behind`: empty release
  list; an inverted comparator survives. Pass a release with the build's tag.
- C-B9 `usage.rs:605`: only `contains("tok")`, `contains("~$")`. Assert the exact
  USD.
- C-B10 `triage/tests.rs:44`, `:58`: only `warn: Some(_)`. Assert the text.
- C-B11 `telegram/notifier/tests.rs:41` `live_animate_card` (`#[ignore]`, no
  asserts). Delete. `:123` asserts Telegram's own behavior and passes silently
  when unconfigured; delete or mark as a manual probe.

**LIES-FN:** `run.rs:696` (splits `include_str!("run.rs")` on exact text);
`run/wiring/tests.rs:22-111` (6 tests slicing between match-arm text);
`main.rs:377`; `issues/tests.rs:113` (doc comment), `:306` (ADR markdown);
`cli/tests.rs:50`, `:69` (identical bodies, `assert_eq!(n, 29)` flags);
`config/tests.rs:647` (prose).

**DELETE:** `run/wiring/tests.rs:114` (tests `remove_var`), `:449`;
`config/tests.rs:667`, `:672` (`:715` checks every key); `cli/tests.rs:84`,
`:95`, `:134` (clap derive; keep the rc-default part of `:106`), `:593`;
`telegram/config.rs:122`, `:135` (serde); `triage/tests.rs:295` (subset of
`:156`); `daemon.rs:402`.

**MERGE:** `guard/tests.rs:31-465` (51 → ~5 tables; the empty-command and
blank-path tests at `:212-227`, `:442-452` pass even without the early return —
cheap rows to add: `gh repo/workflow/secret/auth`, `mkfs`, `iwr|iex`,
`/credentials`, `id_rsa`, `.pfx`); `cli/tests.rs:400-457` (one loop over
`CliAgent::value_variants()` — adds claude and codex); `cli/tests.rs:460-591`
into `internal_commands_are_listed_apart_and_still_parse`;
`cli/tests.rs:275-324`; resolver tests `config/tests.rs:21-48`, `:280-331`,
`run/wiring/tests.rs:216-255`, `:297-316`, `run.rs:747`; config key round trips
`config/tests.rs:377`, `:417`, `:485`, `:502`, `:652`; `run/wiring/tests.rs:339`
+ `:413` (one loop over all variants — adds 5 vendors); `wiring:162-213`,
`triage:201/223/239`, `client.rs:322-358`, `summary.rs` from_report pair,
`hook.rs:336/347`; `tests/mutate.rs:105` + `:152` (`:152` switches to a branch
that does not exist, so it fails even without the lock).

**Integration binaries:** 9 → 1 (`tests/verbs/main.rs` + `mod`s). Each file
copies `init_repo`/`run_git`/`hold_run_lock`. The ~10 lock-refusal tests
(`worktree.rs` ×4, `sync.rs` ×2, `checkouts.rs` ×2, `mutate.rs` ×2) plus
`config set`/`config unset` become one table with one lock-holder child.

**`#[ignore]`:** `update/apply.rs:439` is a documented manual smoke test
(`docs/BUILDING.md:171-175`). Keep.

**Hazards:** `telegram/client.rs:217` and `update/apply.rs:174` change
`ALL_PROXY` without a lock in the same binary (safe under nextest, racy under
`cargo test`). `usage.rs:531` passes only inside a git checkout.

### 3.7 `ralphy-core` (466 tests, ~270 read)

The queue suite is honest: `ScriptedAgent`, `RecordingTracker` and
`ScriptedClock` stand in only at the boundaries, git is real, and assertions check
lifecycle decisions. `tests/stop.rs`, `tests/verify_gate.rs`, `sync/tests.rs` and
`protocol` are strong.

**LIES-FP:**
- K-1 `tests/release_profile.rs:4`: `contains("strip = true")` over the whole
  `Cargo.toml`; renaming `[profile.release]` survives. One binary for one grep.
  Delete. Verified by hand.
- K-2 `tests/queue/selection.rs:124` `view_and_run_agree_under_assignee_filter`:
  no assignee filter runs. Delete.
- K-3 `src/model_recovery.rs:420` `locked_merge_child`: a child helper that
  returns early when its env var is absent, so it always "passes" and costs a
  process. Move to `src/bin/*_test_child.rs` (CONTEXT.md convention) or mark
  `#[ignore]` and run with `--ignored --exact` from the parent. Verified by hand.

**LIES-FN:** `github/labels/tests.rs:36` (`names.len() == 11`); `protocol.rs`
`clean_plan_passes_every_check`, `unticked_step_fails`, `bare_noticed_step…`
(`checks.len() == 7`, `checks[0]`); `init_session.rs`
`triage_prompt_names_marker_verdicts_and_output_path` (~12 prose sentences);
`tests/prompt_ledger.rs:17` (example sentence word for word);
`tests/queue/main.rs:862` `pin` helper (`ev.target == "ralphy_core::emit"`,
used by 8 tests). Minor prose pins: `failure_brief` `"HONESTLY"`/`"SAME"`,
`cmdcost` `"NARROWEST"`, handoff `"leads, not truths"`, references
`"treat it as a lead"`, close_artifacts `"\u{2717} ## Acceptance ledger present"`.

**DELETE:** `src/stop.rs` `the_flag_round_trips_and_clears` (it also sets the
process-global stop flag in a binary that runs `run_queue_with`); `src/types.rs`
3 path getters; `serde_round_trip` ×3 (`issues_draft.rs`, `diagnosis.rs`,
`triage_draft.rs:170`); `github/references.rs:78` `e2e_references_for_bioledger_29`
(`#[ignore]`, hard-coded to a private repo); `github/issues/tests.rs:137`
(confirm that `parse_issue` goes through `From<GhIssue>` first).

**MERGE:** `tests/queue/selection.rs:190` (subset of `:219`); `:249`, `:287`,
`:347` (one queue, one repo); `close_artifacts.rs:6` + `:72`;
`lifecycle.rs:141` into `:6`; `events.rs:14` into the pins test; the two
`exec_usage_*` tests; `verify.rs:215` (duplicate of `runner/tests.rs:342`);
`limits.rs:6` + `:378`; `tests/prompt_assembly.rs`
`overlays_define_exactly_the_known_slots`; `tests/effort.rs` (2 tests, own
binary) into `src/effort.rs`; `protocol.rs` `ledgerless_plan_fails_the_lint_pins_310`;
`markdown.rs` `stops_at_next_heading`; `github/labels/tests.rs` (delete 2, table
3); `settings.rs` 4 → 2; parser cases in `blocked.rs`, `handoff.rs`, `plan.rs`,
`github/issues` (~23 → 7).

Binaries: −3 (`release_profile`, `effort`, `prompt_ledger`). `tests/stop.rs` must
stay separate (process-global flag). About 9 fewer git-repo fixtures in the
queue suite.

### 3.8 Small crates and `xtask` (334 tests)

| Crate | Tests | KEEP | MERGE | LIES-FP | LIES-FN | DELETE |
|---|---|---|---|---|---|---|
| ralphy-usage-scan | 87 | 80 | 4 | 2 | 0 | 1 |
| ralphy-release | 34 | 33 | 1 | 0 | 0 | 0 |
| ralphy-proc-util | 32 | 25 | 1 | 5 | 0 | 1 |
| ralphy-pricing | 30 | 21 | 4 | 2 (partial) | 2 | 1 |
| ralphy-pty | 25 | 13 | 12 (→ 2) | 0 | 0 | 0 |
| ralphy-run-snapshot | 21 | 15 | 3 | 2 | 0 | 1 |
| xtask | 105 | 97 | 2 | 3 | 1 | 2 |

**LIES-FP (beyond P0-4, P0-5):**
- S-1 `proc-util/src/tests.rs:500` `locate_program_prefers_path_over_local_bin`:
  `~/.local/bin` is empty. Put a `tool` there too.
- S-2 `proc-util/src/pid.rs:116` `pid_is_alive_detects_own_process`: alive
  direction only; `-> true` survives. Add a dead pid.
- S-3 `proc-util/src/cursor.rs:310`: only `is_ok()`. Assert the folder is still
  empty.
- S-4 `proc-util/src/cursor.rs:351` `no_cursorignore_in_proc_util`: source grep.
  Delete; assert the exact folder listing in the "creates" test instead.
- S-5 `pricing/src/fetch.rs:446` tail (`PriceTable::defaults()` never reads the
  cache), `:509` tail (`is_some()` — assert `36.75`).
- S-6 `run-snapshot/src/read.rs:175`: runid order equals `started_at` order.
  Give "01LIVEA" the later `started_at`.
- S-7 `run-snapshot/src/document.rs:211`: repeats the production expression.
  Pin the literal path.
- S-8 `usage-scan/src/recovery.rs:350`: `zip` stops at the shorter list. Assert
  equal lengths.
- S-9 `usage-scan/src/claude.rs:483`: `repos: &[]`. Register a non-matching repo.
- S-10 `xtask/src/main.rs:299-310`: input already sorted. Shuffle it.
- S-11 `xtask/src/ui_copy/tests.rs:532`: `let _ = rows(...)`. Assert one row or
  rename to `…_never_panics`.

**LIES-FN:** `pricing/src/lib.rs:423` (private fields); `pricing/src/fetch.rs:579`
(Cargo.toml text); `xtask/src/ui_copy/check/tests.rs:191` (exact column spacing).

**DELETE:** `proc-util/src/cursor.rs:259`; `pricing/src/lib.rs:607`;
`pricing/src/floor.rs:89` (one line: counts its own array);
`run-snapshot/src/document.rs:270`; `release/src/fetch.rs:552`;
`usage-scan/src/codex.rs:666`; `xtask/src/release_cmds/tests.rs:440`;
`xtask/src/changelog.rs:556` (keep only the "no `topic` key" assert).

**MERGE:** `pty/src/lib.rs:356-417` (10 `cmd_hazard_*` → 1);
`pty/tests/pty.rs:245`, `:258` → 1; `pricing/src/floor.rs:102`, `:142`, `:159`
into golden rows; `pricing/src/fetch.rs:499` into the 503/429 tests;
`proc-util/src/tests.rs:213` (Windows-only); `run-snapshot/src/document.rs:218`,
`:250` (drop the round trip and `SNAPSHOT_VERSION == 1`); `read.rs:314` into
`:300`; usage-scan `gemini.rs:336`, `kimi.rs:538`, `opencode.rs:267`,
`copilot/tests.rs:420`; `xtask/src/release_cmds/tests.rs:180` + `:303`,
`changelog.rs:584`.

**Rename:** `pty/tests/pty.rs:168` `kills_and_waits_the_process_tree` →
`kills_and_waits_the_child` (`kill()` only kills the direct child).

**xtask lints:** `ui-copy --check`, `capabilities` and changelog parsing are
tested in both directions. `inline_test_modules` silently skips a module whose
closing brace it cannot find (`inline_test_modules.rs:85`) and has no fixture
over the budget. `asset-pins` enforces nothing and CI never runs it; its 13 tests
are sound, but the tool served the finished file-split series (#367). Retiring it
removes about 14 tests (decision, §6).

**Timing:** `write_atomic_replaces_and_never_shows_a_partial_document` is
probabilistic. The `ALL_PROXY` tests change a process-wide variable without a
lock.

### 3.9 JS `crates/ralphy-daemon/ui-tests` (520 call sites, all read)

`harness.mjs` loads the real assets with `new Function` and stubs only `fetch`,
`WebSocket`, timers, an empty DOM, `BroadcastChannel`, Monaco, and some sibling
modules. No test stubs the function it tests.

**LIES-FP:**
- J-1 `wb-encoding.test.mjs:277`: never checks the dirty mark; deleting
  `wb-viewer.js:1400-1401` survives.
- J-2 `wb-daemon.test.mjs:189`: `typeof RESUME_DEBOUNCE_MS === "number"`. Delete.
- J-3 `wb-console.test.mjs:1781`, `:1904`: "stay named, not inlined" cannot see
  call sites. Delete.
- J-4 `wb-console.test.mjs:1792`: with no windows, the probe is never asked.
  Test with one window.
- J-5 `wb-console.test.mjs:1681`: only test of `atFenceCap`, on an empty plane;
  `() => false` survives. Test at the cap.
- J-6 `wb-runs.test.mjs:397` (+`:403`): `note.includes("run")` with verb
  `"run"`. `assert.equal` on the full string.
- J-7 `wb-notes.test.mjs:365`: checks `MARKDOWN_HELP` data only. Rename or delete.
- J-8 `wb-window-state.test.mjs:485`: first assert contradicts the name; `:467`
  covers it. Delete.
- J-9 `wb-geometry.test.mjs:292` (+ `:57-154`): `TILES[].want` never read here;
  the table is byte-identical to `wb-console.test.mjs:1040`, which asserts the
  exact `want`. Keep one exact table. Verified by hand.

**LIES-FN:** `wb-columns.test.mjs:192` (exact key list); `app.test.mjs:864`
(call order of 4 stubbed methods); `wb-viewer.test.mjs:284` (full log order);
`wb-notes.test.mjs:76`, `:146` (`:165-172`), `:315` (`:320`), `:458` (`:459`)
(constant lists); `wb-console.test.mjs:2179` (`PHONE_MAX_WIDTH === 560`, never
reads CSS); `wb-spend.test.mjs:163` (column order, optional).

**DELETE:** `app.test.mjs:24`, `:96`, `:67` (duplicate of
`wb-changes.test.mjs:465-497`); `wb-daemon.test.mjs:221`;
`wb-detach-link.test.mjs:286`; `wb-console.test.mjs:1731`, `:1743`, `:1756`,
`:1772` (repeat `wb-daemon.test.mjs:30-103`); `wb-changes.test.mjs:32`.

**MERGE:** `wb-daemon.test.mjs:30`, `:40`, `:52`, `:99`; geometry tables in
`wb-console.test.mjs:424-1440` → `wb-geometry.test.mjs` against `WBGeometry`
(about 1,000 lines out of `wb-console.test.mjs`); purity tests
(`wb-geometry:166`, `wb-console:558`, `:694`, `:702`, `:1449`, `:1544`, `:1637`,
`:2316`, `wb-changes:251`, `:456`, `:667`, `wb-agents:201`); `wb-fail.test.mjs:321-354`;
`wb-changes.test.mjs:465`, `:485`, `:489`, `:493`; `wb-columns.test.mjs:36`,
`:323`; `wb-fleet.test.mjs:105`, `:112`; `wb-runs.test.mjs:137`, `:144`, `:155`,
`:160`; `wb-console.test.mjs` small pure-function groups (`:1827-1892`,
`:2261-2273`, `:2329-2342`, `:2415-2433`, `:2447`/`:2454`); `wb-viewer.test.mjs:27-73`,
`:348-368`; `app.test.mjs:425`; `wb-agents.test.mjs:98`, `:216`.

**Rust pins made redundant by JS tests** (drop only the JS-fold lines; keep the
markup, CSS and wiring lines): `src/tests.rs` `:7562`, `:7600`, `:7844`, `:7954`,
`:5122`, `:5266`, geometry pins `:5435`, `:5622`, `:5814`, `:5912`, `:5994`,
`:6098`, `:6854`, `:6900`, `:7185`, and `:6359`, `:6636`, `:6779`, `:5371`,
`:8663`, `:8696`, `:8996`, `:9097`, `:8176`, `:8266`, `:7762`, `:8110`, `:1934`,
`:7081`, `:5033`, `:5704`.

**Timing:** `wb-console.test.mjs:2577`, `:2796` wait on real timers (about 1.2 s
of 4.9 s).

---

## 4. Behavior gaps found (no test today)

| ID | Gap | Evidence |
|---|---|---|
| G-1 | `adapter-support` `classify` has no case with `errored: true`; removing `&& !s.errored` is caught only by opencode. | `classify.rs` |
| G-2 | The scaffold charter write and the stale-plan removal are untested. | `adapter-support/src/scaffold.rs:78` |
| G-3 | codex: limit text on a clean exit is not tested; removing `if !exited_cleanly` (`outcome.rs:51`) survives. | codex `outcome.rs:51` |
| G-4 | usage-scan: `>=` → `>` on the `since` boundary survives in 4 scanners; kimi has no `since` test. | usage-scan scanners |
| G-5 | release: nothing tests that HTTP 429 is retried. | `ralphy-release/src/fetch.rs` |
| G-6 | pty: `kill` has no process-tree test (and kills only the direct child). | `pty/src/lib.rs:219-224` |
| G-7 | OSC 52 clipboard gating (`registerOscHandler`, `scrubClipboard`, `readClipboard`) has no node test. | `wb-console.js` |
| G-8 | `modeFor` (`wb-mode.js`), `isFolder`, `isNoteInNotesDir`, `_changesRefused`, `syncBusy` are guarded only by substring pins. | `src/tests.rs:8761` and others |
| G-9 | `spendView`/`floorNote` (`wb-spend.js:292`, `:325`) have no JS test. | `spend/tests.rs:381` |
| G-10 | The WSL liveness probe's "does not start a distro" claim is unasserted. | `peer/nudge/tests.rs:139` |
| G-11 | The argv in `internal_commands_are_listed_apart_and_still_parse` is not checked against what the daemon spawns. | `ralphy-cli/src/cli/tests.rs` |
| G-12 | Confirm-then-act wiring (`if (ok) …`) is not tested by behavior anywhere. | P0-8 |
| G-13 | Unknown whether each adapter tests its own `emit::planning`/`emit::executing` arguments; this decides whether `capture/tests.rs:509` can go. | `ralphy-cli/src/runstate/capture/tests.rs:509` |

---

## 5. Where the audit did not look (open work)

- **No mutation was executed.** Every LIES-FP above is reasoned from the code.
- Test bodies not read (names and weak-assert grep only):
  - daemon: `fleet/*`, `fleet/watchsub`, `fswrite`, `clipboard`, `pidfile`,
    `rekey`, `session/manager`, `spend/deliveries`, `kpi_tests`,
    `spend/{period,models,rows,activity,format}`, part of `textcodec`,
    `tree/search`, `auth/policy`;
  - daemon integration: most of `observe_read`, `workspace_write`,
    `file_encoding`; `fleet_command`, `fleet_console`, `fleet_usage`,
    `peer_handshake`, `security_*`, `network_bind_gate`, `session_liveness`,
    `session_roundtrip`, `repos_rekey`;
  - core: `worktree/tests.rs` (25), `github/attachments.rs` (17),
    `runner/clock.rs` (17, likely table-mergeable), `verify/parse/tests.rs` (16,
    likely table-mergeable), `changes.rs`, `checkouts`, the rest of `ledger`,
    `knowledge`, `sync`, `blob`, `git.rs`, `github/comments`, `queue_view`;
  - cli integration: `blob.rs`, `changes.rs`, `checkouts.rs`, `sync.rs`,
    `usage_recovery.rs`, `checkout_cwd.rs`, `hook_status.rs`;
  - adapters: claude `usage.rs` (14), opencode `events.rs` (27, skimmed),
    adapter-support `tests/headless.rs` (19, 8 sampled);
  - JS: the data rows of the geometry tables in `wb-console.test.mjs`
    (`:440-1440`) were skimmed, not checked value by value.
- Overlap between `ralphy-daemon/tests/*.rs` and `src/tests.rs` was checked only
  for `auth_ws.rs`, the clipboard and the argv spots.
- The time saved by merging binaries and table-driving tests was **not
  measured**. Under nextest each test is its own process, so merging binaries
  saves link and `--list` time, not per-test spawns.

---

## 6. Decisions this audit does not make

- **Production refactors.** Moving the copied skills dance into
  `ralphy-adapter-support` (3 real callers, nothing vendor-specific) and adding a
  `#[cfg(test)] run_hook` seam to cursor, gemini and copilot (the pattern exists
  in codex `outcome.rs:80`; it would turn about 20 source pins into behavior
  tests) change production code. Each needs its own issue.
- **Retiring `xtask asset-pins`.**
- **Source-grep lints** in `runstate/capture/tests.rs` (`:273`, `:509`, `:582`)
  and `session/spec/tests.rs` guard real drift but are brittle. Keep or drop is a
  policy call.
- **Structural findings outside test quality:**
  - `ralphy-cli/src/events/mod.rs` uses the `mod.rs` layout that ADR-0022
    forbids.
  - `init` has three byte-identical decision functions (`draft_decision`,
    `create_repo_decision`, `labels_decision`).
  - `init/scaffold.rs` `upsert_*` is dead code (`#[allow(dead_code)] // SUSPENDED`).
- **Kept on purpose:** the 79 Playwright scripts; the 3 adapter `#[ignore]`
  probes and `update/apply.rs:439`; no new test framework and no mutation-testing
  tool.

---

## 7. Status and measurements

Filled in while the cleanup (issue #485) is done. Every ID above gets one status: fixed,
deleted, merged, loosened, rejected (the mutation did not survive, with the
reason), or moved to a follow-up issue.

| Measure | Before | After |
|---|---|---|
| `cargo nextest run --workspace`, Windows, median of 3 | 107.8 s | |
| Rust test functions | 2921 | |
| Rust test binaries | 101 | |
| `node --test crates/ralphy-daemon/ui-tests` | 630 tests, 4.7 s | |

Measured on Windows 11 (MINGW64), 2026-09-27, at commit `d501049a`. The three
`cargo nextest run --workspace` wall times were 107.8 s, 108.2 s, 102.3 s
(median 107.8 s). The Rust test count moved from 2920 to 2921 between the
first and second run in the same session, with no source change in between;
`cargo nextest list --workspace` afterward agreed with 2921, so that count is
the stable baseline.

| ID | Status | Commit | Note |
|---|---|---|---|
| P0-1 | fixed | stage 2 | `tree.rs` `read_masks_escape_as_not_found`, `read_image_masks_escape_as_not_found`, `note.rs` `read_masks_a_missing_or_escaping_target_as_a_miss` and `observe_read.rs` `serve_repo` (all callers) now put a real `secret`, `secret.png` or `outside.note` right outside the root. Each fails when `confine::confine` is replaced by `root.join(rel)`. |
| P0-3 | follow-up | stage 2 | Doc comment reworded: the test proves the strip works, not that boot calls it. No mutation: the boot path has no test seam without a production change. The boot seam goes into the follow-up draft (Stage 5). |
| P0-4 | fixed | stage 2 | Both `cursor.rs` tests seed an operator body (`# operator\nsecrets/\n`) that differs from the gate's `*\n` and assert its bytes. They fail when the `.filter(..exists())` is removed. |
| P0-5 | fixed | stage 2 | New `production_cuts_at_the_test_module_and_not_at_a_test_item`. It fails for `production() -> ""` and for a cut at the first `#[cfg(test)]` line. |
| P0-6 | fixed | stage 2 | `runs_unwatch_stops_the_pushes` adds a positive control on the same socket (re-watch, third snapshot, `runs.dirty` arrives). It fails when `runs.unwatch` closes the socket. |
| P0-2 | fixed | stage 3 | Each crate's `tests.rs` now has `production_text`, which cuts at the `#[cfg(test)]` line that gates a `mod`, not at the first `#[cfg(test)]`. gemini `the_auth_probe_reads_no_credential` fails for `oauth_creds.json` in `plan`; gemini `autonomy_argv_is_never_downgraded` fails for `--approval-mode`/`auto_edit` in `execute`; cursor `no_adapter_side_retry_of_a_quota_stop` fails for a `loop` around the quota stop in `execute`. All three passed before the fix. |
| P0-2 siblings | fixed | stage 3 | Every `split("#[cfg(test)]")` cut in gemini, cursor and copilot now calls `production_text`. Only `lib.rs` has a test item above its test module, so only scans that read `lib.rs` were blind. Was blind: yes - cursor `model.rs` `no_default_model_id_is_baked_in` (fails for `"composer-2.5"` in `execute`), cursor `outcome/tests.rs` `every_spawn_site_in_the_crate_is_gated_or_neutralized` (fails for a `Command::new` in `execute`). Was blind: no, cut hardened - gemini `auth.rs` (second cut, `auth.rs`), gemini `command/tests.rs` `the_child_is_pointed_at_the_owned_root_and_never_the_operators` (`tasks.rs`) and `no_direct_command_new`, gemini `outcome/tests.rs` `the_prompt_is_computed_before_the_child_is_spawned`, gemini `revocation.rs` ×2, gemini `root.rs` `the_root_module_names_no_credential_file`, gemini `tasks.rs` ×3, cursor `auth.rs` `the_verdict_never_reads_an_exit_status`, cursor `command/tests.rs` `locate_cursor_delegates_to_the_shared_vendor_locator` and `no_direct_command_new`, cursor `outcome/tests.rs` `no_progress_read_from_the_stream`, copilot `outcome.rs` `no_code_changes_read`. Two whole-file `lib.rs` reads in gemini (`execute_is_plan_agnostic_and_bounds_the_commit`, `the_child_is_pointed_at_the_owned_root_and_never_the_operators`) also use `production_text` now; they were not blind. |
| P0-7 | fixed | stage 4 | Six behavior tests in `ui-tests/app.test.mjs` through `loadShell()`: `toggle` calls `wakePeerFor` on open only; `toggle` runs `destroyChangesSub` then `mountChangesSub`; `emitCreate` sends the resolved directory; the `create` action asks through `askPrompt`; `wakePeerFor` wakes only a sleeping peer; `askPrompt` settles with the name `promptSubmit` accepts. Each fails when its call is removed; the Rust pins passed. The definition needles (`function wakeable(`, `wakePeerFor(ref)`, `mountChangesSub()`, `destroyChangesSub()`, `function shouldReload(`, `function subscribeChanges(`, `emitCreate(node, kind) {`, `createDir(node) {`, `askPrompt(opts = {}) {`, `promptSubmit() {`, `promptRespond(name) {`, `await c.askPrompt({`) are gone from the four Rust tests; markup, CSS, `index.html`, the `subscribeChanges,` export and the `shouldReload` call stay pinned. |
| P0-8 | fixed | stage 4 | `the_destructive_console_clicks_confirm_first` now reads the statement right after each `askConfirm({ ... });` call: `if (ok) arrangeFence(f.id);`, `if (ok) removeFence(f.id);`, and `if (!ok) return;` for `Restart in`, `Restart session?` and both `Close this console?` sites. It fails for `if (ok) arrangeFence` -> `arrangeFence` and for a removed `if (!ok) return;` on the live console's close. G-12 stays open for a behavior test. |
| P0-9 | fixed | stage 4 | `the_console_clipboard_is_write_only_and_refused_on_replay` pins the whole write callback with whitespace removed (`term.write(a.subarray(9),replaying?()=>{replaying=false;}:undefined`), which `let replaying = false;` does not match. It fails for `term.write(a.subarray(9)); replaying = false;`. |
