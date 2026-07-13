# Issues query surface: `ralphy issues` and the queue snapshot

Status: proposed (design interview 2026-07-03; not yet implemented).
Depends on ADR-0019 for the `--push` arm and the enriched `queue.built`.

The original ask was "a GraphQL-like interface to query GitHub issues without
opening GitHub, minimal, covering ~80% of cases". Two facts shape the answer.
First, GitHub already ships a real GraphQL API, reachable through the `gh`
CLI Ralphy is built on — re-exposing it through a local server would be a
proxy nobody asked to maintain. Second, the one thing no other tool can
answer is **Ralphy's judgment** of the backlog: which issue is eligible,
which is parked by a human-return label (ADR-0016), which is gated by an open
blocker (ADR-0002), where `stop-before` will halt the run — logic that lives
in the queue builder and nowhere else. The 80% case is not "browse GitHub in
the terminal"; it is "see the backlog the way the runner will".

There is also a remote consumer: the ADR-0019 platform collects issue data
**through Ralphy's event channel** and holds no GitHub token. Events only
flow during a run, and a remote platform cannot invoke a CLI on a developer's
machine — so the backlog view must also travel *as an event*, both with every
run and on demand.

## Rejected alternatives

- **A local GraphQL server (`ralphy serve`).** Literally the ask, but it adds
  an HTTP server, a schema and heavy dependencies to a CLI that has none —
  the overengineering the ask itself vetoed — for a capability `gh api
  graphql` already provides.
- **The platform querying GitHub directly.** Always fresh, but it duplicates
  the queue/precedence logic outside Ralphy (guaranteed drift) and forces
  GitHub token management onto the platform — the requirement was explicitly
  the opposite: Ralphy is the bus.
- **Write operations (labels, comments) in the subcommand.** `gh issue edit`
  already does this well, and `ralphy triage` (ADR-0017) owns the opinionated
  label transitions. A read surface stays honest by staying read-only.
- **PRs and milestones.** Ralphy never opens PRs by design (it hands back a
  branch); PRs are human context outside the 80%.

## Decision

### 1. `ralphy issues`: the queue as the runner sees it

A read-only subcommand listing open issues with Ralphy's queue judgment,
reusing [github.rs](../../crates/ralphy-core/src/github.rs) and the same
resolution the runner uses (`resolve_queue_labels`, human-return precedence,
blocked-by gating) so the CLI and the run can never disagree:

    $ ralphy issues
    #91  feat: sink http     [queue]      eligible   pos 1
    #89  triage precedence   [queue]      skipped    ready-for-human
    #87  fix: ledger path    [queue,P1]   blocked    by #85

Per-issue: `number`, `title`, `labels[]`, `queue_status`
(`eligible | skipped | blocked | stop_before`), `skip_reason?`,
`blocked_by[]`, `position?`. `--format json` emits exactly this shape;
`--fields` selects a subset — the "GraphQL-like" ask satisfied as field
selection over a flat, stable shape, not a query language.

### 2. `ralphy issues show <n>`: enough detail to decide without a browser

Body, comments — including the ADR-0017 consolidated-spec comment, surfaced
as first-class (`consolidated_spec`) since it is the authoritative spec when
present — labels, queue judgment, plus the issue's Ralphy history from the
usage ledger (ADR-0008): prior runs, outcomes, token totals. Also
`--format json`.

### 3. `ralphy issues --push`: the backlog travels as an event

Emits the list-shape snapshot as a `dev.ralphy.queue.snapshot` CloudEvent to
the configured `events.url` (ADR-0019 sink, same identity extensions, same
delivery semantics). Symmetrically, the runner's `queue.built` event is
**enriched** to carry the same `issues[]` payload, so every run refreshes the
platform's backlog view for free. One payload shape, two triggers — defined
once in [docs/events.md](../events.md).

`--push` is also the seam the deferred daemon mode slots into: a resident
"active listening" Ralphy is just a periodic invoker of this same emission.
Deferred deliberately — Ralphy stays "the run, not the cron" (ADR-0017 §5)
until the platform proves the need.

## Consequences

- The queue-judgment logic gets its second consumer, which pressures it into
  a cleanly callable shape (queue view as data, not a side effect of running)
  — a prerequisite the implementation must deliver.
- `issues show` adds per-issue comment and ledger reads: acceptable at
  human-invocation scale, and the existing transient-retry wrapper applies.
- The enriched `queue.built` grows the payload of an existing event —
  additive, per the evolution rules in [docs/events.md](../events.md).
- The platform never needs a GitHub token; its entire ingest surface is the
  ADR-0019 event stream.

## Amendment (2026-07-13): comments and the Kanban board fold (#188)

Two gaps ADR-0036 §Consequences named against this surface:

- **`ralphy issues show <n> --format json` now carries `comments[]`** — the
  full raw comment thread, in order, alongside the existing first-class
  `consolidated_spec`. No new `gh` call: the comments were already fetched for
  `consolidated_spec` extraction, just not surfaced on the wire.
- **`ralphy issues --board --format json`** emits a Kanban-shaped fold instead
  of the flat `issues[]` array:
  `{issues[] (each IssueView + assignees[], state_reason), labels[]
  ({name,color} repo label vocabulary)}`. `assignees`/`state_reason` come from
  a new batched core fn, `list_issue_meta` — one `gh issue list
  --json number,assignees,stateReason` spawn per queue label (never per
  issue), unioned/deduped by number, mirroring `list_queue`'s spawn shape —
  plus `list_repo_labels` (one `gh label list` call) for the color vocabulary.
  `--board` is list-only and JSON-only (`show`/`--push`/`--format text` bail
  with a clear message) and does not widen the default `IssueView[]` array,
  which stays the stable, already-consumed shape.
- Deliberately NOT added to the domain `Issue` or the shared `IssueView`: both
  feed `resolve_queue_view` → the ADR-0020 `queue.built`/`queue.snapshot`
  CloudEvent payload (`docs/events.md`), and growing that struct would
  silently mutate the event contract. `assignees`/`state_reason` are folded in
  at the CLI's JSON-rendering layer only.
- No live consumer yet: the daemon Query verb that reads `--board` output is
  later work (ADR-0036). The shape is pinned by CLI tests
  (`ralphy-cli/src/issues/tests.rs`:
  `render_board_json_folds_assignees_state_reason_and_label_colors`,
  `show_view_json_includes_comments`).
