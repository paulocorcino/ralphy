# The promote evidence stamp and verdict-comment language

Status: accepted; implemented.

ADR-0017 §2 made `promote` a silent label swap: "executable as-is: swap
`triage-agent` for `ready-for-agent`. No comment, no rewriting." That silence
was chosen deliberately — to avoid a paraphrase layer between the authoritative
spec and the executor, and to never touch the author's body. ADR-0018 then
added the evidence gate: a promote is only legitimate once the triage agent has
**confirmed the problem at source** (confirmable, localizable, contract-preserving).

Those two decisions are now in tension. The evidence gate forces the agent to
do a substantive assessment before promoting — but `promote` throws that
assessment away. Two consequences follow:

1. **The AFK judgment is unauditable.** For every issue an agent-triage pass
   promotes, the board records only a label flip. A human (or a later reader)
   cannot see *why* it was judged agent-ready, or on what evidence — the very
   evidence ADR-0018 requires the agent to hold. The `--yes` scheduler path
   makes this sharper: unattended promotions enter the queue with no recorded
   rationale at all.
2. **Only the weakest verdicts leave a trace.** `consolidate`, `bounce`, and
   `escalate` all write their reasoning to the thread; `promote`, the verdict
   that actually admits an issue to the queue, writes nothing. The most
   consequential decision is the least documented.

Separately, the triage charter never constrained the **language** of the
comments it writes. An issue authored in Portuguese can receive an
English `bounce`/`consolidate`/`escalate` comment, which reads as a foreign
intrusion on the reporter's own thread.

## Decision

### 1. `promote` posts an evidence stamp

`promote` stops being silent. It now posts a single, compact, marked comment —
the **evidence stamp** — carrying the citations that already satisfied the
ADR-0018 evidence gate, then swaps the labels exactly as before. Concretely:

- A new fixed marker `<!-- ralphy:promote-evidence -->` (`PROMOTE_EVIDENCE_MARKER`),
  a sibling of `CONSOLIDATED_SPEC_MARKER`. Idempotent by construction:
  re-triage finds the marker and **edits** its own comment rather than stacking
  a second one — the same `upsert_marked_comment` path consolidate uses.
- The stamp is the **evidence-gate reasoning, not a rewritten spec**: what
  reproduces the problem (`file:line`, a log excerpt, a command and its output),
  the mechanism, and the documented intent the fix restores. This is
  deliberately *not* the paraphrase layer ADR-0017 rejected — it adds the
  agent's own audit trail, it does not restate or replace the author's request.
- `promote` now **requires** a non-empty comment, on par with the other three
  verdicts. `TriageItem::invalid_reason` rejects a promote without one, so a
  draft that would flip a label with no recorded evidence never publishes.

The stamp is **not** authoritative spec. The planner's ADR-0017 §4 elevation
applies only to `CONSOLIDATED_SPEC_MARKER`; the promote-evidence comment stays
background context, and its (absent) `## Blocked by` is never parsed for gating.
This keeps the stamp cheap to write and impossible to confuse with a spec.

Directly-labelled `AFK`/`ready-for-agent` issues are **unaffected** — they never
pass through `ralphy triage`, so they carry no stamp. That is consistent with
ADR-0017: the operator blessed those personally; the stamp documents the
*agent's* judgment, which only exists on the agent-triage path.

### 2. Verdict comments match the issue's language

The charter gains one rule: every comment a verdict writes (`promote` stamp,
`consolidate` spec, `bounce` note, `escalate` diagnostic) is written in the same
language as the issue body and thread. The machine markers, headings, and JSON
keys stay in English (they are parsed); the prose that a human reads is in the
reporter's language.

## Consequences

- Every agent-promoted issue now carries an auditable, provenance-preserving
  record of why it was judged AFK-ready — the `--yes` path included. The board
  stops hiding its most consequential decision.
- `promote` grows an outward write. It is still gated on the operator's confirm
  (or `--yes`), exactly like consolidate; the bounce/escalate "always safe,
  never gated" rule is unchanged.
- `TriageVerdict::Promote`'s contract, `TriageItem::invalid_reason`,
  `apply_triage`, `TriageLabels`, the preview line, `build_triage_prompt`'s
  `## Inputs` block, and the `prompt.triage.md` charter all change (code stage).
  `docs/triage-roles.md` notes the stamp.
- ADR-0017 §2 is amended a second time (after ADR-0018): `promote` is no longer
  "no comment". Its non-goal — never editing the author's body or other
  people's comments — is **preserved**: the stamp is an additive comment Ralphy
  authors, nothing existing is rewritten.
- One comment is now written per promoted issue, including trivially-clear ones.
  This is the accepted cost of an auditable board; the marker keeps re-triage
  from multiplying it.
