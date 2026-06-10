# Plan for #27: OpenCode adapter — usage-limit handling (timeout backstop + best-effort Limit)

## Feasible: yes
The change is localized and mirrors the existing Codex limit handling: force
`stop_on_limit` for OpenCode in `main.rs`, and add a best-effort upgrade of
`Timeout`/`Stuck` to `Outcome::Limit` in the OpenCode adapter when the JSON
stream emits a 429/`APIError` (or the documented rate-limit strings), with the
per-issue wall timeout remaining the primary backstop. Every step names real
code and is unit-test verifiable.

## Execution model: sonnet
Mechanical, well-understood change that closely follows the Codex pattern
(`is_codex_limit_text` / `parse_codex_reset_hint` / `classify_codex_outcome`)
and the one-line `effective_stop_on_limit` extension — no cross-cutting design,
concurrency, or subtle correctness. Sonnet handles this reliably.

## Done when
- `cargo test` passes across the workspace, including a new test asserting
  `effective_stop_on_limit(false, CliAgent::OpenCode)` and
  `effective_stop_on_limit(true, CliAgent::OpenCode)` both return `true`.
- `cargo test` includes new tests over `parse_opencode_limit` proving: an
  `error` event with `name:"APIError"` + `statusCode:429` is detected (with a
  reset hint when one is present, `None` when absent); a documented literal
  rate-limit string is detected; a Zen `*UsageLimitError` is detected; and a
  non-limit error (e.g. `statusCode:500`) or no error yields no limit.
- `cargo test` includes new `classify_opencode_outcome` cases proving a hung
  run with a limit event upgrades `Timeout` → `Outcome::Limit(reset)`, a hung
  run with no limit event stays `Outcome::Timeout`, and a would-be `Stuck`
  with a limit event becomes `Outcome::Limit`.
- `cargo fmt --check` and `cargo clippy` (or the project's format/lint commands)
  pass with no new warnings.

## Acceptance ledger
- [verified] `effective_stop_on_limit` returns `true` for OpenCode regardless of the `--stop-on-limit` flag (unit test). — evidence: commit 64b7920; test `effective_stop_on_limit_opencode_forces_true` in `main.rs`
- [verified] An `error` event carrying `statusCode:429` / `APIError` classifies as `Outcome::Limit` (with a reset hint when present); absent any limit event, a hung run is reclaimed as `Timeout` by the wall budget. — evidence: commit e718459; `parse_limit_apierror_429_with_reset_hint`, `parse_limit_apierror_429_without_reset_hint`, `classify_timeout_upgrades_to_limit_when_seen`, `classify_timeout_stays_timeout_without_limit`
- [verified] The deferred exact limit-string set + any reset parser are firmed up from observed output, with unit tests over the captured text. — evidence: commit e718459; `parse_opencode_limit` keys on ADR-0005 D9 shapes; tests `parse_limit_retryable_literal_string`, `parse_limit_zen_usage_limit_error`, `parse_limit_non_limit_status_500`, `parse_limit_clean_stream_no_limit`
- [review-only] No auto-resume path is introduced for OpenCode. — evidence: human confirms in the PR that forcing `stop_on_limit` routes OpenCode `Limit` through `runner.rs`'s `_ => break outcome` arm (no wait/resume) and that no new resume code was added

## Decisions
- Decision: source the limit-string set from ADR-0005 D9's documented shapes
  rather than a live capture. Why: this autonomous pass cannot trigger a live
  429; the ADR's source-level study (`retryable()`, SDK `APIError`+429, Zen
  `*UsageLimitError`) is the authoritative reference and the tests use
  representative captured JSON — the same conservative branch the auth-error
  detector (D6) took with a representative log.
- Decision: extract the reset hint best-effort from a `retryAfter` field or a
  `Retry-After` / "try again" substring in the error message, returning `None`
  when absent. Why: D9 says extract a reset "only when one is present"; 429
  hints are not guaranteed and the wall timeout is the real backstop.
- Decision: model the limit signal as `Option<Option<String>>` (outer `Some`
  = a limit event was seen; inner = optional reset hint), threaded into
  `classify_opencode_outcome`. Why: it distinguishes "limit, no reset" from
  "no limit" without a new struct, and maps cleanly to `Outcome::Limit(reset)`.

## Steps
- [x] In `crates/ralphy-cli/src/main.rs`, extend `effective_stop_on_limit` to
      force `true` for OpenCode too: change `matches!(agent, CliAgent::Codex)`
      to `matches!(agent, CliAgent::Codex | CliAgent::OpenCode)` and update its
      doc comment to name OpenCode (D9: long limits carry no parseable reset).
- [x] In `main.rs` `#[cfg(test)] mod tests`, add `effective_stop_on_limit_opencode_forces_true`
      asserting both `effective_stop_on_limit(false, CliAgent::OpenCode)` and
      `(true, CliAgent::OpenCode)` are `true` (fails before the step-1 change,
      passes after).
- [x] In `crates/ralphy-agent-opencode/src/lib.rs`, add `fn parse_opencode_limit(stdout: &str) -> Option<Option<String>>`
      that scans the line-delimited JSON `error` events and returns `Some(reset)`
      when one matches a usage limit — `name:"APIError"` with `statusCode == 429`,
      or a documented `retryable()` literal rate-limit string, or a Zen
      `*UsageLimitError` name — extracting the reset hint via a small helper
      (e.g. `parse_opencode_reset_hint`) and `None` otherwise.
- [x] In `lib.rs`, change `classify_opencode_outcome` to take an added
      `limit: Option<Option<String>>` parameter: when `timed_out`, return
      `limit.map(Outcome::Limit).unwrap_or(Outcome::Timeout)`; at the final
      `Stuck` fallthrough, return `Outcome::Limit(reset)` when `limit` is
      `Some`. Update its doc comment to note the best-effort upgrade (D9).
- [x] In `lib.rs` `execute`, call `parse_opencode_limit(&stdout_text)` and pass
      the result into `classify_opencode_outcome(...)`; add it to the `info!`
      end-of-run log fields.
- [x] In `lib.rs` tests, update the existing `classify_*` call sites for the new
      parameter (pass `None`) and add: `classify_timeout_upgrades_to_limit_when_seen`
      (timed_out + `Some(Some(reset))` → `Outcome::Limit(Some(reset))`),
      `classify_timeout_stays_timeout_without_limit` (timed_out + `None` →
      `Outcome::Timeout`), and `classify_stuck_upgrades_to_limit_when_seen`.
- [x] In `lib.rs` tests, add `parse_opencode_limit` cases over representative
      captured JSON: an `APIError`+`statusCode:429` event with a reset hint →
      `Some(Some(...))`; the same without a hint → `Some(None)`; a documented
      literal rate-limit string → `Some(_)`; a Zen `*UsageLimitError` → `Some(_)`;
      a `statusCode:500` error and a clean stream → `None` (this test fails
      before step 3 and passes after — proving the behavior).
- [x] Self-review: spawn the `reviewer` skill as an independent subagent over
      ONLY the commits this run made for issue #27 (not earlier issues on the
      branch). Resolve every HIGH finding before finishing; if one cannot be
      fixed autonomously, record it under `## Notes & decisions` and block
      instead of declaring done.
- [x] Run the project's format and test commands (`cargo fmt`, `cargo clippy`,
      `cargo test`) and confirm they pass with no new warnings.
