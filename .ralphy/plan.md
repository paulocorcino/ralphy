# Plan for #18: Dedup GhIssue to Issue mapping with impl From

## Feasible: yes
`crates/ralphy-core/src/github.rs` duplicates the `GhIssue -> Issue` field
closure in `parse_issue` (lines 32-37) and `parse_issue_list` (lines 46-51).
Both can collapse onto a single `impl From<GhIssue> for Issue`, and the existing
tests (`parses_issue_with_labels`, `parse_issue_list_reads_array`, etc.) verify
behavior is unchanged.

## Execution model: sonnet
Localized, mechanical refactor in one file: extract one `From` impl and rewire
two call sites. No design ambiguity or cross-cutting concerns.

## Done when
- `cargo test -p ralphy-core` passes with all existing github tests green,
  including `parses_issue_with_labels`, `tolerates_missing_body_and_labels`, and
  `parse_issue_list_reads_array`.
- A new test asserts `Issue::from(GhIssue { .. })` maps every field (number,
  title, body, labels) — fails to compile before the impl exists, passes after.

## Steps
- [x] In `crates/ralphy-core/src/github.rs`, add `impl From<GhIssue> for Issue`
      below the `GhIssue` struct (after line 27) that moves `number`, `title`,
      `body`, and maps `labels` via `.into_iter().map(|l| l.name).collect()`.
- [x] Rewrite `parse_issue` (lines 30-38) to return `Ok(Issue::from(g))`,
      removing the inline field map.
- [x] Rewrite `parse_issue_list` (lines 41-53) to use
      `raw.into_iter().map(Issue::from).collect()`, removing the inline closure.
- [x] In the `#[cfg(test)] mod tests`, add `from_ghissue_maps_all_fields` that
      builds a `GhIssue` (with at least one `GhLabel`) and asserts
      `Issue::from(g)` carries number, title, body, and labels through. This
      test references `Issue::from` / the `From` impl, so it fails to build
      before the impl is added and passes after.
- [x] Self-review: spawn the `reviewer` skill as an independent subagent over
      ONLY this issue's commits. Resolve every HIGH finding; if one cannot be
      fixed autonomously, record it under `## Notes & decisions` and block
      instead of declaring done.
- [x] cargo fmt && cargo test pass with no new warnings.
