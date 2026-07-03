# Agent triage entry path: `ralphy triage` and the consolidated-spec comment

Status: accepted. Not yet implemented — see the tracking issue for the code
stage.

Ralphy's coupling to issue *format* is already minimal: the body goes verbatim
into `.ralphy/issue.json` and the planning agent interprets free prose,
returning `Feasible: no` when the spec is not executable. The only structure
the **runner** parses from a body is `## Blocked by` / `## Parent`
([blocked.rs](../../crates/ralphy-core/src/blocked.rs)). So "accept issues in
any format" mostly works today — labelling `AFK` directly remains the fast
path and is unchanged by this ADR.

What is missing is a ramp for issues the operator trusts but does not want to
normalize by hand:

- a user-authored issue whose real spec emerged across a long comment thread,
- dependencies expressed outside the `## Blocked by` shape the runner parses,
- issues the operator wants an agent to evaluate before they enter the queue.

We rejected a per-run LLM conversion layer (issue → rewritten local JSON): it
duplicates work the planner already does with the codebase in view, inserts a
paraphrase between the authoritative spec and the executor (exactly what the
planning prompt's "never launder second-hand claims" rules fight), and creates
a local artifact that drifts from the living issue. We also rejected editing
the author's issue body — even additively — because the original post is the
material record of the request and the discussion is where the problem got
defined; rewriting either destroys provenance.

## Decision

### 1. `triage-agent`: a fixed operational label

A new fixed, non-configurable label `triage-agent`, added to
`ralphy_label_specs` and synced by `ralphy init`. Like `stop-before`, `AFK`
and `HITL` (ADR-0001), it lives **outside** the five canonical triage roles
and outside the setup-pocock mapping table.

Applying it is the operator's **trust act**: "I judge this issue good enough
for an agent to work, after normalization." That single human decision covers
the whole pipeline that follows — which is what makes unattended operation
(§5) sound. While present, the label also parks the issue out of the run
queue (a human-return label under ADR-0016), so triage and run never race.

### 2. `ralphy triage`: a judgment session on the ADR-0012 skeleton

A new CLI command reusing the `ralphy init` machinery (agent session driven by
a prompt on disk, emitting JSON against a Rust schema, previewed locally,
published via `gh`). For each open issue carrying `triage-agent`, the session
reads the body and the **full comment thread** and emits a `TriageDraft` with
one of three verdicts:

- **promote** — executable as-is: swap `triage-agent` for `ready-for-agent`.
  No comment, no rewriting. Expected to be the common case.
- **consolidate** — the executable spec needs assembling from body + thread:
  post the consolidated-spec comment (§3), then swap the labels as above.
- **bounce** — under-specified even with the thread: comment what is missing,
  swap `triage-agent` for `needs-info` (the canonical reporter-bounce).

Label swaps introduce `remove_label` on the tracker trait — the first label
removal in the codebase. It is hygiene (one-shot triage, a readable board),
not a safety mechanism: ADR-0016 precedence already guarantees a
`triage-agent` issue cannot be run.

### 3. The consolidated-spec comment

A single comment authored by Ralphy, carrying a stable machine marker
(`<!-- ralphy:consolidated-spec -->`) and, for humans, a fixed heading. It
contains:

- the problem statement in executable form,
- acceptance criteria,
- a `## Blocked by` section when dependencies exist,
- **provenance**: a link per consolidated clause to the thread comment or
  body passage it came from — the audit trail that replaces editing anything.

Idempotent by construction: re-triage finds its own marked comment and
**edits it** rather than stacking versions. The author's body and other
people's comments are never modified — a hard non-goal of this ADR.

### 4. Contract changes in the existing pipeline

- **Planner prompt** (`prompt.plan.md`): today "the `body` is the
  authoritative spec; `comments` are secondary". The new rule: when a marked
  consolidated-spec comment is present, **it** is the authoritative spec, and
  the body plus the rest of the thread become background context. Without this
  change the planner would rank the consolidation as secondary chatter —
  the opposite of its purpose.
- **Blocked-by gating** (`blocked.rs` + queue build): `## Blocked by` is
  parsed from the body **and** from the marked comment. This costs one extra
  `gh` comments fetch per queued issue at queue-build time (the queue is the
  labelled subset — tens, not hundreds — and the existing transient-retry
  wrapper applies). Chosen over "append `## Blocked by` to the body", which
  would breach the never-edit rule for one convenient case and erode it.

### 5. Unattended mode and scheduling

Interactive `ralphy triage` previews every outward action (consolidated
comment, label swaps) and asks before publishing — the ADR-0012 posture.
`ralphy triage --yes` (for schedulers) publishes and promotes directly: the
trust act already happened at labelling time (§1), so unattended promotion is
mechanical continuation of a human decision, not an agent expanding its own
authority. The bounce arm needs no confirmation in either mode — returning
work to a human is always safe.

The scheduled recipe becomes two phases in one window, triage first so
tonight's promotions join tonight's run:

    ralphy triage --repo <path> --yes ; ralphy run --repo <path> --deadline-hours 8

Two commands chained by the **external** scheduler, deliberately: Ralphy
remains "the run, not the cron", non-users pay nothing, and `;` (not `&&`)
means a broken triage costs the triage, never the night's execution of issues
already `ready-for-agent`.

## Consequences

- Free-form issues gain an audited, provenance-preserving ramp into the queue;
  direct `AFK` labelling stays untouched for issues the operator blesses
  personally.
- The planner gains a second authoritative-spec source; its "verify at source"
  discipline now includes checking for the marked comment.
- Queue building grows per-issue comment fetches; acceptable at queue scale,
  and a reason to keep the queue the labelled subset rather than all-open.
- `docs/triage-roles.md` and `docs/scheduling.md` must document the new label
  and the two-phase recipe (code stage).
- The setup-pocock skill is **unchanged**: `triage-agent` is a Ralphy
  operational label, not a triage role (ADR-0001 amendment).
