# Plan for #26: OpenCode adapter — auth-error stop + provider-key scrubbing

## Feasible: yes
The work mirrors the Codex adapter's existing auth-error prior art
(`is_codex_auth_error` + the actionable `bail!` in `plan`/`execute`) onto the
OpenCode adapter; the provider-key scrubbing it also asks for is already in place
(`build_opencode_command` `env_remove`s both keys, with a passing test). All
behavior is unit-test verifiable except confirming the exact `ProviderAuthError`
string against a true live signed-out run, which is review-only.

## Execution model: sonnet
Localized, single-file change with a direct, well-understood template in the
Codex adapter (detector fn + precedence-ordered `bail!`); mechanical, no tricky
design judgment.

## Done when
- `cargo test -p ralphy-agent-opencode` passes, including a new test
  `is_opencode_auth_error_matches_captured_provider_auth_error` that feeds a
  representative captured `ProviderAuthError` error event (and stderr text)
  through the detector and asserts it matches, plus a test asserting the detector
  ignores unrelated and `RALPHY_DONE_EXIT` text.
- A new test asserts the detector still fires when the same text also carries a
  `RALPHY_DONE_EXIT` sentinel (the auth signal wins over a clean-looking finish),
  proving precedence at the detector level; the `bail!` is placed before
  `classify_opencode_outcome` in `execute` and before the generic "no plan"
  `bail!` in `plan`, so auth takes precedence over generic/limit/timeout
  classification.
- The existing `build_command_removes_both_api_keys` test continues to pass
  (both `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` scrubbed on the child).
- Review-only: the exact `ProviderAuthError` / logged-out string used by the
  detector matches what a true live signed-out `opencode run` emits (the ADR-0005
  D6 "deferred until live" string); a human confirms this against a real
  signed-out run in the PR.

## Acceptance ledger
- [verified] A signed-out `opencode run` stops the run with the actionable "run `opencode auth login`" message (both during `plan` and `execute`), not a generic "no plan" / `Stuck`. — evidence: `is_opencode_auth_error` gates an actionable `bail!` placed before the generic "no plan" bail in `plan` and before `classify_opencode_outcome` in `execute`; a unit test asserts the detector matches the captured signed-out text
- [verified] `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` are removed on the child; a unit test asserts both are scrubbed. — evidence: already implemented in `build_opencode_command` (`env_remove` both keys); existing test `build_command_removes_both_api_keys` asserts both are scrubbed and stays green
- [review-only] The deferred exact auth-error string(s) are firmed up from an observed signed-out run, with a unit test over the real captured text. — evidence: the detector keys on OpenCode's documented `ProviderAuthError` SDK error name (ADR-0005 D6) with a unit test over representative captured text; a human confirms the string against a true live signed-out run in the PR
- [verified] The auth-error check takes precedence over usage-limit and generic classification. — evidence: the auth `bail!` runs before `classify_opencode_outcome` (which returns `Timeout`/`Stuck`); OpenCode has no usage-limit classifier yet (D9 deferred), so precedence is structural; a unit test asserts the detector fires even when the text also contains `RALPHY_DONE_EXIT`

## Decisions
- Decision: the detector keys on the case-insensitive substring `providerautherror` (OpenCode's documented SDK error name, named in ADR-0005 D6). Why: it is the firm signal from the source-level study, covers the issue-#15562 Claude-OAuth-reset masquerade (same error name), and avoids false positives from our own prompt text mentioning `opencode auth login`.
- Decision: the auth detector reads the combined stdout+stderr log, not just the JSON stdout stream. Why: the issue requires detection "over the JSON stream / stderr" and a logged-out error often prints only to stderr; `run_opencode` already writes the combined log to `opencode.log`, so it returns it for in-memory detection too.

## Steps
- [x] In `crates/ralphy-agent-opencode/src/lib.rs`, add `OPENCODE_AUTH_ERROR_MSG` const ("OpenCode is not authenticated (ProviderAuthError) — run `opencode auth login` and retry") and `fn is_opencode_auth_error(text: &str) -> bool` keyed on the lowercased substring `providerautherror`, mirroring `is_codex_auth_error`.
- [x] In `crates/ralphy-agent-opencode/src/lib.rs`, change `OpenCodeAgent::run_opencode` to also return the combined stdout+stderr log (e.g. return `(exited_cleanly, timed_out, stdout_text, log)`), so the auth detector can see stderr; update both call sites in `plan` and `execute`.
- [x] In `OpenCodeAgent::execute`, after `run_opencode`, check `is_opencode_auth_error(&log)` and `bail!("{OPENCODE_AUTH_ERROR_MSG} (see {})", self.run_dir.join("opencode.log").display())` BEFORE calling `classify_opencode_outcome` — so auth takes precedence over timeout/generic classification.
- [x] In `OpenCodeAgent::plan`, inside the `if !plan_path.exists()` branch, check `is_opencode_auth_error(&log)` and `bail!` with the actionable message BEFORE the generic "opencode produced no plan" bail.
- [x] In the `tests` module of `crates/ralphy-agent-opencode/src/lib.rs`, add `is_opencode_auth_error_matches_captured_provider_auth_error` over a representative captured `ProviderAuthError` JSON error event plus stderr text, and a test that it ignores unrelated / `RALPHY_DONE_EXIT`-only text — these reference the not-yet-existing fn so they FAIL to compile before the change and PASS after.
- [x] Add a test `is_opencode_auth_error_takes_precedence_over_done_sentinel` asserting the detector returns `true` for text that also contains `RALPHY_DONE_EXIT`, proving the auth signal wins over a clean-looking finish (precedence at the detector level).
- [ ] Self-review: spawn the `reviewer` skill as an independent subagent over ONLY this issue's commits (not the whole branch). Resolve every HIGH finding before finishing; if one cannot be fixed autonomously, record it under `## Notes & decisions` and block instead of declaring done.
- [ ] Run `cargo fmt --all` and `cargo test -p ralphy-agent-opencode` (and `cargo clippy` if used by the project); all pass with no new warnings.
