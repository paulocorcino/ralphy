# Knowledge consolidation dispatches on the selected agent

Status: accepted (implemented 2026-07-09).

The end-of-run knowledge-consolidation session — the pass that folds the loose
`.ralphy/knowledge/issue-<N>.md` notes into a single curated `KNOWLEDGE.md`
(ADR-0008) — was hardwired to Claude. Both entry points (`ralphy consolidate` and
the automatic trigger in `finalize_run`) funnelled into a single call to
`ralphy_agent_claude::consolidate_knowledge`, and the automatic path hardcoded
`--model opus --effort medium`. So a run driven entirely by `--agent kimi`
(or `codex`/`opencode`) still reached for the `claude` CLI at the very end. On a
box where Claude is absent or unauthenticated — the whole reason to pick another
adapter — the pass failed every time (best-effort, so the run survived and the
notes stayed loose, but the curated cache never advanced).

This was a deliberate scope-out when the Kimi adapter landed ("consolidation is
not agent-dispatched"), not a considered design: consolidation predates the
multi-adapter split and simply never got the per-adapter treatment the other
one-shot sessions already have.

## D1 — Consolidation joins the per-adapter one-shot family

Repo *diagnosis* (ADR-0012 stage 2), backlog → *issues* drafting (stage 8), and
agent *triage* (ADR-0017) are each a free `fn` on every adapter crate, dispatched
by a `match Agent { … }` in the cli. Consolidation now follows the same shape: a
`consolidate_knowledge` on all four adapter crates, dispatched by
`consolidate_with_agent` in the cli. The run's **executor** `--agent` drives it —
consolidation curates the knowledge the just-finished work produced, so it belongs
to the agent that did the work, not (in a split run) the `--plan-agent`. A bare
`ralphy consolidate` still defaults to Claude, so no existing invocation changes.

## D2 — The charter is vendor-neutral and moves to core

Unlike the planning prompt (which has per-adapter overlays, `prompt.plan.*.md`),
the consolidation charter is a single `prompt.consolidate.md` with no vendor
specialization — every adapter drives the identical text. It therefore moves from
a private `const` in the Claude crate to `ralphy_core::PROMPT_CONSOLIDATE`,
alongside the other agent-neutral charters (`PROMPT_DIAGNOSE`,
`PROMPT_INIT_ISSUES`, `PROMPT_TRIAGE`). One source of truth, no per-crate
`include_str!` copies. The session is a *text* one-shot (its only deliverable is
the rewritten `KNOWLEDGE.md`, verified by the caller against
`knowledge::validate_knowledge`), so each adapter reuses the shared
`run_text_session` plumbing exactly as Claude's did.

## D3 — Model/effort defaults are per-vendor, not a hardcoded opus/medium

The automatic path's `opus`/`medium` were Claude-specific and meaningless to the
others (Kimi and OpenCode have no reasoning-effort knob at all — ADR-0005 D3,
ADR-0028 D3). Defaults are now resolved by `consolidate_defaults(agent)`: Claude
keeps the deliberate opus/medium pairing (curation is judgment-heavy), and every
other adapter passes `None`, letting the adapter resolve its own default model and
ignore `effort`. The `ralphy consolidate` command's `--model`/`--effort` become
optional overrides over that vendor default rather than clap constants pinned to
Claude; an explicit flag still wins.

## D4 — The `ANTHROPIC_API_KEY` scrub stays, harmlessly

Both entry points clear `ANTHROPIC_API_KEY` before the session (the
subscription-quota sentinel Claude runs rely on). It is a no-op for the other
vendors and is left in place unconditionally rather than gated on the agent —
one less branch, and it keeps the Claude path byte-for-byte unchanged.
