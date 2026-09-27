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
`"treat it as a lead"`, close_artifacts `"✗ ## Acceptance ledger present"`.

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
| P0-10 | deleted | stage 5 | `loop_exhaustion_yields_maxcalls` and `maxcalls_outcome_is_stuck` deleted: they test a copy of production (the helper `run_headless_steps` returns its own `MaxCalls`). No mutation, because no test can reach the real loop. `run_headless_steps` stays, two stuck-counter tests use it. `headless_reason_maxcalls_maps_to_stuck` stays as the Stuck mapping test. |
| P0-10 | follow-up | stage 5 | The headless loop has no injectable step source. The seam and the test it allows are in `docs/plans/issue-485-followup-draft.md`. |
| P0-11 | fixed | stage 5 | New `each_phase_reads_its_own_model_and_effort` slices the bodies of `fn plan(` and `fn execute(` in `lib.rs` and requires `self.phase_model(Phase::X)` and `self.phase_effort(Phase::X)` for its own phase and no other `Phase::`. It fails for `Phase::Plan` -> `Phase::Execute` in `plan`'s `phase_model` call; all 93 other crate tests passed. The five argv tests stay, renamed for what they test (`the_builder_puts_the_given_model_in_argv`, `phase_model_gives_the_exec_pin_to_the_builder`, `the_builder_omits_model_when_unpinned`, `the_builder_carries_the_clamped_effort_in_argv`, `the_builder_omits_effort_when_unset`). A behavior test needs a seam: follow-up draft. |
| P0-12 | fixed | stage 5 | Unit test deleted from `config/tests.rs`. New `tests/mutate.rs` `config_set_refuses_under_held_lock` and `config_unset_refuses_under_held_lock` run the real `ralphy config` binary under a lock held by `runlock_test_child`: failed exit, `refusing to config set/unset` in stderr, settings bytes unchanged. A lock-free `config set` in the same test is the control. Each fails when its own `guard_run_lock` call in `config.rs` `run()` is deleted; the old unit test passed both times. |
| P0-13 | fixed | stage 5 | New `preflight_agents_probes_cursor_with_its_adapter_locator` slices the body of `fn preflight_agents` and requires `CliAgent::Cursor => ralphy_agent_cursor::locate_cursor().is_some(),`. It fails when the Cursor arm uses `locate_program(a.cli_name())`; the old test, renamed `check_agents_present_uses_the_given_locator`, passed. A behavior test needs injectable locator roots: follow-up draft. |
| P0-14 | deleted | stage 5 | `timer_spec_run_with_triage_chains_triage_first` deleted: it set `pre_invocation` itself (tests a copy of production). Its `triage_prelude()` value is kept as `triage_prelude_is_triage_yes_without_if_idle`, and its `# ralphy-schedule:run:` tag assert moved into `timer_spec_run_names_task_and_args`. The `install` seam goes to the follow-up draft. |
| P0-15 | deleted | stage 5 | `init_git_safety_branch_and_scaffold_end_to_end` deleted: it did the orchestration itself. `commit_decision`/`branch_decision` keep their tests in `init/run/decisions/tests.rs`. Its scaffold asserts (agent docs written, no CLAUDE.md/AGENTS.md, twice) are kept as `write_scaffold_writes_the_agent_docs_and_no_instruction_file`, with no git calls. The git-stage seam goes to the follow-up draft. |
| K-3 | fixed | stage 5 | `locked_merge_child` is `#[ignore]`, and the parent `separate_process_transactions_are_serialized` spawns it with `--ignored`. A plain run lists the child as skipped. With the child made to return at once, the parent fails (timed out waiting for `session-a.ready`). |
| §3.1 dyn_agent | deleted | stage 6 | `*_agent_is_a_dyn_agent` deleted in codex, copilot, cursor, gemini, kimi, opencode (6 found, not 7 — claude carries no such test). Compile-only; subsumed by `ralphy-cli/src/run/wiring.rs` boxing every agent as `&dyn Agent`. |
| §3.1 mint_session_id | deleted | stage 6 | `mint_session_id_is_a_fresh_uuid` deleted in copilot, cursor, gemini (3). Tests `uuid::Uuid`. |
| §3.1 charter-size | deleted | stage 6 | `plan_charter_exceeds_argv_safe_size` (cursor, gemini), copilot `exec_charter_exceeds_argv_safe_size`, gemini `the_roundtrip_fixture_carries_the_whole_charter` deleted. Asset byte-size facts, not behavior. |
| §3.1 ADR-prose | deleted | stage 6 | opencode `adr_0005_d3_amendment_separates_variant_from_effort`, cursor `the_limit_stance_is_documented`, gemini `the_limit_stance_is_documented_as_the_one_most_likely_to_be_revised` deleted. Pin ADR prose, not behavior. |
| §3.1 serde/Default | deleted | stage 6 | gemini `the_two_phase_pins_round_trip`, cursor `cursor_settings_round_trips_json`, copilot `copilot_settings_defaults_are_all_none` + `copilot_settings_round_trips_json` deleted. Pure serde round-trip/default checks. |
| §3.1 constants | deleted | stage 6 | gemini `accepts_images_is_true`, `the_auth_message_reproduces_the_vendor_sentence`; cursor `the_credit_note_names_both_units` deleted. Pin constants. |
| §3.1 copilot usage.rs | deleted | stage 6 | `copilot_usage_maps_session_rows_to_usage`, `copilot_usage_unknown_session_is_zero`, `copilot_usage_reads_no_premium_requests` deleted (duplicate `ralphy-usage-scan`'s own `session_tokens` coverage); their now-unused `seed_p2`/`usage_of`/`CREATE_USAGE` helpers deleted too. `every_effort_model_supports_low_medium_high` deleted (asserts fixture data); the dangling doc-comment reference to it in `clamp_never_exceeds_the_request` reworded. |
| §3.1 opencode/adapter-support singles | deleted | stage 6 | opencode `resolved_model_label_returns_model_or_unknown` (Option::unwrap_or getter), opencode `events.rs` `is_opencode_auth_error_takes_precedence_over_done_sentinel` (no precedence logic to test), adapter-support `detect_limit_maps_the_three_states` (tests `bool::then`), `issue_budget_new_seeds_the_default_cap` (getter), codex `xhigh_tier_effort_is_a_codex_accepted_word` + `effort_does_not_alter_the_tier_routed_model` (subsets of `tier_to_model_effort_maps_and_defaults`/`neutral_top_rungs_saturate_to_codex_high`/`build_command_threads_the_effort_through`), cursor `the_pinned_path_is_a_shape_the_vendor_classifier_accepts` (subset of the builder test) — all deleted. |
| §3.1 max_minutes | deleted | stage 6 | Cross-adapter table row 1: 11 pass-through tests found (report estimated 13) — claude `issue_deadline_zero_minutes_disables_the_cap`, codex/kimi/opencode `{honours,zero_minutes}` pairs (2 each), copilot `{honours,zero_minutes}` pair, cursor/gemini single `honours` tests — all pass through to `ralphy_adapter_support::issue_deadline`, covered by `budget.rs`'s `uncapped_beats_a_finite_budget`/`zero_minutes_is_still_bounded_by_the_run_deadline`. Red run: `budget.rs`'s `issue_deadline` body forced to always use `unbounded` — both `budget.rs`'s own tests AND codex's `codex_zero_minutes_disables_the_per_issue_cap`/`codex_honours_max_minutes_per_issue` failed; reverted. Each crate's now test-only `issue_deadline()` oracle (`#[cfg(test)]`) is unused after its callers are gone and is deleted too. |
| §3.1 classify | deleted | stage 6 | Cross-adapter table row 2: 21 pass-through tests deleted (codex 5, kimi 4, copilot 3, opencode 9) — confirmed exactly via the tests present per crate before deleting, matching the report's "about 21" and the KEEP set (`non-zero exit → Stuck`, vendor `classify_limit_*`, kimi `classify_403_*`/exit-75, opencode `classify_stuck_on_error_event`). Red run: `classify.rs`'s `classify` dropped `&& !s.errored` — the shared `ladder_matches_adr_0023_d2` test PASSED (does not cover `errored`; recorded as G-1 below), but opencode's `classify_stuck_on_error_event` FAILED, confirming it is the one test that actually covers that path today and must stay. |
| §3.1 prompt-trailer | deleted | stage 6 | Cross-adapter table row 3: `prompt_plan_*_carries_finalize_trailer` deleted in all 7 adapters (claude carried 2 asserts in one test, others 1 each = 8 asserts). Red run: the `## Finalize` section removed from `assets/prompts/plan/template.md` — `ralphy-core/tests/prompt_assembly.rs`'s `plan_prompt_artifacts_match_template_plus_overlays` FAILED (drift between template and the stored artifacts), while claude's own `plan_prompts_carry_finalize_trailer` still PASSED (it only pins the stored artifact file, which the mutation did not touch) — confirming `prompt_assembly.rs` is the test that actually catches a dropped trailer. Reverted. |
| plan_pointer | deleted | stage 6 | claude `plan_pointer_is_a_pointer_not_the_charter` deleted (`plan.rs`); codex/kimi/opencode's copies of the same `PLAN_CHARTER.len() * 50 <` assert were carried inside their A-2 `plan_charter_file_carries_full_prompt` tests and removed with those. Covered by `ralphy-adapter-support/src/sentinel.rs`'s `plan_charter_points_at_disk_artifacts` (`PLAN_CHARTER.len() < 512`). |
| A-2 | deleted | stage 6 | codex/kimi/opencode `plan_charter_file_carries_full_prompt` deleted: `fs::write` then read back tests `std::fs`, not adapter behavior. Covering test: none today — the scaffold write itself (`adapter-support/src/scaffold.rs:78`) has no direct test. Follow-up: Stage 12 G-2 adds one. |
| A-3 | deleted | stage 6 | copilot `classify_done_ignores_zeroed_code_changes` deleted: `committed` is hard-coded `true` in the call, so the ladder's Done branch is reached regardless of the "zeroed" envelope — the test never exercises the described false-friend path. `no_code_changes_read`'s doc comment (which referenced this test) reworded to state the invariant directly. |
| A-4 | deleted | stage 6 | opencode `resolved_effort_does_not_become_variant_on_argv` (lib.rs) + `resolved_effort_never_becomes_variant` (command.rs, subset of `build_command_includes_variant_only_when_some`'s first case), kimi `resolved_effort_never_appears_on_argv` deleted: the builders have no effort parameter, so nothing production could regress. |
| A-6 | deleted | stage 6 | copilot `preflight_or_bail_rejects_continue_on_auto_mode` deleted: asserts only `is_ok()`, and the result depends on whether the test host has a Copilot config on disk. `preflight_or_bail` stays production code called by all four one-shot verbs. |
| G-1 | follow-up | stage 6 | `ralphy-adapter-support/src/classify.rs`'s `ladder_matches_adr_0023_d2` has no case for `errored` (found live during the classify red run above): dropping `&& !s.errored` from `classify()` does not fail it. Only opencode's vendor-specific `classify_stuck_on_error_event` covers that path today. Stage 12 to add a case. |
| §3.2 daemon tests.rs | deleted | stage 7 | `api_desk_serves_windows_and_fences_together` (dup of `api_desk_empty_when_no_file`'s identical body assert), `xterm_asset_is_served` (subset of `root_serves_vendored_xterm`), `build_presence_carries_identity_and_uptime` (covered by `tests/auth_ws.rs`'s live `/ws` presence frame assert), `bind_addr_default_is_loopback` (tests `SocketAddr::new`, category: std), `login_gate_drops_mock_hint` (the "any 6-digit code" half is a subset of `served_ui_copy_has_no_mock_or_false_claims`'s whole-asset sweep; the "Set up two-factor first" half is an unduplicated UI-copy pin, deleted per report), `root_serves_wb_daemon`/`root_serves_wb_mode`/`root_serves_wb_fail` (the "shell loads the module" half is covered by the index.html tag-reference sweep; the in-module substring pins are weak text-existence checks, not behavior — `root_serves_wb_fail`'s `function message` half is additionally covered behaviorally by `wb-fail.test.mjs`'s `load().message(...)` calls), `translation_is_gone_from_the_served_ui` (tombstone of a permanently completed removal; no other test references "xlate"/"wbtranslate") deleted, all 9 found matching the report's count. |
| §3.3 daemon other modules | deleted | stage 7 | `autostart/tests.rs` `uninstall_targets_the_installed_task` (fully redundant with `render_uninstall_windows`, `render_enable_disable_systemd`, and `render_uninstall_and_query_launchd_name_the_service_target`, one per platform); `dispatch/spawn/tests.rs` `take_output_yields_child_bytes` (tests `FakeChild`'s own `Option::take`, defined in the test file, category: test-double self-test); `peer/nudge/tests.rs` `nudge_argv_has_no_shell_metacharacters` and `keepalive_argv_has_no_shell_metacharacters` (both strict subsets of their sibling `*_argv_is_exact` tests — an exact-vec assert already implies no stray metacharacter); `usage/tests.rs` `run_records_returns_all_lines_when_since_is_none` (subset of `run_records_skips_a_malformed_middle_line`, which also calls with `since: None`) and `the_workbench_labels_a_lower_bound_record` (source-text pin on `wb-spend.js`; `wb-spend.test.mjs`'s `"a lower_bound row marks EVERY count and says so in words (ADR-0043 D10)"` is a real behavior test through `ledger()`, strictly stronger); `desk/tests.rs` `a_fence_rect_is_sane_on_the_same_rule_as_a_window` and `a_note_rect_is_sane_on_the_same_rule_as_a_window` (both exact duplicates of the window-record `rect_is_sane` tests against the same pure function, different record types only); `cookie.rs` `a_remembered_cookie_round_trips_its_kind` (the REM branch is already asserted by `a_cookie_minted_by_an_earlier_build_still_verifies`); `roster.rs` `availability_is_presence_only` (repeats the same per-agent present→available mapping already proven by `availability_is_computed_per_roster_with_the_injected_locator`, with no aggregation logic to add coverage for); `epoch.rs` `save_then_load_round_trips` (strict subset of `bump_increments_and_persists`, which round-trips through a fresh `load_epoch_from` too); `release.rs` `the_cache_is_a_sibling_of_the_rest_of_the_store` (pure `Path::join` getter test, category: trivial getter, matches K's "path getters" category) all deleted. |
| §3.4 daemon tests/*.rs | deleted | stage 7 | `observe_read.rs` `file_read_refuses_binary` deleted: `file_encoding.rs`'s `file_read_still_refuses_binary_and_an_unknown_label` asserts the exact reason string `"binary"` (stronger than `.contains("binary")`) over the same fixture, plus the unknown-encoding case. `workspace_write.rs` `image_write_refuses_svg`, `image_write_refuses_oversize`, `image_write_refuses_bad_base64_and_a_missing_payload` deleted: `clipboard.rs`'s `decode_image_refuses_what_is_not_an_allowlisted_image` (None/bad-base64/empty/html/svg) and `decode_image_refuses_an_oversized_payload_before_decoding_it` cover the same inputs at the unit level; the wire path (verb reaches `decode_image` and the refusal reaches the client) stays proven by the kept `image_write_refuses_html_dressed_as_an_image`. `tree_watch.rs` `dirty_nudge_reaches_a_watcher` deleted: its exact scenario (watch, write, one dirty nudge) is the positive control already run at the end of `malformed_checkout_on_watch_holds_nothing`. |
| C-A1 | deleted | stage 7 | `runstate/event/tests.rs` `the_idle_reap_is_emitted_below_warn_so_it_stays_a_first_class_event` deleted: never calls a real emitter, only the local `decode` helper with a hand-built `EventFields`. |
| C-A2 | deleted | stage 7 | `runstate/event/tests.rs` `decoder_maps_kimi_planning_and_executing` deleted: decodes `ralphy_core::emit::PLANNING_MSG`/`EXECUTING_MSG` directly, never touches the kimi adapter. |
| C-A3 | deleted | stage 7 | `runstate/capture/tests.rs` `no_vocabulary_literal_outside_emit` and its `MIGRATED_EMITTERS` constant deleted: scans a 23-file hand-written list, so a vocabulary literal in an unlisted file survives. |
| C-A5 | deleted | stage 7 | `runstate/snapshot.rs` `project_is_pure_over_runstate` deleted: calls `project()` twice in the same process with no clock/pid/fs input in play, so the assertion cannot fail. |
| C-A8 | deleted | stage 7 | `init/gate.rs` `cursor_login_probe_reads_the_status_json_not_the_exit_code` and `copilot_login_probe_is_the_free_catalog_fetch` deleted: both are source-text pins on `agent_logged_in`'s match arms; `let _ = probe_cursor_login(); return cursor_logged_in(true)` survives the cursor one. |
| C-A9 | deleted | stage 7 | `init/issues.rs` `load_issues_draft_round_trips_persisted_draft` deleted: writes the draft with `serde_json::to_string_pretty` itself, then reads it back — tests serde, not the publish/resume path. |
| C-A10 | deleted | stage 7 | `events/emitter.rs` `public_ip_falls_back_to_local_when_probes_yield_nothing` deleted: makes a real HTTP call via `public_ip()`; the probes are never forced to fail. |
| C-A11 | deleted | stage 7 | `events/envelope/tests.rs` `events_doc_documents_assignee_filter` deleted: greps `docs/events.md` prose. |
| C-A13 | deleted | stage 7 | `ui/presenter/tests.rs` `enqueue_is_off_the_run_path_even_with_a_stalled_renderer` deleted: builds its own `mpsc::channel` and never calls `PresenterHandle::on_event`. |
| C-B1 | deleted | stage 7 | `run.rs` `no_print_notice_call_remains_in_run_cmd` deleted: the function it forbids (`print_notice`) no longer exists in `crates/`; the assertion cannot fail. |
| C-B2 | deleted | stage 7 | `run.rs` `resolution_byte_for_byte_when_absent` deleted: builds its own copy of the branch-mode resolution chain inline instead of calling the production resolver. |
| C-B3 | deleted | stage 7 | `delivery.rs` `layer_enqueue_is_off_the_run_path` deleted: pushes onto the `EventQueue` directly and never calls `DeliveryLayer::on_event`. |
| C-B4 | deleted | stage 7 | `pricing.rs` `refresh_if_stale_sole_production_call_is_usage_cmd` deleted: greps a hand-written list of 6 files for the call site. |
| C-B5 | deleted | stage 7 | `main.rs` `copilot_one_shots_are_wired` and `cursor_one_shots_are_wired` deleted: whole-file `contains` checks; swapping the two vendors' calls between arms survives (as the file's own comment for the still-kept, arm-scoped `gemini_one_shots_are_wired` explains). |
| C-B11 | deleted | stage 7 | `telegram/notifier/tests.rs` `live_animate_card` deleted: `#[ignore]`d live demo with no assertions, only `eprintln!`. `live_edit_dedup_against_real_telegram` is left as is — it already carries real `assert_ne!`/`.expect` assertions and is already marked `#[ignore = "hits the live Telegram Bot API; needs \`telegram setup\` first"]`, which satisfies the report's "or mark as a manual probe" alternative. |
| §3.5 cli DELETE (non C-A) | deleted | stage 7 | `runstate/event/tests.rs`: `decoder_maps_queue_built_assignee_filter` (both cases covered by `roundtrip.rs`'s `roundtrip_queue_built` and `roundtrip_queue_built_folds_its_sentinels`), `decoder_maps_the_api_degraded_from_either_execution_path` (byte-identical to the kept `decoder_maps_api_degraded_events`), `decoder_maps_sleep_and_deadline_events` (covered by `roundtrip_usage_limit_waiting`/`roundtrip_reset_reached`/`roundtrip_deadline_passed`), `decoder_maps_knowledge_consolidation_events` (covered by `roundtrip_knowledge_consolidating`/`roundtrip_knowledge_consolidated`), `decoder_maps_run_boundary_events` (covered by `roundtrip_run_started`, `roundtrip_run_started_folds_the_no_deadline_sentinel`, `roundtrip_run_finished`, `roundtrip_run_finished_no_work`) all deleted — each hand-builds `EventFields` for a shape `roundtrip.rs` already proves through the real `ralphy_core::emit::*` call. `runstate/fields.rs` `effort_field_doc_names_five_rung_lexicon` deleted (doc-comment text pin). `runstate/state/tests.rs`: `plan_written_with_zero_steps_is_infeasible` deleted (subset of the renamed `needs_split_upgrades_infeasible`'s first half) and the decoder half of that test removed (duplicate of `roundtrip_needs_split`); `planned_status_wire_is_additive` deleted (covered by `snapshot.rs`'s `every_issue_status_is_known_to_the_runs_panel`, which cross-checks every `IssueStatus` variant's `status_wire()`/`is_terminal()` against the served panel). `init/scaffold.rs`: `upsert_appends_block_when_absent`, `upsert_replaces_existing_block_in_place`, `upsert_preserves_following_h1_sibling_section` deleted — all exercise `agent_skills_block`/`upsert_agent_skills_block`, both `#[allow(dead_code)] // SUSPENDED`. `init/wizard.rs` `init_state_path_is_under_gitignored_ralphy_dir` deleted: `init_state_path()` is a one-line `Path::join`, category trivial getter (correction: `ralphy-core/src/types.rs`'s own `init_state_path_is_under_ralphy_dir` was cited here as the covering test, but it is itself one of K's core "3 path getters", deleted in this same stage's core commit — the actual category for both is trivial getter, not cross-crate duplication). `init/skills.rs` `skill_names_is_non_empty_and_contains_to_issues` deleted (asserts a hardcoded constant array). `events/emitter.rs` `source_prefixes_slug` deleted (trivial one-line formatter). `events/sink/delivery.rs` `phase_sleeping_wins_over_executing_issue` and `phase_starting_before_any_issue` deleted: `phase()` is a one-line delegate to `RunState::run_phase()`, already exhaustively tested by `runstate/state/tests.rs`'s `run_phase_reports_sleeping_over_executing`. `ui/tests.rs`: `render_plain_executing_is_none` deleted (exact duplicate assertion inside the larger `render_plain_line`-sweep test), `usage_lite_is_alias_of_core_usage` deleted (`UsageLite` is `pub type UsageLite = ralphy_core::Usage`, a bare alias — compile-only, same category as §3.1's `dyn_agent` deletions). Not resolved with confidence, left in place: `init/gate.rs:374`/`:458` (line numbers already shifted twice this stage on top of Stage 5's edits; no candidate in range showed a clear duplicate), `events/config.rs:~200` (`round_trips_slug_entry_and_env_override_wins` is a legitimate multi-behavior test), `run/wiring/tests.rs:449` (`plan_agent_gemini_is_accepted` uniquely exercises clap acceptance of `--plan-agent gemini`), `ui/tests.rs:932` (`bar_label_no_colour_emits_no_ansi` is the only `bar()`-based test that also asserts no-ANSI). |
| §3.6 cli rest DELETE | deleted | stage 7 | `config/tests.rs`: `help_lists_verify_command` and `help_lists_events_keys` deleted — both are subsets of `every_registry_key_is_handled_by_all_subcommands`'s loop, which already asserts `supported_keys_help().contains(k)` for every `k` in `SUPPORTED_KEYS` (including `verify.command`, `events.url`, `events.token`). `cli/tests.rs`: `init_subcommand_is_registered`, `triage_subcommand_is_registered`, `schedule_subcommand_is_registered` deleted (pure clap-registration checks; `update_subcommand_is_registered_and_defaults_to_the_rc_channel` keeps both its registration check and its behavior per the report's note to keep "the rc-default part"); the `run_help_lists_all_flags` tail asserting `CliAgent::from_str("opencode"/"open-code", ...)` removed — byte-identical to `cli_agent_accepts_opencode_spelling`. `telegram/config.rs`: `toml_round_trips_token_and_chat_id`, `toml_round_trips_without_chat_id` deleted (plain serde round-trips of a struct with no custom (de)serialize logic). `triage/tests.rs` `retriage_edits_existing_marked_comment` deleted (strict subset of `consolidate_upserts_marked_comment_then_swaps_labels`). `daemon.rs` `status_line_shows_avatar_then_name` deleted (`format_status_line` is a one-line `format!("{} {}", ...)`; no other test names it, category: trivial formatter, same as `events/emitter.rs`'s `source_prefixes_slug`). |
| K-1 | deleted | stage 7 | `tests/release_profile.rs` (whole file) deleted: `workspace_cargo_toml_has_release_profile_keys` does a whole-`Cargo.toml` `contains("strip = true")` etc.; renaming `[profile.release]` survives. One binary for one grep, no covering test needed (category: source-text pin over build config, not behavior). |
| K-2 | deleted | stage 7 | `tests/queue/selection.rs` `view_and_run_agree_under_assignee_filter` deleted: despite the name, no assignee filter ever runs — the "filtered subset" is hand-built directly as the input `queue`, not produced by the real `--assignee` fetch-time filter. `view_and_run_agree_issue_for_issue` (kept, same file) already proves the view/run blocked-by parity this test claimed. |
| §3.7 core DELETE | deleted | stage 7 | `src/stop.rs` `the_flag_round_trips_and_clears` deleted: the module's own doc comment states the hazard directly — this unit test sets the process-global stop `AtomicBool` inside the same test binary as every other `ralphy-core` unit test (including ones that run `run_queue_with`, which polls the same flag), so it is a source of exactly the leak the module warns against; `tests/stop.rs` already carries the behavioral round-trip coverage in its own serialized binary. `src/types.rs`: `run_lock_path_is_under_ralphy_dir`, `init_state_path_is_under_ralphy_dir`, `references_path_is_under_ralphy_dir` deleted — all three are `Path::join` + `starts_with`/`ends_with` checks on trivial getters (category: trivial getter, no behavior to regress). `serde_round_trip` deleted in `diagnosis.rs`, `issues_draft.rs`, `triage_draft.rs` (×3): each builds a value, serializes, deserializes, and asserts equality with derived `Serialize`/`Deserialize` and no custom logic — category: serde. `github/references.rs` `e2e_references_for_bioledger_29` deleted: `#[ignore]`d, hard-coded to a specific external private repo (`paulocorcino/bioledger-platform`), needs live network + `gh` auth. `github/issues/tests.rs` `from_ghissue_maps_all_fields` deleted after confirming (per the plan's order of operations) that `parse_issue` calls `Issue::from(g)` directly — `parses_issue_with_labels` and `tolerates_missing_body_and_labels` already exercise the same field mapping through the public `parse_issue` entry point, which goes through `From<GhIssue>`. |
| S-4 | deleted | stage 7 | `ralphy-proc-util/src/cursor.rs` `no_cursorignore_in_proc_util` deleted: hand-written recursive source-text grep for `.cursorignore`, category: source grep, not behavior. The report's suggestion to instead assert the exact folder listing in the "creates" test is left for a fix-a-lying-test stage — this stage only deletes. |
| J-2 | deleted | stage 7 | `wb-daemon.test.mjs` `"the resume debounce is exported so both triggers share one threshold"` deleted: only `typeof RESUME_DEBOUNCE_MS === "number"`. |
| J-3 | deleted | stage 7 | `wb-console.test.mjs` `"the resume thresholds stay named, not inlined at the call sites"` and `"the dormancy thresholds stay named, not inlined at the call sites"` deleted: both are `typeof X === "number"` existence checks plus a constant-vs-constant numeric comparison, neither of which can see a call site; no other test in the suite checks `RESUME_DEBOUNCE_MS < RESUME_HIDDEN_MS` or `DORMANT_AFTER_MS > RESUME_DEBOUNCE_MS`, so this coverage is lost, matching the report's own description of these two tests as unable to catch what their names claim. |
| J-7 | deleted | stage 7 | `wb-notes.test.mjs` `"the cheat sheet names every mark the editor actually recognises"` deleted: checks the `MARKDOWN_HELP` data table only (membership + shape), never the editor's actual mark recognition. |
| J-8 | kept | stage 7 | `wb-window-state.test.mjs` shrank to 158 lines (11 tests) since the report's line numbers were taken — none of today's tests sits at `:485`, and the file no longer holds enough content for that offset to exist. The closest match by the report's description ("first assert contradicts the name") is `"watchingOf answers false for a window with no session to watch"` (its first assert is `watchingOf(live(4, true)) === true`), but no other test in the file duplicates its live/asleep/spawning/initWindow/null coverage — deleting it would be a net loss, not a duplicate removal. Left in place; the misleading name is a rename candidate for a LIES-FN-loosening stage, not this one. |
| §3.9 JS DELETE (non-J) | deleted | stage 7 | `app.test.mjs` `"projectBadge answers per project, and hides a failed read"` deleted: `app.js`'s `projectBadge(slug)` is a one-line delegate to `window.WBChanges.projectBadge(this.changesCount, slug)`, already covered end to end by `wb-changes.test.mjs`'s five `projectBadge` tests (`:463`–`:497`, no-count/never-read, anti-aggregate, failed read, clean tree, dirty count). The two other line numbers the report grouped with it (`app.test.mjs:24` `"shell() builds the whole component off an empty document"`, `:96` `"the shell-wide changes flash is a STRING, and the per-project errors are a MAP"`) test unrelated facts about `app.js`'s own state literal, not anything `wb-changes.test.mjs` covers — left in place; not resolved with confidence. `wb-detach-link.test.mjs` `"the heartbeat constants are the ones the callers read off the module"` deleted (asserts 4 hardcoded constant values; `KEY`'s value is independently exercised by `"the registry is stored under wb.detach.v1 as a versioned record"`). `wb-changes.test.mjs` `"a change set folds to its count and one entry per row"` deleted: `"the list model carries a status marker per row"` is a strict superset (7 statuses vs. 3, the identical `renamed`-entry `deepEqual` shape, plus the mark array and count). |
| §3.8 small crates DELETE | deleted | stage 7 | `ralphy-proc-util/src/cursor.rs` `indexing_gate_allows_with_the_optout_file` deleted: `the_gate_writes_nothing_when_already_protected` already proves `indexing_gate(...)` succeeds AND leaves an existing opt-out byte-for-byte unchanged — a strict superset of the bare `is_ok()` check. `ralphy-pricing/src/lib.rs` `unknown_model_never_returns_some_zero` deleted: the exact assertion `table.cost_usd("big-pickle", &tokens) == None` already appears inside `PriceTable::from_layers`'s own seed/overlay test. `ralphy-pricing/src/floor.rs`: the `assert_eq!(rows.len(), 39, ...)` line removed from `every_former_defaults_id_prices_identically_from_seed_and_overlay` (counts its own hardcoded array; the per-id loop below it is the real assertion and is kept). `ralphy-run-snapshot/src/document.rs` `snapshot_path_is_runid_keyed_under_runstate`: the assertion that repeated `snapshot_dir(...).join(...)` (the production expression itself) replaced with a literal-path pin (`Path::new("/repo/.ralphy/runstate/01ABC.json")`), per the report's "pin the literal path" instruction — a test-only strengthening, no production change. `ralphy-release/src/fetch.rs` `the_default_endpoint_is_the_list_not_latest` deleted: asserts substrings of the `DEFAULT_RELEASES_URL` constant (category: constant). `ralphy-usage-scan/src/codex.rs` `missing_codex_dir_contributes_zero` deleted: an absent directory reads as empty through `fs::read_dir`'s own error path, not custom `scan_codex` logic (category: std-equivalent). `xtask/src/release_cmds/tests.rs` `a_kind_is_never_silently_widened` deleted (+ the now-unused `parse_fragment`/`Kind` import): its "docs is refused" half duplicates `changelog.rs`'s `an_unknown_kind_is_refused_by_name` (tests "chore" instead), and each individual kind it loops over is already exercised by other tests in `changelog.rs` (`Kind::Feature`, `Kind::Fix`, etc.). `xtask/src/changelog.rs` `the_history_round_trips_through_json` trimmed to `an_absent_topic_field_stays_absent_in_json`, keeping only the "no `topic` key" assert per the report; the derive-serde round-trip (`to_string_pretty`/`from_str`/`assert_eq!`) and the `"kind": "feature"` substring check removed. |
| A-1 | follow-up | stage 8 | codex `the_dance_is_not_reimplemented_locally` still PASSES with the stale clear (`remove_path`) removed from codex `skills.rs` (seen). The exposure loop is already copied in codex, copilot and cursor `skills.rs`, so no test-only change can see an edit to one copy. Test unchanged; the shared-function seam is in `docs/plans/issue-485-followup-draft.md`. |
| A-5 | fixed | stage 8 | gemini `the_triage_command_includes_every_attachment_directory` now slices the body of `triage_issues` and requires `add_include_directories(&mut cmd, &command::attachment_dirs(req.image_paths))`. It fails for `&[]` in that call; it passed before. A pin, because the verb builds and runs the child in one function: a behavior test needs a seam. |
| A-7 | fixed | stage 8 | `assets.rs` `materialize_assets_clears_extracts_and_writes_gitignore` asserts `.gitignore == "*\n"`. It fails when the body is `"*.log\n"`. |
| A-8 | fixed | stage 8 | opencode `parse_limit_zen_usage_limit_error` asserts `Some(None)`. It fails when the limit branch invents a hint from the message. |
| D-1 | fixed | stage 8 | `render_install_windows_runkey` and `render_install_windows_falls_back_to_windows_powershell` assert `'/usr/local/bin/ralphy' daemon *>>`. Both fail for `'{exe}' *>>` in `autostart.rs`. |
| D-2 | fixed | stage 8 | `systemd_unit_has_execstart_and_wantedby` asserts `ExecStart=/usr/local/bin/ralphy daemon\n`. It fails for `ExecStart={}` without `daemon`. |
| D-3 | fixed | stage 8 | `distro_liveness_answers_without_starting_anything` now pins the argv inside `is_distro_running`: `.args(["--list", "--running", "--quiet"])`, and no `"-e"` or `"-d"`. It fails for `--running` dropped from the argv; the old test passed. The mutation run did not start a distro (a `-e` mutation would have). G-10 is covered by the same pin. |
| D-4 | fixed | stage 8 | `otpauth_uri_carries_secret_and_params` compares with the literal `secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&`. It fails when `secret_base32` encodes hex. |
| D-5 | kept | stage 8 | `session/spec/tests.rs` `the_optin_key_matches_the_adapters_own_schema`, `the_console_name_key_matches_the_adapters_own_schema`, `the_gemini_root_layout_matches_the_adapters_own`: source pins over another crate, a known weak guard. The daemon may not depend on the adapter crates, so a pin is the only check; the behavior is covered by `an_unreadable_settings_file_never_names_a_console` and `gemini_launches_under_the_owned_root_and_its_policy`. |
| D-6 | fixed | stage 8 | `every_launchable_vendor_has_a_store_path_resolver` reads only code lines of `interactive_records` (comment lines dropped, whitespace removed) and requires `let <v>=scan_<v>(&` and `<v>.iter()`. It fails with the kimi scan and its `.chain` commented out; the old test passed. |
| D-7 | fixed | stage 8 | New `wb-spend.test.mjs` test `"the floor note says in words why the total is only a floor"` asserts the exact note for no floor, unpriced volume, volume plus uncounted sessions, sessions only, and neither. It fails when `floorNote` always returns `""`; the Rust pin passed under it. This closes the `floorNote` half of G-9; `spendView` still has no JS test. The `!js.contains("toFixed")` half (LIES-FN) is left for Stage 9. |
| C-A4 | fixed | stage 8 | `every_issue_status_is_known_to_the_runs_panel` checks `GLYPH` and `LABEL` each on its own. It fails when `blocked` is deleted from `LABEL` in `wb-runs.js`; the whole-file check passed. Not merged with the step-status test: a merge is a rule-3 change. |
| C-A6 | fixed | stage 8 | `deadline_event_sets_terminal_summary` asserts `Some("deadline reached before #7")`. It fails for `stopped before #7`. |
| C-A7 | fixed | stage 8 | `newcomers_go_last_in_all` pins the whole `Agent::ALL` order. It fails when Claude and Codex are swapped. |
| C-A12 | fixed | stage 8 | The last block of the first poller test is trimmed and the test renamed `poller_emits_plan_step_on_checkbox_transition`. With `reset_from_written` emptied, the old test passed and `a_fold_seeded_baseline_suppresses_an_already_checked_step` fails, so the sibling owns that behavior; its doc comment no longer points at the trimmed block. |
| C-A14 | fixed | stage 8 | `queue_bar_label_fits_the_terminal_width` runs at 30 columns (the label is 56) and asserts `"▱▱▱▱▱▱▱ 0/7 (pending #217 #2…"`, width 30. It fails when the pending list is not cut; at 60 columns neither it nor the ten-column test saw that. `render_active_line_survives_a_ten_column_terminal` asserts the exact line `"⚙\u{fe0f} #31  · claude-opus-4 · 1:05 / 45:00"`: the tail is never cut, so the report's `display_width <= 10` is false for this design. It fails when the title is left whole once it cannot fit. |
| C-B6 | kept | stage 8 | `cli_manifest_pins_ureq_excludes_reqwest_tokio`: a weak text lint on the direct dependency, not a lie (the report's own correction). No change. |
| C-B7 | fixed | stage 8 | `header_face_is_stable_per_title_but_varies_across_titles` asserts that 16 titles draw more than one face. It fails when `header_face` always returns the first face. |
| C-B8 | deleted | stage 8 | `update.rs` `this_build_never_reports_itself_behind` deleted. It passed with `v > current` changed to `v >= current` in `ralphy-release` `standing`, and so did the report's fix (a release with the build's tag): a dev or CI build is always `ahead`, and `standing` returns `Ahead` before it compares. Covering test: `ralphy-release` `the_newest_build_is_level`, which fails under that mutation. |
| C-B9 | fixed | stage 8 | `render_table_balance_carries_tokens_and_usd` uses million-scale rows and asserts `["balance: 2.1M tok · ~$25.50"]`. It fails when only the first row is priced. The shared fixture prices to `~$0.00`, so it could not show a USD value. |
| C-B10 | fixed | stage 8 | `presence_gate_warns_and_proceeds_when_held_alive` and `presence_gate_takes_over_stale_lock` assert the exact warning. Both fail when the two warnings are swapped. |
| S-1 | fixed | stage 8 | `locate_program_prefers_path_over_local_bin` puts the same `tool` in PATH and in `~/.local/bin`, and asserts the PATH one. It fails when `~/.local/bin` is checked first. |
| S-2 | fixed | stage 8 | Renamed `pid_is_alive_detects_own_process_and_not_an_exited_one`: it spawns the test binary with `--list`, waits for it, and asserts its pid is not alive. It fails when `pid_is_alive` answers `true` for every pid. |
| S-3 | fixed | stage 8 | `indexing_gate_allows_when_there_is_no_repository_at_all` asserts the folder is still empty. It fails when a directory with no repository is treated as a root. |
| S-5 | fixed | stage 8 | `malformed_json_leaves_prior_cache_bytes_unchanged` asserts `Some(36.75)` from the prior cache. It fails when `load()` ignores the cache (`110.25`, the seed floor). The tail of `http_503_after_retries_leaves_no_or_prior_cache` is trimmed: it read `PriceTable::defaults()`, which never reads the cache, so it could not see the fetch; seed pricing is owned by `floor.rs` `every_former_defaults_id_prices_identically_from_seed_and_overlay`. |
| S-6 | fixed | stage 8 | `list_runs_lists_a_live_run` gives `01LIVEA` the later `started_at` and expects `["01LIVEB", "01LIVEA"]`. It fails for a sort by runid only. |
| S-7 | fixed | stage 7 | Fixed in Stage 7 (`e81d0ae8`, the literal-path pin). Verified here: it fails when `snapshot_dir` joins `runs` instead of `runstate`. |
| S-8 | fixed | stage 8 | `existing_vendor_readers_resolve_fixture_candidates` asserts one result per candidate before the `zip`. It fails when `resolve_models` drops the opencode candidate. |
| S-9 | fixed | stage 8 | `unmatched_workspace_yields_null_project` registers a repo whose key is not the workspace. It fails when an unmatched workspace falls back to the first repo. |
| S-10 | fixed | stage 8 | `output_is_deterministic_and_idempotent` refreshes a seed with its providers in reverse order and asserts the same output as the sorted seed. It fails for an emitter that keeps the input order (applied as a hand-written emitter, because every serde_json map sorts). |
| S-11 | fixed | stage 8 | `input_cut_short_gives_rows_and_never_panics` asserts `[("text", "Hello there")]` for the cut HTML and no rows for the three cut JS strings. It fails when text before a cut tag is dropped. |
| J-1 | fixed | stage 8 | `"the dirty mark waits for the daemon's ack and comes back on a refusal"` reads the mark through `externalChange`: after `saveFailed` the pane keeps its bytes, after `saveDone` it takes the disk's. It fails when `saveFailed` no longer sets `dirty`. |
| J-4 | follow-up | stage 8 | `"resumeAll and setStaleProbe are exported like the rest of the module's seam"` passes with `setStaleProbe` made a no-op (seen). The probe is read only by the private `isStale`, and a window joins `wins` only through the DOM path the harness does not reach, so no test-only change can see it. Test unchanged; seam in `docs/plans/issue-485-followup-draft.md`. |
| J-5 | fixed | stage 8 | The fence-cap test lands desks of 0, 11 and 12 fences through the real GET: `atFenceCap()` is false, false, true, and `createFence()` is refused at 12. It fails for `atFenceCap` answering `false` always. |
| J-6 | fixed | stage 8 | Both `exitNote` tests use `assert.equal` on the whole note, with verbs `triage` and `push` (not `run`). Both fail when the note hard-codes `run`. |
| J-9 | fixed | stage 8 | The exact `TILES` table now lives only in `wb-geometry.test.mjs` (the module's own file), one test per row with `deepEqual` on `want`; the relation-only test and the byte-identical copy in `wb-console.test.mjs` are gone, and the table's comment moved with it. It fails when every tile is put in the first column; the relation test passed. |
| §3.8 pty cmd_hazard ×10 | merged | stage 9 | Ten `cmd_hazard_*` tests (14 cases) are one table, `cmd_hazard_names_the_first_character_cmd_would_read`, with 14 named rows. It fails when `'^'` leaves the bare list. |
| §3.8 pty.rs refusals ×2 | merged | stage 9 | `batch_program_refuses_an_ampersand_argument` + `batch_program_refuses_a_percent_argument` → `batch_program_refuses_a_bare_special_character`, 2 rows, one shim. It fails when `'%'` leaves the always-refused list. |
| §3.8 pty rename | fixed | stage 9 | `kills_and_waits_the_process_tree` renamed `kills_and_waits_the_child`: `kill()` kills only the direct child. |
| §3.8 pricing floor ×3 | merged | stage 9 | The three adapter-id floor tests → `every_adapter_emitted_id_prices_through_the_floor`, 12 rows, each with an exact USD (8 were `is_some()`). It fails when the provider-prefix fallback is removed. |
| §3.8 pricing fetch.rs:499 | merged | stage 9 | `transport_error_falls_back`, `http_429_after_retries_falls_back` and `http_503_after_retries_leaves_no_or_prior_cache` → `a_failed_fetch_falls_back_and_leaves_the_cache_alone`, 3 rows; every row checks both "no cache created" and "prior cache kept". It fails when 429 is not retried. |
| §3.8 pricing fetch.rs:579 | loosened | stage 9 | `manifest_excludes_core_and_agent_crates` reads dependency names from the parsed manifest tables, not the whole text. A comment naming `tokio` and `ralphy-core` passes; a `tokio` dependency fails. |
| §3.8 pricing lib.rs:423 | loosened | stage 9 | `opus_pipeline_resolves_via_synthesis_to_seed` drops the two private-field asserts (`seed`, `overlay`). Moving the opus rate into the overlay passes; `synthesize` mapping `claude-` to a wrong provider fails. |
| §3.8 proc-util tests.rs:213 | merged | stage 9 | `find_program_resolves_a_windows_cmd_shim` + `find_program_skips_extensionless_shim_when_cmd_present` → `find_program_resolves_the_cmd_of_an_npm_shim` (Windows), 2 rows. The non-Windows half is the non-Windows branch of `find_program_locates_a_file_on_the_search_path`. It fails when the PATHEXT check on the bare file is `||`. |
| §3.8 run-snapshot document.rs:218/:250 | merged | stage 9 | → `missing_fields_parse_permissively`, 2 rows; the serde round trip and `SNAPSHOT_VERSION == 1` are dropped. It fails when `phase` loses `#[serde(default)]`. |
| §3.8 run-snapshot read.rs:314 | merged | stage 9 | `list_runs_ignores_a_stop_sentinel` into `list_runs_ignores_non_json_entries`, 2 rows. It fails when `stop_path` ends in `.stop.json`. |
| §3.8 usage-scan gemini.rs:336 | merged | stage 9 | `the_record_is_flagged_a_lower_bound` is one assert in `a_single_turn_folds_total_minus_input_as_billable_output` (same fixture). It fails for `lower_bound: false`. |
| §3.8 usage-scan kimi.rs:538 | merged | stage 9 | `kimi_code_strips_model_prefix` is one assert in `kimi_code_counts_only_turn_scope_and_strips_the_model_prefix`. It fails when the prefix is not stripped. |
| §3.8 usage-scan opencode.rs:267, copilot/tests.rs:420 | merged | stage 9 | The direct-match case is already in `opencode_attributed_record_carries_git_actor_email` and `copilot_attribution_covers_matched_and_unmatched_cwd` (`find_repo` compares strings, so a literal path is no new case). Both fail when the direct match in `find_repo` is disabled. |
| §3.8 xtask release_cmds/tests.rs:180/:303 | merged | stage 9 | → `an_unrecorded_version_publishes_the_exact_pointer_and_is_not_announced`: exact body and `announce = no`. It fails when the pointer branch announces. |
| §3.8 xtask changelog.rs:584 | merged | stage 9 | `topic_and_headline_are_read_from_the_front_matter`, `a_fragment_is_its_kind_and_its_prose`, `windows_line_endings_parse` → `a_fragment_parses_into_its_front_matter_and_its_prose`, 3 rows, whole `Entry`. It fails for `topic: None`. |
| §3.8 xtask ui_copy/check/tests.rs:191 | loosened | stage 9 | Lines are compared with their runs of spaces collapsed; the closing-sentence pin is dropped. A wider rule column passes; a wrong total fails. |
| §3.7 core selection.rs:190 | merged | stage 9 | `only_issue_ignores_stop_before` + `forced_issues_list_ignores_stop_before_across_the_list` → `forced_issues_ignore_stop_before`, 2 rows. It fails when the stop-before gate ignores the forced list. |
| §3.7 core selection.rs:249/:287/:347 | merged | stage 9 | `human_return_label_skips_issue_and_continues`, `reparked_issue_is_not_reworked_on_next_run`, `custom_mapped_human_return_label_skips` → `human_return_labels_skip_their_issues_and_the_queue_continues`: one queue in one repo, one issue per case. It fails when `ready-for-human` is not a human-return match. |
| §3.7 core close_artifacts.rs:6/:72 | merged | stage 9 | Same one-verified-line ledger: → `green_close_writes_evidence_and_adds_no_label_for_a_verified_ledger`. It fails when the review label is applied at `review_only == 0`. |
| §3.7 core lifecycle.rs:141 | merged | stage 9 | `report_carries_commit_count_and_oneline` is two asserts in `works_issues_in_order_and_closes_each_green` (both run three green issues). It fails when `log_oneline` drops a line. |
| §3.7 core events.rs:14 | merged | stage 9 | `runner_emits_plan_written_steps_and_plan_opened_closed_snapshots` into `pins_green_run_vocabulary`: the checked-step content of `steps_json` is one more assert. It fails for `steps_json = "[]"`. |
| §3.7 core exec_usage ×2 | merged | stage 9 | `exec_usage_single_attempt_keeps_model` + `exec_usage_without_model_stays_unattributed` → `exec_usage_single_attempt_keeps_its_model_or_none`, 2 rows. It fails when `fold_usage` picks a record without a model. |
| §3.7 core verify.rs:215 | kept | stage 9 | `green_issue_writes_one_plan_and_one_execute_ledger_line` is not a duplicate of `runner/tests.rs` `green_close_runs_through_fakes_only`: it is the only test that sees `run_queue` wire the real `FileLedger`; the fake test injects its own sink. Merging would lose that case. |
| §3.7 core limits.rs:6/:378 | merged | stage 9 | `stop_on_limit_opt_out_stops_as_limit` + `limit_no_reset_stops_when_stop_on_limit` → `stop_on_limit_opt_out_stops_as_limit`, 2 rows (reset / no reset). It fails when the no-reset wait ignores `stop_on_limit_exec`. |
| §3.7 core prompt_assembly overlays | merged | stage 9 | `overlays_define_exactly_the_known_slots` into `plan_prompt_artifacts_match_template_plus_overlays`: the exact slot-name set replaces the count there. It fails for a renamed overlay slot. |
| §3.7 core effort.rs | kept | stage 9 | `tests/effort.rs` into `src/effort.rs` removes a test binary: left to Stage 10 (integration binaries). |
| §3.7 core protocol.rs ledgerless | merged | stage 9 | `ledgerless_plan_fails_the_lint_pins_310` is case (a) of `ledger_presence_check_covers_absent_blank_and_unparsable`. It fails for `ledger_present = true`. |
| §3.7 core protocol.rs checks | loosened | stage 9 | `clean_plan_passes_every_check`, `unticked_step_fails`, `bare_noticed_step_fails_reasoned_one_passes`: `checks.len() == 7` becomes a control (an empty plan fails); `checks[0]`/`[1]` become a lookup by label. An extra first check passes; each still fails for its own mutation. |
| §3.7 core markdown.rs | merged | stage 9 | Three `section_after_heading` tests → one table, 3 exact rows. It fails when the section runs to the end of input. |
| §3.7 core github/labels | merged | stage 9 | Three `parse_triage_mapping` tests → one table, 3 rows. It fails for `cells[1]`. The two deletions the audit names here are not identified in the report and are left alone. |
| §3.7 core github/labels/tests.rs:36 | loosened | stage 9 | `names.len() == 11` dropped; each expected name is still required. A twelfth spec passes; a renamed `HITL` fails. |
| §3.7 core settings.rs 4 → 2 | merged | stage 9 | `agent_section_round_trips_through_save` into `round_trip_set_and_unset`; `require_verify_gate…` + `queue_assignee…` → `typed_keys_default_unset_and_round_trip`, 3 rows. They fail for `#[serde(skip)]` on `assignee` and for a no-op `set_agent_settings`. |
| §3.7 core parsers 23 → 7 | merged | stage 9 | `blocked.rs` `parse_blocked_by` (6 rows), `parse_parent` (6), `structured_refs` (3), `referenced_issues` (4); `plan.rs` (2); `github/issues` `parse_issue_url` (3), `parse_issue_state` (2). Each table fails under its own mutation. |
| §3.7 core init_session triage | loosened | stage 9 | `triage_prompt_carries_the_charter_and_names_its_inputs`: charter prose asserts dropped, the builder's inputs kept. A reworded charter sentence passes; a prompt without the issue numbers fails. |
| §3.7 core prompt_ledger.rs:17 | loosened | stage 9 | The verbatim criterion constants are gone; the criteria come from the parsed example. A reworded example passes; `apply_ledger` ticking the wrong kind fails. |
| §3.7 core queue/main.rs pin | loosened | stage 9 | The `pin` helper no longer asserts the event target (18 call sites). An emit with another target passes; a renamed field fails. |
| §3.7 core minor prose pins | loosened | stage 9 | "HONESTLY", "SAME" ×2, "NARROWEST", "leads, not truths" ×2, "treat it as a lead" dropped. Each rewording passes; each test fails for a mutation of what it still asserts. `close_artifacts.rs` `"✗ ## Acceptance ledger present"` is kept: it names which check failed. |
| §3.1 claude headless_reason ×5 | merged | stage 9 | → `headless_reason_maps_onto_a_core_outcome`, 5 rows. It fails when `MaxCalls` maps to `Timeout`. |
| §3.1 claude headless_step ×3 | merged | stage 9 | With the two loop tests → `headless_loop_decides_each_call_sequence`, 3 rows over the real `headless_step`. The reset row now ends in `Done`: the old loop test expected `Stuck` either way and passed with the reset removed. It fails when a commit keeps the streak. |
| §3.1 claude parse_reset_hhmm ×5 | merged | stage 9 | → `parse_reset_hhmm_reads_the_reset_time`, 6 rows (one repeated assert dropped). It fails without `% 12` for `am`. |
| §3.1 claude is_claude_auth_error ×3 | merged | stage 9 | → `is_claude_auth_error_needs_both_signals_in_any_case`, 5 rows. It fails when the AND group is split. |
| §3.1 claude staged_plan_env ×2 | merged | stage 9 | → `staged_plan_env_flags_only_a_staged_plan`, 2 rows. It fails for `if !staged`. |
| §3.1 claude plan_prompt_for_not_staged_with_no_labels | merged | stage 9 | A row of `plan_prompt_for_selects_standard_without_label`. It fails for `all` instead of `any`. |
| §3.1 claude settings_have_stop_hook… | merged | stage 9 | Its `type` asserts and literal flags moved into `status_hooks_ride_both_phases_after_the_guard_and_the_sentinel`. It fails for `autoCompactEnabled: true`. |
| §3.1 claude status.rs:313 | loosened | stage 9 | `every_child_shape_is_handed_the_status_file` binds the watcher variable and reads production text without whitespace. A renamed watcher passes; a missing `STATUS_ENV` fails. |
| §3.1 gemini exit→Blocked ×4 | merged | stage 9 | → `every_named_exit_stops_with_its_sentence`, 7 exit rows and each control once. It fails when exit 54 maps to `Other`. |
| §3.1 gemini usage.rs ×3 | merged | stage 9 | → `the_cached_fold_derives_output_and_counts_cache_once`, the whole folded tuple. It fails when output comes from the raw field. |
| §3.1 gemini root.rs ×3 | merged | stage 9 | `ensure_is_idempotent` + `…_with_sessions_present` + `the_installation_identity_survives_reconciliation` → `ensure_is_idempotent`: bytes and mtimes of every file. It fails when `settings.json` is rewritten each run (the old byte-only test passed). |
| §3.1 gemini tests.rs:28 | loosened | stage 9 | `resolved_effort_is_stored_for_documented_discard` no longer pins the no-op `let _ =` lines. Removing them passes; not storing the effort fails. |
| §3.1 gemini tests.rs:157 | loosened | stage 9 | `execute_is_plan_agnostic_and_bounds_the_commit` reads `execute`'s body: plan parameter by name, `head_sha(` before and after the session, `committed` in either order. A swapped comparison passes; `committed` from one sample fails. |
| §3.1 gemini tests.rs:217 | loosened | stage 9 | `ralphy_adds_no_retry_of_its_own` scans the whole `plan`/`execute` bodies, not the `run` closure. A renamed closure passes; a `loop` around the spawn fails. |
| §3.1 gemini each_verb_roots… | loosened | stage 9 | Every `one_shot_command(` call takes `&one_shot_base(`, whatever the count. A call split over lines passes; a hand-rolled base fails. |
| §3.1 codex build_command_threads_the_effort_through | merged | stage 9 | The effort assert in `build_command_argv_and_env` loops over `high` and `low`. It fails for a fixed `high`. |
| §3.1 codex usage.rs:172 | merged | stage 9 | Its plan/execute half is covered by `tests.rs` `plan_and_execute_agent_paths_serialize_the_resolved_model`; the model-less case stays as `a_model_less_rollout_fold_stays_unattributed`. Each fails under its own mutation. |
| §3.1 codex tests.rs:266 | loosened | stage 9 | `plan_and_execute_use_the_resolved_effort_helpers` finds each helper call in its own function body, production text, whitespace ignored. A split binding passes; `let effort = "high"` fails. |
| §3.1 kimi resolved_effort… | loosened | stage 9 | As gemini: the no-op pins dropped. Removing the no-op passes; not storing the effort fails. |
| §3.1 kimi command.rs:285/:321 | loosened | stage 9 | The two argv tests read each flag's value by name plus the argument count. A reordered argv passes; a wrong `--output-format` fails. |
| §3.1 cursor every_one_shot_gates_before_it_spawns | merged | stage 9 | Its four cases are pairs of the relation in `every_spawn_site_in_the_crate_is_gated_or_neutralized`. That test fails when `draft_issues` loses its preflight. |
| §3.1 cursor wiring pins ×7 | loosened | stage 9 | `both_phases_report_stream_usage`, `every_run_notes_the_credit_unit_mismatch`, `execute_notes_the_degraded_calls`, `the_plan_path_routes_a_quota_stop_to_plan_limit`, `the_resume_path_is_gated_on_a_fresh_login_verdict`, `command/tests.rs` `argv_carries_no_prompt_word` and `locate_cursor_delegates_to_the_shared_vendor_locator` read production code in the phase's own body. Each passes a harmless edit (split call, comment, reordered flag, `use` import) and fails its own real mutation. |
| §3.1 copilot no-hardcoded ×3 | merged | stage 9 | → `tests.rs` `no_hardcoded_model_table`, 3 files × 4 prefixes. It fails for a pinned id in `effort.rs`. |
| §3.1 copilot tasks.rs:265, tests.rs:107/:156 | loosened | stage 9 | Each guard is found in its own phase body; the one-shot guards are a relation over every spawn. Split calls pass; each removed guard fails. |
| §3.1 cross-adapter lints | kept | stage 9 | The skill-frontmatter lint and `no_tests_directory` merges cross crates; this stage merges within a crate only. |
| §3.2 daemon :1222/:3454 GET / | merged | stage 9 | `root_serves_workbench_shell` into `root_serves_the_embedded_page`. It fails when `/` serves another page. |
| §3.2 daemon bearer ×4 | merged | stage 9 | → `the_bearer_policy_admits_only_the_right_token`, 4 rows with the legitimate case in the same test (the missing-header case is row 1 there, not added to `:4577`). It fails when any non-empty token is accepted. |
| §3.2 daemon :4184 | merged | stage 9 | `session_serves_shell_but_gates_data` into `session_policy_login_flow`: every data endpoint is refused before login. It fails when `/ws/command` joins the login allowlist. |
| §3.2 daemon :4215/:4281 | merged | stage 9 | → `api_session_reports_the_policy_and_whether_authed`, 4 rows, both fields each. It fails when a cookie-less session reads as authed. |
| §3.2 daemon api_usage_carries ×4 | merged | stage 9 | → `api_usage_carries_each_vendor_stores_interactive_records`: four stores, one router, 4 rows. It fails when the Copilot scan reads another store. |
| §3.2 daemon :2226/:2271 | merged | stage 9 | → `api_repos_reports_reachability_and_branch` (same registry). It fails for `reachable: true`. |
| §3.2 daemon desk PUT ×4 | merged | stage 9 | `:444`, `:890`, `:922` into the `:989` loop → `api_desk_put_refuses_a_bad_body_without_touching_the_store`, 8 rows. It fails without the `left >= 0` check. |
| §3.2 daemon colour-token scans | merged | stage 9 | Four marked-block hex scans → `marked_css_blocks_add_no_colour_outside_the_token_set` (4 rows); the two hover scans → `the_write_and_discard_controls_are_not_hover_gated` (2 rows). Each fails under its own CSS mutation. |
| §3.2 daemon load orders | merged | stage 9 | The six "X before `wb-console.js`" checks and the release-before-app check are rows of one order table in `every_shell_tag_resolves_and_every_asset_is_reachable` (8 rows). It fails for a dropped or late tag. |
| §3.2 daemon :8045 | loosened | stage 9 | `a_refused_change_act_reports_in_the_changes_panel` checks each Changes act, not a count of 15. A new act passes; a refusal flashed outside the panel fails. |
| §3.2 daemon :4148, :6615, :7823, :7381, :7267, :6977 | loosened | stage 9 | The exact-statement pins read whitespace-free text (`squeeze`), scope to their function, and accept either operand order where order is not the behavior. Each passes a harmless edit (wrap, swap, rename, extra fallback) and fails its real mutation. |
| §3.3 daemon autostart :50/:58/:86 | merged | stage 9 | With the two Windows install tests → `render_windows_commands`, 4 rows. It fails when `query` renders `delete`. |
| §3.3 daemon desk :90/:126/:177/:478 | merged | stage 9 | → `a_field_that_is_off_or_empty_is_not_serialised`, 11 rows with their "on" controls. It fails when `checkout` is always written. |
| §3.3 daemon desk :164/:372/:835 (+:478) | merged | stage 9 | → `a_desk_from_an_older_build_loads`, 2 documents. It fails when `fences` loses its default. |
| §3.3 daemon desk :138/:402/:419 → :947 | merged | stage 9 | Into `a_desk_with_both_a_note_and_a_checkout_round_trips`. It fails when `checkouts` is skipped. |
| §3.3 daemon desk :349/:633/:917 | loosened | stage 9 | → `context_md_defines_the_desk_terms`: the bold glossary terms, not the definitions' wording. A reworded definition passes; a renamed term fails. |
| §3.3 daemon checkout.rs:363 | loosened | stage 9 | The doc pin keeps the names an operator types and the no-vendor check; the quoted sentences go. A reworded sentence passes; a renamed key fails. |
| §3.3 daemon protocol ×3 + ×4 | merged | stage 9 | → `every_frame_round_trips` (5 rows) and `a_malformed_frame_is_refused_by_kind` (4 rows). Each fails under its own mutation. |
| §3.3 daemon dispatch/argv :888-:953 | merged | stage 9 | → `spawn_argv_carries_every_agent_through_to_the_agent_flag`, a loop over `Agent::ALL` (adds claude, codex, opencode). It fails for a missing `from_query` arm or a binary-name flag. |
| §3.3 daemon dispatch/argv :474/:483/:494/:786 | merged | stage 9 | → `list_verbs_argv_is_static`, 4 rows. It fails without `--board`. |
| §3.3 daemon session/spec :379 | merged | stage 9 | `an_unreachable_repo_root_is_not_a_naming_decision` is a row of `a_claude_console_is_unnamed_until_the_repo_opts_in`. It fails when a missing settings file opts in. |
| §3.3 daemon session/spec :147 | loosened | stage 9 | The Cursor arm of `locate_program` is found in whitespace-free production text. A block-form arm passes; the plain resolver fails. |
| §3.3 daemon watch.rs:430 | merged | stage 9 | Its case is the positive control of `unwatched_and_noise_children_emit_nothing`. That test fails when root nudges are dropped. |
| §3.3 daemon usage/tests.rs:435 | loosened | stage 9 | The tombstone sweeps the removed modal's own names only. A new `.usage-row` rule passes; `openUsage()` back fails. |
| §3.3 daemon dispatch.rs:538 | loosened | stage 9 | `app_js_holds_no_vendor_list` drops the `"claude"` literal count. A comment naming it passes; a roster literal fails. |
| D-7 | loosened | stage 9 | The `toFixed` half: `!js.contains("toFixed")` dropped from `the_spend_tab_renders_the_servers_figures_and_formats_none_of_its_own`. A comment naming it passes; reading `c.share` instead of `c.share_label` fails. |
| §3.5 cli init/gate.rs:477-576 | merged | stage 9 | 8 `evaluate_gate_*` tests → `evaluate_gate_reports_each_missing_prerequisite`, 8 rows comparing the whole blocker list. It fails when `NoAgentLoggedIn` joins `NoAgentCli` (the old `contains` tests passed that). |
| §3.5 cli init/gate.rs:341 | loosened | stage 9 | `ALL.len() == 7` was a tautology for `[Agent; 7]`; now ALL must hold every `value_variants()` entry. A duplicated vendor in ALL fails (the old assert passed it). |
| §3.5 cli yes/no decisions ×7 | merged | stage 9 | → `init/run/decisions/tests.rs` `every_prompt_decision_maps_its_answers`, 7 prompts, 43 rows. It fails when `publish_decision` accepts an empty answer. |
| §3.5 cli roundtrip.rs:96-221 | merged | stage 9 | 8 planning/executing tests → `roundtrip_planning` and `roundtrip_executing`, 4 rows each. Each fails under its own `emit` mutation. |
| §3.5 cli event/tests.rs:446 + roundtrip.rs:686 | merged | stage 9 | → `roundtrip_level_wins_over_message`, 3 rows. It fails when WARN no longer collapses to `Notice`. |
| §3.5 cli state/tests.rs:136/:318 | merged | stage 9 | → `skipped_event_sets_skipped_status`, 4 rows. It fails when a stop-before skip keeps its status. |
| §3.5 cli envelope/tests.rs:306/:339 | merged | stage 9 | → `queue_snapshot_data_matches_queue_built_data`, 2 rows. It fails when `queue.built` drops its filter. |
| §3.5 cli envelope mappings ×10 | merged | stage 9 | → `each_run_event_maps_to_its_type_subject_and_data`, 13 rows. It fails when `needs_split` loses its subject. |
| §3.5 cli snapshot.rs:336 | loosened | stage 9 | The runs-panel table pins (and the issue-status sibling) read a table by its name line and a key however indented or quoted. A re-indented table passes; a missing key fails. Not merged with `:401`, as Stage 8 decided. |
| §3.5 cli capture/tests.rs:16/:81/:163/:273 | loosened | stage 9 | Target compared with `module_path!()`; the emit target, the constants' wording and the line-shape arm count dropped (the count now reads every message pattern). Each passes its harmless edit and fails its real mutation. |
| §3.5 cli capture/tests.rs:509 | kept | stage 9 | `adapter_emit_sites_pass_the_right_arguments` is the only guard against a swapped `model`/`effort` at the nine adapter emit sites; loosening the argument texts would lose it. G-13 is its replacement. |
| §3.5 cli ui/tests.rs:1445 | loosened | stage 9 | The meter's USD is two phases priced by the table, not `30.0`. A repriced seed passes; pricing only the last phase fails. |
| §3.6 cli guard/tests.rs:31-465 | merged | stage 9 | 51 tests → 4 tables (67 rows), with the cheap rows the audit lists. Each table fails under its own mutation; the `gh secret` row fails a mutation no old test saw. |
| §3.6 cli cli/tests.rs:50/:69 | loosened | stage 9 | → `copilot_rides_the_shared_run_flags`: no Copilot-only `run` flag, the shared flags present — no count of 29. A new unrelated flag passes; `--copilot-model` fails. |
| §3.6 cli cli/tests.rs:400-457 | merged | stage 9 | → `every_cli_agent_parses_from_its_one_word_name`, a loop over `value_variants()` (adds claude and codex) plus the alias row. It fails for `cursor-agent`. |
| §3.6 cli cli/tests.rs:460-591 | merged | stage 9 | 8 subcommand parse tests → argv rows of `internal_commands_are_listed_apart_and_still_parse` (18 rows); their field asserts restated clap. It fails when `branch create` is renamed. |
| §3.6 cli cli/tests.rs:275-324 | merged | stage 9 | 4 `schedule` parse tests → `schedule_subcommands_parse`, 4 rows. It fails when `--every` is renamed. |
| §3.6 cli resolvers | merged | stage 9 | `config/tests.rs` opencode ×4 and `resolve_str` ×3, `run/wiring/tests.rs` copilot ×4 and cursor ×2, `run.rs:747` into `resolve_u64`. Each table fails under its own mutation. |
| §3.6 cli config key round trips ×5 | merged | stage 9 | → `config_keys_round_trip_through_set_and_unset`, 5 rows plus the unknown key. It fails when `unset queue.assignee` does nothing. |
| §3.6 cli run/wiring/tests.rs:339/:413 | merged | stage 9 | → `build_agent_builds_every_agent_the_cli_accepts`, a loop over all variants (adds 5 vendors). It fails when Kimi builds OpenCode. |
| §3.6 cli wiring:162-213 | merged | stage 9 | → `check_agents_present_gates_executor_and_planner`, 3 rows. It fails when the planner is not checked. |
| §3.6 cli triage:201/223/239 | merged | stage 9 | → one escalate test, 2 rows. It fails when escalate consults `decide`. |
| §3.6 cli client.rs:322-358 | merged | stage 9 | → `detect_chat_id_reads_the_last_start_message_chat`, 4 rows. It fails when a non-message update stops the scan. |
| §3.6 cli summary.rs from_report pair | merged | stage 9 | Into `from_report_buckets_and_rollup`. It fails for a wrong review-debt predicate. |
| §3.6 cli hook.rs:336/:347 | merged | stage 9 | → `transcript_fallback_when_inline_blank_or_absent`, 2 rows. It fails when a blank message does not fall back. |
| §3.6 cli tests/mutate.rs:105/:152 | merged | stage 9 | Into `branch_switch_under_held_lock_leaves_head`, on a branch that exists. It fails without `guard_run_lock`. |
| §3.6 cli run.rs:696, run/wiring/tests.rs:22-111, main.rs:377 | loosened | stage 9 | Whitespace-free production text, each call found in its own function or arm whatever the order; the six arm pins are one table. Each passes a wrapped statement and fails its real mutation. |
| §3.6 cli issues/tests.rs:113/:306, config/tests.rs:647 | loosened | stage 9 | Prose only (a doc comment, an ADR sentence, a help sentence): nothing is left once the text goes, so the three tests are removed. |
