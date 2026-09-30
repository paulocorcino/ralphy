# Retirement review

Reviewed on 2026-09-30. These 15 files are candidates for deletion, not active
instructions. No retained repository document or code consumer requires them.
Their content is preserved; relative links were repaired during the move and
the missing historical `guard.ps1` oracle is identified as unavailable.
An original path below is provenance, not a live link.

## Manifest

| Original path | Preserved copy | Reason | Condition before deletion |
|---|---|---|---|
| `docs/backlog/README.md` | [Rewrite backlog index](backlog/README.md) | Index for the initial Rust rewrite, not the current queue. | Review the entire backlog as one historical package. |
| `docs/backlog/0001-walking-skeleton-dry-run-plan.md` | [0001](backlog/0001-walking-skeleton-dry-run-plan.md) | Initial workspace and dry-run implementation brief. | Confirm no remaining requirement needs transfer to current work. |
| `docs/backlog/0002-ralphy-pty-foundation-conpty.md` | [0002](backlog/0002-ralphy-pty-foundation-conpty.md) | Initial PTY implementation brief. | Preserve any unresolved platform requirement. |
| `docs/backlog/0003-interactive-execute-completion-detection.md` | [0003 brief](backlog/0003-interactive-execute-completion-detection.md) | Initial interactive execution brief. | Review with its implementation plan. |
| `docs/backlog/0003-interactive-execute-impl-plan.md` | [0003 plan](backlog/0003-interactive-execute-impl-plan.md) | Historical execution plan. | Process-tree termination remains recorded in the retained [test follow-up](../plans/issue-485-followup-draft.md#out-of-scope-in-the-issue-itself); do not infer it was completed. |
| `docs/backlog/0004-pretooluse-guard-hook.md` | [0004](backlog/0004-pretooluse-guard-hook.md) | Initial guard-hook brief. | Check any unique acceptance criterion before deletion. |
| `docs/backlog/0005-queue-loop-close-on-green-stop-on-nongreen.md` | [0005](backlog/0005-queue-loop-close-on-green-stop-on-nongreen.md) | Initial queue-loop brief. | Check against the current queue contract. |
| `docs/backlog/0006-stop-before-usage-limit-reset-report.md` | [0006](backlog/0006-stop-before-usage-limit-reset-report.md) | Initial stop and usage-limit brief. | Check against the current run and adapter contracts. |
| `docs/backlog/0007-staged-plan-routing-triage-labels-resolution.md` | [0007](backlog/0007-staged-plan-routing-triage-labels-resolution.md) | Initial routing and label brief. | Check against current planning and triage rules. |
| `docs/backlog/0008-branch-modes-preconditions-run-wrapup.md` | [0008](backlog/0008-branch-modes-preconditions-run-wrapup.md) | Initial branch-lifecycle brief. | Check against the current checkout-per-run decision. |
| `docs/backlog/0009-headless-exec-fallback.md` | [0009](backlog/0009-headless-exec-fallback.md) | Initial fallback brief. | Check against the current adapter contract. |
| `docs/execplans.md` | [Generic ExecPlan template](execplans.md) | No repository consumer; requires an untracked `PLANS.md` and is separate from the staged-plan workflow. | Confirm there is no maintainer workflow relying on this template. |
| `docs/strategy/backlog-snapshot-raw.json` | [Backlog snapshot](strategy/backlog-snapshot-raw.json) | Unreferenced historical export, with issue updates through July 29. | Confirm the historical export is no longer needed; it is not the current issue tracker. |
| `docs/research/2603.23613v1_manifest.json` | [Extraction metadata](research/2603.23613v1_manifest.json) | Unreferenced extraction metadata with machine-local paths. | The [paper extraction](../research/2603.23613v1.md) remains available. |
| `docs/research/Loop-Engineering-IEEE_manifest.json` | [Extraction metadata](research/Loop-Engineering-IEEE_manifest.json) | Unreferenced extraction metadata with machine-local paths. | The [paper extraction](../research/Loop-Engineering-IEEE.md) remains available. |

## What remains outside this directory

Audits, acceptance walkthroughs, the editor spike, adapter investigations,
validation companions and raw log families retain their evidence role. They
are indexed under [evidence](../evidence/README.md) and
[research](../research/README.md). The test-cleanup execution package remains
with its follow-up in [plans](../plans/README.md).

The [consolidation proposal](../adapter-support-consolidation.md) and
[file-size triage](../borderline-500-triage.md) remain at their original paths:
ignored research exports for #107 and #110–119 still cite their evidence.
The [test audit](../audit-tests-2026-09-27.md) also keeps its path because a
local ledger script reads it. These dependencies prevent retirement even
though some of the recorded implementation work is complete.

Unchecked boxes in a historical brief do not prove work is unfinished. A
completed implementation does not prove every old concern is resolved either.
This manifest records retirement eligibility, not issue closure.

## Review scope and limits

The inventory covered 232 tracked files under `docs/`, tracked repository
consumers and relevant local research. There were also 417 ignored local files;
they were left in place. The move excludes screenshots, raw capture families,
active lint configuration, ADRs and documents with unresolved evidence needs.
External issue/PR backlinks were not queried. Review those before final deletion.

Research/essay material without incoming links remains outside retirement when
it contains unique analysis or proposals. Lack of a backlink alone is not a
deletion criterion. The same applies to unlinked acceptance evidence.
