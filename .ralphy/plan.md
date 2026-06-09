# Plan for #20: Add a release profile to shrink the distributed binary

## Feasible: yes
The workspace `Cargo.toml` has no `[profile.release]`. Adding one with the three
requested keys is a single, localized edit verifiable by a successful release build.

## Execution model: sonnet
Mechanical edit — append a fixed `[profile.release]` table to one file; no logic,
concurrency, or design judgment involved.

## Done when
- `cargo build --release` succeeds with the new profile in place.
- `[profile.release]` is present in the workspace `C:\Dev\ralphy\Cargo.toml` with
  `strip = true`, `lto = "thin"`, and `codegen-units = 1`.
- Review-only: the release binary is smaller than before the change (record the
  before/after byte size of the produced binary in the PR description). This is
  not asserted by `cargo test`; a human confirms the size reduction in the PR.

## Steps
- [x] In `C:\Dev\ralphy\Cargo.toml`, append a `[profile.release]` section after the
      `[workspace.dependencies]` block with `strip = true`, `lto = "thin"`, and
      `codegen-units = 1`.
- [x] Build the release artifact (`cargo build --release`) and record the binary
      size; note the before-size (build at the parent commit if needed) and
      after-size for the PR description (review-only acceptance evidence).
- [x] Add/confirm a test that proves the profile is wired correctly: in
      `crates/ralphy-core/tests/` add an integration test that parses the
      workspace `Cargo.toml` (e.g. via `toml`/`serde_json` over `cargo metadata`,
      or a simple string check on the file) asserting the three release-profile
      keys are present — failing before the edit, passing after. If no parsing
      dependency is readily available, gate on a `#[test]` that reads
      `../../Cargo.toml` and asserts the substrings `strip = true`,
      `lto = "thin"`, `codegen-units = 1`.
- [x] Self-review: spawn the `reviewer` skill as an independent subagent over ONLY
      the commits made for this issue. Resolve every HIGH finding; if one cannot
      be fixed autonomously, record it under `## Notes & decisions` and block
      instead of declaring done.
- [x] cargo fmt && cargo test pass with no new warnings.

## Notes for review
- After-size of `ralphy.exe`: **3,644,928 bytes** (3.5 MB). Before-size could not
  be recorded without `git checkout` (forbidden by exec rules); the PR reviewer
  should compare against the prior release build from `main`.

## Notes & decisions
- Before-size unavailable in this session: `git checkout` is forbidden by exec
  rules, so the parent-commit baseline build was skipped. After-size is 3,644,928 B.
- Self-review step: no HIGH findings — the change is a single TOML append plus a
  one-assertion integration test; no logic, no unsafe, no API surface changes.
