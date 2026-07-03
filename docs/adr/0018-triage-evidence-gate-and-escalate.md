# Triage evidence gate and the `escalate` verdict

Status: accepted; not yet implemented (code stage: #91 evidence gate, #92
escalate verdict).

ADR-0017's triage charter judges **spec executability**: whether an issue has a
clear "done" a test or build can verify. It deliberately does not judge whether
the reported problem is *real* — an issue describing a nonexistent bug, written
with crisp acceptance criteria, is promoted, and the phantom is only caught
later when the execution phase cannot produce a failing test. Two gaps follow:

1. **Promotion rests on the reporter's claim.** The planning prompt's "verify
   at source, never launder second-hand claims" discipline starts only *after*
   the issue enters the queue; triage itself launders the claim into
   `ready-for-agent`.
2. **Accepted-but-judgment-heavy issues have no honest destination.** The
   bounce arm targets `needs-info` — semantically "the reporter owes
   information". An issue that is accepted but requires a *maintainer's*
   decision (a business-rule change, a flow redesign, anything ADR-shaped) is
   not the reporter's debt; parking it under `needs-info` misstates the board.

The design draws on the loop-engineering field notes in
[docs/research](../research/loop-engineering-vs-ralphy.md): the cost of a
mistake scales with the number of turns it survives, an agent grading work
tends toward approval unless calibrated to doubt, and the human review point
is a permanent feature of a trustworthy loop, not scaffolding to remove once
trust is earned.

## Decision

### 1. The evidence gate: promotion requires positive evidence

The charter's promotion bar changes from "executable spec" to "executable spec
**plus** confirmed problem". The calibration is asymmetric by design — a
false-human costs minutes of reading; a false-promote costs a bad PR or a
silent contract change — so the default stance is doubt: **an issue is not
agent-ready until the evidence gate proves it is.** Promote (and consolidate)
require all three:

1. **Confirmable at source** — the symptom reproduces, the log already shows
   it, or the defect is visible in the logic when read against the narrated
   scenario.
2. **Localizable** — the triage agent can point at file:line and explain the
   mechanism of the error.
3. **Contract-preserving** — the fix *restores* behavior already documented as
   intent (a test, a doc, an ADR). A change where the intent itself changes is
   never agent-first, whatever its size.

Failing any criterion means the verdict is not promote: missing information
the reporter owns → bounce; anything else → escalate (§3). In a repo with
little documented intent, criterion 3 escalates almost everything — that is
the safe default working as designed, not the triage being timid.

### 2. `## Evidence`: checkable citations and a falsifiable claim

The consolidated-spec comment (and the promote arm's diagnostic, when one is
posted) gains an `## Evidence` section. Two rules keep it from being prose
that merely *sounds* verified:

- **Citations, not narrative.** Each evidence line is a pointer a human or the
  planner can audit in seconds: `file:line`, a log excerpt, a command and its
  output — the same provenance discipline ADR-0017 applies to thread clauses.
- **The acceptance criteria must name the red test.** The spec states a test
  that fails today and passes after the fix. This converts triage's
  existence-claim into something the execution phase checks
  *deterministically*: the executor's mandatory failing-test-first step either
  confirms the claim (test is red) or falsifies it (test will not go red — the
  problem does not exist), one turn after the claim was made. Triage asserts;
  the red test adjudicates.

The consolidated-spec comment is the highest-blast-radius artifact triage
produces — the planner treats it as the authoritative spec (ADR-0017 §4) and
executes it faithfully — which is why evidence and per-clause provenance are
mandatory there, never optional polish.

### 3. `escalate`: a fourth verdict for accepted-but-human-first issues

New verdict beside promote/consolidate/bounce: the issue is **accepted** but
requires human judgment before any agent works it — a business-rule or flow
change, an ADR-shaped decision, or a scope too large for one executable spec.

- Label swap: `triage-agent` → `ready-for-human` (resolved through the repo's
  triage mapping, like every triage role; `HITL` alias honored). ADR-0016
  precedence keeps the issue out of the queue.
- The comment is required and must **deliver work, not defer it**: the
  diagnostic (what was confirmed, with evidence), the exact question a human
  must decide, and a proposal — a suggested decomposition into agent-sized
  child issues, or the restricted follow-up issue to open. The human receives
  a prepared decision, not "this is complex, good luck".

The verdict taxonomy now separates the two debts bounce used to conflate:
`bounce` = the *reporter* owes information (`needs-info`); `escalate` = a
*maintainer* owes a decision (`ready-for-human`). Label state on the board
stays truthful (the ADR-0016 goal).

### 4. The redirect flow: propose issues, never author outcomes

When escalate proposes a restricted follow-up issue (the maintainer-scoped
reframing of a sensitive request), the mechanics are:

- The proposal lives in the escalate comment: title, body draft, and — when
  the follow-up supersedes the original — a `Closes #<original>` line in the
  drafted body, so the original closes **mechanically on merge of the real
  work**, by GitHub, auditable. The agent never closes anyone's issue.
- Issue creation is always human-confirmed. Interactive `ralphy triage`
  previews the draft and asks; `--yes` publishes the escalate **comment only**
  and leaves creation to the human. This checkpoint is permanent, not a
  first-version caution to relax later: the ADR-0017 trust act ("I judge
  *this* issue good enough for an agent") does not extend to authoring new
  work items, and a review point removed on good behavior is how comprehension
  rot starts.
- Suggested decompositions reuse the existing dependency machinery: children
  are proposed with the parent's consolidated spec carrying `## Blocked by`
  on them, so if the human accepts, blocked-by gating sequences children
  before the parent with zero new runner mechanics.

## Consequences

- A confidently-written issue about a nonexistent problem stops at triage
  (bounce, with "looked in A, B, C; the described behavior is not in the
  current code") instead of burning a plan-execute cycle — and when triage is
  wrong in the other direction, the red-test step catches it one turn later.
- Triage runs read the repository more per issue (it already reads enough to
  judge executability; confirming at source is incremental). Reproduction by
  *execution* stays out of triage — reading is the cheap early filter, the red
  test is the expensive deterministic one, each at the right point in the
  funnel.
- `needs-info` regains a single meaning. Dashboards and humans can trust the
  split: `needs-info` = waiting on reporter, `ready-for-human` = waiting on a
  maintainer decision.
- The charter grows a fourth verdict; `TriageVerdict`, `apply_triage`, and the
  prompt asset change (code stage). `docs/triage-roles.md` gains the escalate
  edge in the state machine.
- ADR-0017 §2 is amended: the three-verdict set becomes four, and the
  promotion bar is the evidence gate, not executability alone.
