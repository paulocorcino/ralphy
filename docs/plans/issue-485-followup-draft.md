# Add test seams for the wiring that #485 could not test

## Context

Issue #485 fixed or removed the tests that tested a copy of production code
instead of the code itself. For some of them, no honest test was possible
without a production change: the code has no way in for a test. #485 does not
change production code, so those tests were deleted, reworded, or replaced by a
source pin. This issue adds the small production seams, so each behavior gets
a test that drives the real code.

Each section below names the production code, the seam it needs, and the test
that becomes possible. The rule for every new test is in `docs/TESTING.md`:
seen red under a named mutation, and it drives production.

## Boot strips the daemon token from the environment (P0-3)

- **Code:** `crates/ralphy-daemon/src/serve.rs`, the boot path calls
  `auth::strip_token_from_env()` before any child can be spawned.
- **Today:** `crates/ralphy-daemon/tests/child_env_hygiene.rs` calls
  `auth::strip_token_from_env()` itself. It proves the strip works, not that
  boot calls it. Deleting the call in `serve.rs` leaves every test green, and
  every child inherits `RALPHY_DAEMON_TOKEN`.
- **Seam:** split the boot steps before the listener into a function that a
  test can call, or add a test that starts the real daemon binary with
  `RALPHY_DAEMON_TOKEN` set and asks it to spawn a child that reports its env.
- **Test that becomes possible:** boot the daemon with the token in its env,
  spawn a child through the normal path, and assert the child's env has no
  `RALPHY_DAEMON_TOKEN`. Seen red: delete `auth::strip_token_from_env();` in
  `serve.rs`.

## The headless loop stops at the call limit (P0-10)

- **Code:** `crates/ralphy-agent-claude/src/headless.rs`, the `for` loop in
  the headless execute path returns `HeadlessReason::MaxCalls` when it runs out
  of calls.
- **Today:** `loop_exhaustion_yields_maxcalls` and `maxcalls_outcome_is_stuck`
  were deleted. They drove the test helper `run_headless_steps`, which has its
  own copy of the loop and its own `MaxCalls` return. The pure mapping test
  `headless_reason_maxcalls_maps_to_stuck` stays.
- **Seam:** the loop has no injectable step source. Move the loop body behind
  a closure or a small trait that returns one call's result (output, exit,
  committed), so the loop can run without spawning `claude`.
- **Test that becomes possible:** run the real loop with a step source that
  commits on every call and `max_exec_calls = 3`, and assert the outcome is
  `Outcome::Stuck` after exactly three calls. Seen red: change the final
  `HeadlessReason::MaxCalls` to `HeadlessReason::Done`.
  `stuck_fires_after_two_consecutive_no_commit_calls` and
  `commit_resets_no_commit_streak` can then drive the real loop too.

## Copilot passes each phase its own model and effort (P0-11)

- **Code:** `crates/ralphy-agent-copilot/src/lib.rs`, `plan` and `execute`
  read `self.phase_model(..)` and `self.phase_effort(..)` and pass the result
  to `build_copilot_command`.
- **Today:** the argv tests call `build_copilot_command` with values the test
  picks. `each_phase_reads_its_own_model_and_effort` pins the two calls inside
  each method body, as source text.
- **Seam:** a function that returns the phase's command arguments (model and
  resolved effort) from the agent and the phase, called by both `plan` and
  `execute`. Or a recorded command in place of the spawn.
- **Test that becomes possible:** build an agent with a plan pin and an
  execute pin, run the real plan and execute paths up to the command, and
  assert the `--model` and `--effort` values in each argv. Then the source pin
  goes. Seen red: `Phase::Plan` to `Phase::Execute` in `plan`.

## The run preflight finds Cursor through its adapter locator (P0-13)

- **Code:** `crates/ralphy-cli/src/run/wiring.rs`, `preflight_agents` passes a
  locator to `check_agents_present` that uses
  `ralphy_agent_cursor::locate_cursor()` for Cursor.
- **Today:** `check_agents_present_uses_the_given_locator` passes its own
  locator. `preflight_agents_probes_cursor_with_its_adapter_locator` pins the
  Cursor arm as source text.
- **Seam:** `locate_cursor` and `locate_program` read the real host. Make the
  locator's search roots injectable (for example a `PATH` value and a home
  directory), so a test can place a fake `cursor-agent` and no `cursor`.
- **Test that becomes possible:** call `preflight_agents(Cursor, Cursor)`
  with roots that hold only `cursor-agent`, and assert it succeeds. Then the
  source pin goes. Seen red: the Cursor arm uses `locate_program("cursor")`.

## `schedule install run --with-triage` registers the triage prelude (P0-14)

- **Code:** `crates/ralphy-cli/src/schedule.rs`, `install` sets
  `spec.pre_invocation = Some(spec::triage_prelude())` and then calls
  `host_install(&spec)`.
- **Today:** `timer_spec_run_with_triage_chains_triage_first` was deleted. It
  set `pre_invocation` itself and then asserted it.
  `triage_prelude_is_triage_yes_without_if_idle` checks the prelude value only.
- **Seam:** `install` calls the host directly. Split it into a function that
  builds the final `TimerSpec` from the arguments, and a thin caller that
  passes the spec to `host_install`. Or inject the host call.
- **Test that becomes possible:** build the spec for `install run
  --with-triage` and assert `pre_invocation` is `triage --yes`; build it for
  `install triage --with-triage` and assert the error. Seen red: delete the
  `spec.pre_invocation = ..` line in `install`.

## The init git stage commits, then branches, before any write (P0-15)

- **Code:** `crates/ralphy-cli/src/init/run.rs`, stage 4 (git safety) and
  stage 4b (branch). They ask the operator on stdin through `ask_yes_no`.
- **Today:** `init_git_safety_branch_and_scaffold_end_to_end` was deleted. It
  did the orchestration itself (decision, `commit_all_snapshot`,
  `checkout_new_branch`). `commit_decision` and `branch_decision` keep their
  tests in `init/run/decisions/tests.rs`, and
  `write_scaffold_writes_the_agent_docs_and_no_instruction_file` keeps the
  scaffold asserts.
- **Seam:** move stages 4 and 4b into a function that takes the answers (or
  an `ask` closure) and the repo path, and that the init run calls.
- **Test that becomes possible:** a dirty temp repo on `main`, answers "yes"
  and "": assert one new snapshot commit, a clean tree, and HEAD on
  `ralphy/init`. A second case answers "no" to the commit and asserts that no
  branch was made. Seen red: skip `commit_all_snapshot`, or name the branch
  wrong.

## Workbench console: confirm-then-act and the clipboard gate (P0-8, P0-9)

- **Code:** `crates/ralphy-daemon/assets/ui/wb-console.js`. The destructive
  console actions run `askConfirm` and act only on `ok` (P0-8). The OSC 52
  clipboard write is refused while the console replays its saved text; the
  write callback clears `replaying` (P0-9).
- **Today:** Rust pins in `crates/ralphy-daemon/src/tests.rs`
  (`the_destructive_console_clicks_confirm_first`,
  `the_console_clipboard_is_write_only_and_refused_on_replay`) read the
  statements as source text. No behavior test exists (gap G-12).
- **Seam:** the actions and the OSC handler are closures inside the console
  component. Export the confirm-then-act handlers and the OSC 52 handler (or a
  factory that takes `askConfirm`, the fence actions and the terminal), so
  `node --test` can load them with stubs for the browser edges only.
- **Tests that become possible:** in `crates/ralphy-daemon/ui-tests`, answer
  the confirm with `false` and assert the action did not run, then with `true`
  and assert it ran. For the clipboard, send an OSC 52 sequence during replay
  and assert no clipboard write, then after replay and assert one write. Then
  the pins go. Seen red: `if (ok) arrangeFence(f.id)` to
  `arrangeFence(f.id)`, and `term.write(a.subarray(9)); replaying = false;`.

## Also out of scope in #485
