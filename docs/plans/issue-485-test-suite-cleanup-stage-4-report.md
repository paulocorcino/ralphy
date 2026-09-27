# Stage 4 — P0 UI wiring pins — Post-stage report

**Backlog items:** P0-7, P0-8, P0-9
**Commit:** _filled by parent_
**Plan:** issue-485-test-suite-cleanup.md

## Files changed

- `crates/ralphy-daemon/ui-tests/app.test.mjs` — six new behavior tests through
  `loadShell()` (P0-7):
  - `opening a row asks to wake its peer, and closing it does not` (`toggle` → `wakePeerFor`, with a close control)
  - `opening a row remounts the run-completion subscription` (`toggle` → `destroyChangesSub`, then `mountChangesSub`)
  - `emitCreate sends the directory the create lands in` (spy on `WB.emit`; a file, a folder and `null`)
  - `the create action asks for the name through the shell's prompt` (the `workbench:action` listener captured through `opts.document.addEventListener`)
  - `wakePeerFor wakes the daemon of a sleeping peer's row only` (covers the `wakePeerFor` definition that the first test stubs)
  - `askPrompt settles with the trimmed name the prompt submits` (covers `askPrompt`, `promptSubmit` and `promptRespond`, which the create test stubs)
- `crates/ralphy-daemon/src/tests.rs`:
  - P0-7: `the_run_completion_nudge_is_wired_through_the_ui_assets`, `the_peer_wake_is_wired_through_the_ui_assets`, `the_explorer_can_create_at_every_target_including_the_repo_root` and `naming_a_new_entry_uses_the_design_system_prompt` lose their definition needles. They keep the markup, CSS and `index.html` needles, the `subscribeChanges,` export, `createHere(kind) {` (the header calls it; no node test does), `async wakePeer(` / the nudge URL / `reply.ready`. `shouldReload?.(` became the full call `window.WBChanges?.shouldReload?.(frame, this.openSlug)`.
  - P0-8: `the_destructive_console_clicks_confirm_first` replaces the title-only loop with a table (title, site count, next statement). For each match of a title, it checks the title opens an `askConfirm({` call, and that the first statement after that call's `});` is the expected action.
  - P0-9: `the_console_clipboard_is_write_only_and_refused_on_replay` drops `"replaying = false;"` and pins `term.write(a.subarray(9),replaying?()=>{replaying=false;}:undefined` against the source with all whitespace removed.
- `docs/audit-tests-2026-09-27.md` — §7 rows for P0-7, P0-8, P0-9.

## Evidence

- P0-7a: mutation app.js `toggle`, deleted `if (this.openSlug === ref) this.wakePeerFor(ref);` | before fix: PASS (`the_peer_wake_is_wired_through_the_ui_assets`) | after fix: FAIL (`opening a row asks to wake its peer…`) | reverted: PASS
- P0-7b: mutation app.js `toggle`, deleted `this.destroyChangesSub(); this.mountChangesSub();` | before fix: PASS (`the_run_completion_nudge_is_wired_through_the_ui_assets`) | after fix: FAIL (`opening a row remounts the run-completion subscription`) | reverted: PASS
- P0-7c: mutation app.js `emitCreate`, `path: this.createDir(node)` → `path: ""` | before fix: PASS (`the_explorer_can_create_at_every_target_including_the_repo_root`) | after fix: FAIL (`emitCreate sends the directory…`) | reverted: PASS
- P0-7d: mutation app.js `create` case, `const name = c ? await c.askPrompt({` → `const name = null ? …` (falls back to `window.prompt`) | before fix: PASS (`naming_a_new_entry_uses_the_design_system_prompt`) | after fix: FAIL (`the create action asks for the name…`) | reverted: PASS
- P0-7e: mutation app.js `wakePeerFor`, deleted `if (window.WBFleet.wakeable(group)) this.wakePeer(daemon);` | after fix: FAIL (`wakePeerFor wakes the daemon…`) | reverted: PASS
- P0-7f: mutation app.js `promptSubmit`, deleted `this.promptRespond(name);` | after fix: FAIL (`askPrompt settles with the trimmed name…`) | reverted: PASS
- P0-8a: mutation wb-console.js tile button, `if (ok) arrangeFence(f.id);` → `arrangeFence(f.id);` | before fix: PASS | after fix: FAIL | reverted: PASS
- P0-8b: mutation wb-console.js live console close button, deleted `if (!ok) return;` after the `Close this console?` dialog | before fix: PASS | after fix: FAIL (left `["const finish = () => {", "if (!ok) return;"]`) | reverted: PASS
- P0-9: mutation wb-console.js replay write callback → `term.write(a.subarray(9)); replaying = false;` | before fix: PASS | after fix: FAIL | reverted: PASS

"Before fix" ran the test from `HEAD`; "after fix" ran the new one; both with the mutation applied.

## Gate results

`PYTHONIOENCODING=utf-8 python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ui --ids P0-7,P0-8,P0-9`: 8/8 checks passed (§7 rows, fmt, clippy, nextest, doctests, changelog check, node ui-tests, ui-copy).

## Acceptance criteria audit

- [x] Each P0-7 call site is covered by a behavior test that fails when the call is removed (four cases from the plan, plus two that cover the definitions the first four stub).
- [x] P0-8 pins each confirm site's action on the answer, site by site, for all six sites.
- [x] P0-9 pin fails for the clobber mutation and is not satisfied by the declaration.
- [x] Definition needles removed; markup, CSS and `index.html` needles kept.
- [x] §7 rows added; gate passes with `--ui`.
- [x] No asset file is in this commit.

## Deviations from plan

- Two extra node tests (`wakePeerFor wakes…`, `askPrompt settles…`). Without them, removing the definition needles would leave `wakePeerFor`'s body and the `askPrompt`/`promptSubmit`/`promptRespond` definitions with no test at all, because the call-site tests stub them.
- Needles that are not definitions and that no node test reaches were kept: `async wakePeer(`, `createHere(kind) {`, the `subscribeChanges,` export, and the `shouldReload` call (made exact).
- The gate script crashed printing nextest output under cp1252 on this host; it ran clean with `PYTHONIOENCODING=utf-8`. The script was not changed.

## Surprises / notes

- The maintainer edited `crates/ralphy-daemon/assets/ui/wb-console.js`, `crates/ralphy-daemon/ui-tests/wb-console.test.mjs` and added `changelog.d/touch-font-default.md` (touch font default) during this stage. Their editor wrote `wb-console.js` while the P0-9 mutation was applied, so their working copy briefly carried the mutation. It was removed with an exact inverse edit, and their font change is left untouched and unstaged. The first revert method (`git checkout -- <asset>`) is unsafe with a concurrent editor; later stages should revert with an exact inverse edit only.
- `app.js` and `wb-console.js` are CRLF in the working tree on Windows; a mutation script must match `\r\n`.
- G-12 (a behavior test for confirm-then-act) stays open: the P0-8 fix is still a source pin.
