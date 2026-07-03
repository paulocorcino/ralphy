# AUTOMEM, scored against Ralphy — and the human-gated retro idea

A reading of [2607.01224v1.md](2607.01224v1.md) (AUTOMEM: Automated Learning of
Memory as a Cognitive Skill, Stanford) mapped onto Ralphy's existing memory
subsystem (handoff → knowledge cache → `consolidate` → citations). Verdict up
front: **the paper's two optimization loops do not transfer to Ralphy's domain,
and on persistent memory Ralphy is already ahead of the paper's own scope** —
but one insight survives the domain change and is worth parking as a future
idea: the **human-gated retro** (below).

## What AUTOMEM is

Two automated outer loops, both driven by a meta-LLM that reads *complete*
episode trajectories (up to 10^5 steps):

1. **Structure loop** — the meta-LLM reviews full traces, diagnoses memory
   failures, and rewrites the agent scaffold (prompts, memory-file schema,
   action vocabulary). Every revision is gated on **measured improvement over
   the same fixed seeds**; revisions that don't beat the previous version are
   rejected (their Appendix A.2).
2. **Proficiency loop** — LoRA-finetunes a dedicated "memory specialist" model
   on curated examples of the agent's own good memory decisions.

Result: ~2–4× progression gains for Qwen-32B on Crafter/MiniHack/NetHack.
Solid work in its domain.

## Why neither loop applies to Ralphy

- **Proficiency loop: dead on arrival.** Ralphy drives closed-weight vendor
  CLIs (Claude/Codex/OpenCode) on subscription. There are no weights to train.
- **Structure loop: missing its safety condition.** The loop only works because
  procedural games give a repeatable eval — fixed seeds, a scalar progression
  metric, re-run the same episodes and compare. Real GitHub issues are
  one-shot: no seed, no re-run, no metric. Without the measured-improvement
  gate, "automated scaffold revision" degenerates into *letting an LLM rewrite
  your prompts and hoping* — and the paper itself shows ungated revisions are
  frequently regressions (hence their retry/restart mechanics). Importing the
  mechanism without the condition that makes it safe would be overengineering.

## Where Ralphy is already ahead

The paper's own Limitations section (§6) lists as future work: *"the file
system starts fresh at the beginning of each episode; a natural extension is a
persistent memory that carries knowledge across episodes."* That extension is
exactly what Ralphy ships. The concrete failure modes their structure loop
discovered are the ones Ralphy's design already addresses:

| AUTOMEM finding (games) | Ralphy today |
|---|---|
| Unbounded append-only memory buries signal (v0 `dungeon_map.txt`) | `consolidate` with dedup, 200-line cap, provenance (`knowledge.rs`) |
| Consult-before-write had to be *trained into* the model | `prompt.execute.md` mandates reading `KNOWLEDGE.md` first |
| Scaffold-maintained auto-synced files (`inventory`, `status`) beat model-maintained ones | Runner materializes `issue.json`, `references.md`, `handoffs.md` deterministically |
| Empty-search rate as a memory-utility signal | Citations hit-rate with prune-on-non-use — a *better* signal for this domain, because it measures actual reliance, not failed lookups |

## The idea worth keeping: `ralphy retro` (human-gated trajectory review)

The one AUTOMEM insight that survives the domain change:

> **The executor only reports what it noticed. An independent review of the
> full trajectory catches memory failures that self-report cannot.**

Today every reflective surface in Ralphy is self-reported by the same agent
that did the work: `## Handoff`, `## Plan friction`, `**Knowledge used**`. If
the executor ignored a relevant `KNOWLEDGE.md` fact and burned twenty minutes
rediscovering it, nobody notices — the citation is simply absent, which is
indistinguishable from the fact being irrelevant.

**Shape of the idea.** A post-run command — `ralphy retro` — runs a one-shot
meta-session that reads the run's session transcripts and proposes revisions:

- edits to `KNOWLEDGE.md` (facts that were needed but missing, or present but
  ignored — the latter suggests a findability/format problem, not a content one);
- edits to the prompt charters (`prompt.plan.md`, `prompt.execute.md`) or the
  handoff format;
- flagged waste patterns (rediscovery, repeated failed commands the cmdcost
  gate didn't cover).

**The gate is the human, not a metric.** AUTOMEM's fixed-seed improvement gate
is impossible here, so the proposal is never auto-applied: `retro` emits a
diff or a triaged issue for the operator to review — the same trust boundary
as the merge itself, consistent with [ADR-0014](../adr/0014-hitl-in-path-visibility.md)
and [ADR-0016](../adr/0016-queue-label-precedence.md) (human-return outranks
queue). *Propose, don't apply.*

**Trigger condition — do not build speculatively.** This only pays off with
evidence of recurring memory failure and enough run volume to amortize the
cost. Before building anything, answer the cheap empirical questions:

1. What is the actual hit-rate in `.ralphy/knowledge/citations.jsonl`?
2. How often are predecessor handoffs (`handoffs.md`) actually used by
   dependent issues?
3. Is there visible rediscovery waste in transcripts (same environment fact
   re-derived across issues)?

If citations are healthy and pruning is working, the current subsystem is
delivering and `retro` stays parked. A nightly personal backlog likely does
not generate the volume; this becomes interesting if Ralphy ever runs at
team/CI scale.

## Status

Idea parked, not scheduled. No ADR until the trigger condition is met.
Recorded 2026-07-03 after reviewing the AUTOMEM paper.
