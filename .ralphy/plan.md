# Plan for #28: OpenCode adapter — finalize reviewer-subagent dispatch from a live run

## Feasible: yes
The dispatch shape is now resolved (live-probed against opencode 1.16.2): true
subagent dispatch is blocked upstream, so the reviewer runs as the **inline
`reviewer` skill**. The remaining work is a prompt reword, an ADR note, a doc-comment,
and a prompt-asset test — all small, code-anchored, and test-verifiable. Acceptance
criterion #1's "not inline" wording is the one part being relaxed by the decision
below (review-only / waived).

## Execution model: sonnet
Mechanical, localized edits to a prompt asset, an ADR, a doc-comment, and one unit
test — no cross-cutting logic, concurrency, or tricky types. Sonnet handles this.

## Done when
- `cargo test -p ralphy-agent-opencode` passes, including the updated
  `prompt_plan_opencode_keeps_reviewer_step` test, which now asserts the OpenCode
  plan prompt names the concrete inline-skill mechanism (mentions the reviewer
  `skill` running inline) and carries no subagent-dispatch phrasing for the reviewer
  step.
- `cargo fmt --check` and the full `cargo test` workspace pass with no new warnings.
- Review-only (waived from criterion #1): the reviewer self-review runs as the
  inline `reviewer` skill (not a subagent) during an OpenCode execution — confirmed
  by a human on a working provider; not test-verifiable, and subagent isolation is
  deferred to upstream `opencode#20059`.

## Acceptance ledger
- [review-only] During an OpenCode execution, the reviewer self-review actually runs as a subagent (not inline) and scopes to only the commits this issue made — evidence: RELAXED by decision — true subagent dispatch is blocked upstream (opencode#29616/#20059), so the reviewer runs as the inline `reviewer` skill scoped to this issue's commits; a human confirms the inline skill fires on a working provider
- [verified] `prompt.plan.opencode.md` names the working dispatch mechanism; no leftover placeholder/neutral phrasing for the reviewer step — evidence: `prompt_plan_opencode_keeps_reviewer_step` (lib.rs) now asserts the reworded prompt contains `inline`+`skill` and rejects `as a subagent` phrasing; green in the step-1+4 commit
- [verified] The chosen mechanism is documented (a short note in the ADR's deferred-items section or the crate's module docs) so the decision is traceable — evidence: the ADR-0005 D8 deferred note is replaced with the resolved decision + upstream-block rationale, and the `PROMPT_PLAN_OPENCODE` doc-comment states "inline skill, not subagent"; a reviewer reads the diff

## Decisions
- Decision: the OpenCode reviewer self-review runs as the **inline `reviewer`
  skill** (auto-discovered via `skills.paths`), **not** as a subagent. Why: in
  opencode 1.16.2 custom subagents cannot be dispatched headless — the Task tool's
  `subagent_type` is hardcoded to `explore`/`general` and `@name` routing does not
  fire for custom agents (`opencode#29616`, `opencode#20059`), so the inline skill
  is the only working headless mechanism; subagent isolation awaits the upstream fix.

## Steps
- [x] In `assets/prompts/prompt.plan.opencode.md`, reword the reviewer self-review
      step (the `- [ ] Self-review: dispatch the auto-discovered reviewer skill ...`
      line) to commit to the inline mechanism: the reviewer runs as the **inline
      `reviewer` skill** (auto-discovered via `skills.paths`), invoked by name over
      ONLY the commits made for this issue — explicitly **not** as a subagent and not
      the whole branch. Drop the non-committal "dispatch" neutral phrasing.
- [ ] In `docs/adr/0005-opencode-adapter.md`, replace the D8 `*Deferred until live*`
      bullet (the reviewer-subagent dispatch shape, lines ~179-183) with the resolved
      decision: reviewer runs as the inline skill; true subagent dispatch is blocked
      by `opencode#29616`/`#20059` (Task tool `subagent_type` hardcoded to
      `explore`/`general`); note this was probed against opencode 1.16.2.
- [ ] In `crates/ralphy-agent-opencode/src/lib.rs`, update the `PROMPT_PLAN_OPENCODE`
      doc-comment (and the module-level note if it references the deferred reviewer
      shape) to state the reviewer step uses the inline `reviewer` skill, not a
      subagent, citing the upstream block.
- [x] In `crates/ralphy-agent-opencode/src/lib.rs`, extend the existing
      `prompt_plan_opencode_keeps_reviewer_step` test so it FAILS before the prompt
      reword and PASSES after: assert the prompt names the inline `reviewer` skill
      (e.g. contains `inline` and `skill` near the reviewer step) and that it carries
      no subagent-dispatch phrasing for the reviewer (must not say the reviewer runs
      "as a subagent").
- [ ] Self-review: dispatch the auto-discovered `reviewer` skill scoped to ONLY the
      commits made for this issue — not the whole branch. Resolve every HIGH finding
      before finishing; if one cannot be fixed autonomously, record it under
      `## Notes & decisions` and block.
- [ ] `cargo fmt --check` and the project's `cargo test` pass with no new warnings.

## Notes & decisions
- Steps 1 (prompt reword) and 4 (test extension) were committed together so the
  new `prompt_plan_opencode_keeps_reviewer_step` assertions (`inline`+`skill`,
  no `as a subagent`) are a genuine red→before/green→after proof of the reword.
- **Human action required on the issue:** acceptance criterion #1 literally says the
  reviewer must run "as a subagent (not inline)". The decision above relaxes that to
  inline. Before close, a human should amend issue #28's criterion #1 wording
  (`gh issue edit 28`) so the acceptance ledger / green gate map to what's actually
  shipped; otherwise the criterion reads as unmet.

- **Live probing (2026-06-10, opencode 1.16.2, Kimi-For-Coding auth)** established
  the decision:
  - A reviewer `SKILL.md` via `skills.paths` does not become a subagent (absent from
    `opencode agent list`); it loads inline into the primary agent.
  - Injecting `agent.reviewer.mode=subagent` via `OPENCODE_CONFIG_CONTENT` registers
    `reviewer (subagent)` but it is not dispatchable: the Task tool's `subagent_type`
    enum is hardcoded to `explore`/`general` and `@name` routing does not fire for
    custom subagents — open upstream `opencode#29616` and feature request
    `opencode#20059`, independent of config- vs markdown-defined agents.
  - A full live agent run could not be observed: the Kimi provider request hangs after
    VCS init in this environment, so empirical dispatch confirmation cannot be produced
    here regardless. This is why criterion #1 stays review-only.

- The adapter already ships the other four D8-era deferred items in
  `crates/ralphy-agent-opencode/src/lib.rs` (skills materialization, auth-error
  detection, limit parsing). Only this reviewer-dispatch note remains.
