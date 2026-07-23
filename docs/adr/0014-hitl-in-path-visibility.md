# A HITL blocker is a surfaced human-blocker that stalls only its own chain, not the whole run

`ready-for-human` (alias `HITL`) has been vocabulary only since ADR-0001: human-only,
never queried, carrying no runtime behaviour. Because the roles are mutually exclusive
(an issue is `ready-for-agent` *or* `ready-for-human`), a HITL is **never a queue
member** — it can only ever appear as a **blocker** in another issue's `## Blocked by`
section. This ADR gives that one position runtime *visibility* without making the HITL
itself queryable: the agent still never works a `ready-for-human` issue.

The gap is in the blocked-by gate (ADR-0045). The gate treats every open blocker
identically: a blocker that is "agent work, not done yet" (the queue will resolve it)
is indistinguishable from one that is "HITL, parked until a human acts" (the queue
will *never* resolve it). Both render as `⏭️ skipped (blocked)`, so an unattended run
gives no signal that a chain is stalled on a human rather than merely waiting its turn.

**Decision.** At the gate, when an open blocker carries `ready-for-human`, classify the
skip reason as *waiting on a human* instead of generic *blocked*, and surface it. The
run **continues** — this is a deliberate extension of ADR-0045's skip-and-continue, not
a new hard halt. Only the chain that depends on the HITL stalls (which the gate already
does today); independent chains run to completion. The division of labour is:

- `stop-before` — a deliberate, operator-placed **hard halt**: the run `break`s before
  the tagged issue.
- `HITL`-in-path — an **organically-encountered human-blocker**: it is detected,
  classified, and reported, but the run keeps going.

Two scoping choices, both deliberately minimal for v1:

- **Direct-only classification.** Only the issue whose *immediate* blocker carries
  `ready-for-human` is marked `🙋 waiting on human at #N`. Issues further down the
  chain are blocked by that (still-open) intermediate issue and show the generic
  blocked reason. Transitive attribution (the head of a chain reporting "ultimately
  parked on a human at #N") is deferred, not rejected — see Consequences.
- **Label on the blocker issue itself.** The classifier reads the labels of the open
  blocker `#N` directly (`IssueTracker::issue_labels` → `gh issue view N --json labels`)
  and matches them against the human-gate vocabulary (`ready-for-human`/`HITL`). A
  targeted per-blocker label fetch, not a full open-issue listing: blockers per issue
  are few, and one small `gh` call each is cheaper than pulling the whole open set.

Surfacing lands in three places, mirroring how `stop-before` is already presented:

1. **Pending bar** — a `🙋 HITL` marker on the affected issue, mirroring the `⛔
   stop-before` marker (`bar_label` in `crates/ralphy-cli/src/ui.rs`).
2. **Run summary** — a dedicated "waiting on human" bucket, separate from ordinary
   blocked/skipped issues.
3. **Telegram** — a "human action required" section in the notifier
   (`crates/ralphy-cli/src/telegram/notifier.rs`).

To carry the reason out of the gate, `IssueResult.blocked_by: Vec<u64>` (numbers only,
no "why") is enriched to distinguish human blockers from agent-work blockers — e.g.
`Vec<Blocker>` where `Blocker { number, human: bool }`, or a parallel `human_blockers`
field. This is the one type change that ripples into the presenter and the notifier.

## Considered Options

- **Stop-and-notify (abort the whole run, like `stop-before`)** — the first HITL in any
  path adds a new `break` / `StopReason::Hitl` and ends the run. Rejected as the default:
  it recreates exactly the waste diagnosed when `stop-before` sat on an issue that was
  last *and* already blocked — one parked gate aborts unrelated, runnable chains. It is
  only the right call when a HITL is typically *foundational* (almost the whole queue
  depends on it transitively); that is not the assumed norm for this project. If that
  assumption changes, this ADR is the thing to revisit.
- **Make `ready-for-human` queryable / a worked state** — rejected, as in ADR-0001: it
  contradicts the canonical mutual exclusivity of the roles. This ADR adds *visibility*
  of the label as a blocker, not *execution* of it.
- **Transitive classification** — walk the blocker closure so the head of a stalled
  chain can name the human gate at its root. Deferred, not rejected: it belongs next to
  `sort_queue_in_graph` in `crates/ralphy-core/src/blocked.rs`, not inline in the
  runner, and the direct-only signal already covers the common "one parked gate, why is
  this branch stuck" case. Promote it if the direct signal proves insufficient.

## Consequences

- A HITL deep in a chain is named only on the issue that *directly* declares it as a
  blocker. The head of the chain still shows `blocked by #<intermediate>`. This is the
  accepted v1 limitation; the transitive variant is the upgrade path.
- The gate gains a targeted label lookup per open blocker (`issue_labels` →
  `gh issue view N --json labels`). This is one extra small `gh` call per *open* blocker
  of a *blocked* issue — a narrow slice of the queue — so the cost stays bounded without
  caching. A label-fetch failure is non-fatal: it degrades to the generic "blocked"
  reason rather than aborting the run, since classification is a visibility concern, not
  a correctness gate (unlike `is_closed`, which stays authoritative and fatal).
- `IssueResult` changes shape (or gains a field), so the presenter and Telegram notifier
  must be updated together; older serialized results without the field deserialize via
  `#[serde(default)]`, as elsewhere in the domain types.
- An unattended run now ends with an explicit "waiting on human" account instead of
  silently folding HITL-stalled chains into the generic blocked bucket — the operator
  knows which human action unblocks which chain.
