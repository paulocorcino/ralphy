# Plan for #21: Document the edition-2024 migration for env::set_var

## Feasible: yes
The call site (`crates/ralphy-cli/src/main.rs:190`) already exists with a partial
comment; the task is to expand that comment to capture the three documented
points. Purely a comment change, no behavior touched.

## Execution model: sonnet
Single-file, localized comment edit at a known line with no logic change — the
most mechanical kind of task. Sonnet handles this trivially.

## Done when
- `cargo test --workspace` is clean (no behavior change; this is the issue's own
  acceptance criterion).
- `cargo fmt` leaves the file unchanged and no new warnings appear.
- Review-only: the comment at the `set_var` call states (a) why `""` and not
  `remove_var` (claude's empty-vs-absent handling is unverified; `""` is the
  intentional ps1-parity behavior), (b) the single-threaded safety precondition
  that makes the 2021-edition call sound, and (c) that edition-2024 migration
  must wrap the call in `unsafe`. Only a human can confirm the prose conveys
  these three points.

## Decisions
- Decision: do not add a test for this change. Why: it is documentation-only with
  no observable behavior to assert; the verifiable gate is that `cargo test`
  stays clean, per the issue's own acceptance criteria. (Adding a "failing"
  test would be artificial and is not what the issue asks for.)

## Steps
- [x] In `crates/ralphy-cli/src/main.rs`, replace the existing two-line comment
      above `std::env::set_var("ANTHROPIC_API_KEY", "")` (lines 188-189) with an
      expanded comment covering: (a) why `""` not `remove_var`, (b) the
      single-threaded safety precondition, (c) the edition-2024 `unsafe`-wrap
      requirement. Leave the `set_var` line itself unchanged.
- [ ] Self-review: spawn the `reviewer` skill as an independent subagent over
      ONLY this run's own commits for issue #21 (not the whole branch). Resolve
      every HIGH finding before finishing; if one cannot be fixed autonomously,
      record it under `## Notes & decisions` and block instead of declaring done.
- [ ] cargo fmt && cargo test --workspace pass with no new warnings.
