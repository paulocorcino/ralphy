# A blocked issue is skipped until its blocker is CLOSED; this is safe only because runs share one branch

Ralphy reads each issue's `## Blocked by` section. When an issue declares a blocker
that is still open, the runner **skips** it (no close, no stop — the same path as an
infeasible plan) so unrelated later issues still run, and a future run picks it up
once the blocker clears. A blocker counts as satisfied when it is simply **CLOSED**
on the issue tracker, not when its code is merged to the base branch.

"Closed is enough" is safe **only** because Ralphy works a single persistent branch
(`BranchMode::Current`, or one `afk/run-*` shared by every issue in a `New` run):
a closed blocker necessarily ran on that same branch, so its commits are already
present when the blocked issue runs on top of them. We accept that this couples the
satisfaction rule to the single-branch operating model.

## Considered Options

- **Merged to base** — only treat a blocker as satisfied once its code is on the
  base the dependent is cut from. Rejected: Ralphy closes on green *before* merge
  (ADR-0001), so this would stall every dependent chain until a human merges,
  defeating an unattended overnight run.
- **Closed AND commits reachable from the run-branch HEAD** — the mode-agnostic rule.
  Deferred, not rejected: it is the correct generalization, but it only earns its
  extra bookkeeping under `BranchMode::New` cross-run, which we do not currently use.

## Consequences

- A forward dependency (a blocker with a *higher* issue number than its dependent)
  costs one run of latency: ascending order reaches the dependent first, skips it
  while the blocker is open, runs the blocker, and the dependent is picked up next
  run. Correct, just not optimal — no topological reorder is done.
- If Ralphy ever adopts `BranchMode::New` for cross-run dependent work, this rule
  must be upgraded to the "closed AND reachable" variant, or dependents will run on
  a branch missing their blocker's code.
