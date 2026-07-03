# Assignee-scoped queue: opt-in `--assignee` filter

Status: proposed (design interview 2026-07-03; not yet implemented).
Builds on ADR-0020 (queue as data, view/runner parity) and ADR-0019 (event
envelope the filter mark rides on).

Two developers running `ralphy run` from different clones of the same repo
build the same label-driven queue and work the same issues in duplicate —
there is no claim mechanism, and git itself has no notion of ownership.
The forge does: the GitHub **assignee**. The decision is to let an operator
scope the queue to an assignee, opt-in, with the default behaviour unchanged.

## Rejected alternatives

- **Blocking run lock.** The presence lock (`.ralphy/run.lock`, issue #72) is
  deliberately a signal, not a mutex, and is per-clone anyway — it cannot see
  a run on another machine.
- **Automatic claim (self-assign on pick).** The GitHub API offers no
  compare-and-swap on assignee, so a claim is racy; filtering on assignments
  humans already made is race-free and covers the scenario. Claiming can be a
  later, separate decision if unassigned-issue contention ever materialises.
- **A boolean `--mine`.** Same implementation cost as the general form but
  closes the door on a scheduler running as a bot account
  (`--assignee ralphy-bot`).
- **Post-fetch domain filter.** Would add `assignees` to the `Issue` struct
  and an identity-resolution call; passing `--assignee` through to
  `gh issue list` keeps the struct intact and lets `gh` resolve `@me`.

## Decision

### 1. Filter at fetch, in the one shared point

`list_queue` gains `assignee: Option<&str>`, appended verbatim as
`--assignee <value>` to `gh issue list`. Multiple-assignee semantics are
`gh`'s: the login being *among* the assignees qualifies.

### 2. Scope: the work surfaces only

The filter applies to `ralphy run` and `ralphy issues list/show` — through
the shared queue-view seam, so the ADR-0020 view/runner parity holds under
the filter. Triage and init/consolidate **never** filter: they are whole-repo
housekeeping, and scoping them would rot the colleague's backlog or corrupt
bundle-children counts.

### 3. Invocation and persistence

`--assignee <login>` (typically `@me`) on both surfaces; a persisted
per-operator default in `.ralphy/settings.json` under `queue.assignee`
(the file is per-clone, hence per-operator). Precedence:
`--assignee X` > `--no-assignee` (one-shot escape) > config > no filter.

### 4. Explicit selection bypasses the filter

`--issues` / `--only-issue` fetch without `--assignee`, consistent with the
stop-before convention that a verbatim operator selection outranks queue
rules. Without this, `--only-issue 7` on a colleague's issue would silently
do nothing — the worst failure mode of an explicit command.

### 5. The event stream marks the scope

A filtered queue is a *partial* view, and the ADR-0019 platform cannot know
that unless told: the enriched `queue.built` and the `queue.snapshot` payloads
gain an optional `assignee_filter` field (`null` = whole queue), with `@me`
**resolved to the concrete login** (`gh api user`, once per run) so snapshots
from different developers stay distinguishable.

## Consequences

- Blocked-by judgment is unaffected by construction: it consults the
  dependency's real state via the tracker (`is_closed`), not queue
  membership, so an issue blocked by a colleague's open issue stays
  `Blocked` under a filtered queue.
- An active filter with an empty result prints a notice naming the filter,
  so "no issues" is never mysterious.
- The filter narrows visibility but does not claim: an unassigned issue is
  invisible to every filtered operator until a human assigns it. For the
  motivating workflow (each dev assigns their issues before running) this is
  the desired behaviour, not a gap.
- `assignee_filter` is an additive payload change, per the evolution rules in
  [docs/events.md](../events.md).
