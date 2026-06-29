# Loop Engineering, scored against Ralphy

A reading of [Loop-Engineering-IEEE.md](Loop-Engineering-IEEE.md) (the Anthropic
"Loop Engineering" playbook) mapped onto what Ralphy actually does today — what
already lines up, and where the gaps are. The paper's framework is the closest
public articulation of *what Ralphy is*, so it doubles as a design checklist.

The paper's spine: a loop is **five moves** — discovery, handoff, verification,
persistence, scheduling — realized by **six parts**, and the failures of a loop
are simply those moves skipped. The hardest move is verification, "because an
agent grading its own work praises it." Its remedy is structural: a check the
generator does not control.

## Where Ralphy sits in the four-layer stack

The paper stacks prompt → context → harness → **loop**, where the loop is the
floor that "makes it run itself over and over" on a timer. Ralphy is mostly the
**harness-and-one-turn** layer: one `ralphy run` walks the whole labelled queue
once, plan → execute → verify → close, and hands back a branch. The *scheduling*
floor above it — the thing that re-invokes `ralphy run` on a timer — is left to
the user (Task Scheduler / cron / a CI schedule). That is the single cleanest
gap against the playbook, and it is arguably out of scope by design: Ralphy is
the run, not the cron.

## The five moves

| Move | Paper | Ralphy today | Verdict |
|---|---|---|---|
| **Discovery** | Loop finds its own work via a skill | Queue labels (`ready-for-agent`/`AFK`), `## Blocked by` gating, `stop-before`, ascending order | **Partial — by design.** Ralphy discovers the *queue*, but a human chooses *which* issues by labelling. The selection is the deliberate trust boundary, not an oversight. |
| **Handoff** | One isolated git worktree per task | `.ralphy/plan.md` (vendor-neutral) is the plan→execute handoff; sequential, single run branch, **in place, no worktree** | **Met for its model.** No worktree because there is no parallelism — one issue at a time. Worktrees would only matter if Ralphy ever ran issues concurrently (see Tangled loop below). |
| **Verification** | A separate, skeptical agent that *acts* and is judged by a fresh model | Runner-enforced **deterministic** verify gate ([ADR-0011](../adr/0011-verify-gate-before-close.md)): re-runs `## Verify` commands over the committed code, exit codes only, bounded repair (2 attempts) | **Strong, but a different shape.** Ralphy's "say no" is a *deterministic* gate, not an LLM evaluator — which is exactly the Stripe lesson the paper itself endorses ("anything deterministic never goes to a probabilistic model"). What it lacks is *behavioral/semantic* checking beyond exit codes (the paper's Playwright-style "does it run right, not look right"). That half is delegated to the human merge. |
| **Persistence** | State on disk that survives the conversation | Branch commits + `.ralphy/plan.md` + knowledge cache (`issue-<N>.md` → consolidated `KNOWLEDGE.md`) + append-only `usage.jsonl` ledger + runstate | **Strong.** "The agent forgets, the repo does not" is implemented several times over. |
| **Scheduling** | An automation/timer makes one turn into a loop | None built in — the human runs `ralphy run`; `--deadline-hours` bounds the single run | **Gap (likely intentional).** Without an external timer Ralphy is, in the paper's strict sense, the "Manual loop." It is *built to be wrapped* by one, but ships none. |

## The six parts

| Part | Maps to | Ralphy |
|---|---|---|
| **Automations** | Scheduling | External only — no built-in cron/cloud trigger. |
| **Worktrees** | Handoff | Deliberately not used; runs in place to reuse the warm build cache. N/A while sequential. |
| **Skills** | Discovery | Ships `reviewer` + `staged-plan` to every agent — but these serve *planning/review*, not discovery. |
| **Connectors** | Persistence / Discovery | `gh` CLI is the GitHub connector; Telegram notifier is a read-only status connector. No MCP. |
| **Sub-agents** | Verification | Planner/executor split exists ([ADR-0009](../adr/0009-split-planner-executor.md)) — but both *generate*; it is not a generator/evaluator split. The "evaluator" role is filled by the deterministic gate, not a second agent. |
| **Memory** | Persistence | Knowledge cache + ledger + plan. Strong. |

## Generator / evaluator — Ralphy's answer is "deterministic gate"

The paper's central claim is that an agent praises its own work, so judgment must
be structurally separated from generation. Ralphy agrees but resolves it
*without a second LLM*: the runner — not the agent — re-runs the `## Verify`
commands and only closes on a green it *saw* ([ADR-0011](../adr/0011-verify-gate-before-close.md),
[runner.rs](../../crates/ralphy-core/src/runner.rs)). The bundled `reviewer`
skill is agent-side self-review and is explicitly **not** treated as the gate (the
ADR rejects "a gate as a shipped skill" for exactly the paper's reason).

The one residual "nodding" risk: when *no* verify command resolves, Ralphy closes
on the agent's self-report — but emits a loud warning, never a silent hole.

## The four silent costs, and Ralphy's guards

| Cost | Paper's guard | Ralphy's guard |
|---|---|---|
| **Verification debt** | Independent evaluator | Deterministic verify gate (ADR-0011). |
| **Comprehension rot** | Read a sample, explain it | Structural: **never pushes, never opens a PR** — you merge by hand, reading the per-issue verify artifacts and the morning diff. |
| **Cognitive surrender** | Keep one door open | The whole design *is* one door — the human is the merge gate. Plus `--dry-run`, `stop-before`, stop-at-first-failure. |
| **Token blowout** | Hard caps before shipping | **Time** caps, not token caps (no API spend to cap): `--max-minutes-per-issue` (default 90), `--deadline-hours`, stop-at-first-failure, bounded repair. Spend is *measured* (`usage.jsonl`), not budgeted. |

## The five anti-patterns

- **Nodding loop (verification skipped)** — avoided by the gate; residual only when no command resolves (loud-warned).
- **Amnesiac loop (persistence skipped)** — avoided by the knowledge cache + branch.
- **Manual loop (scheduling skipped)** — **this is the open one.** Ralphy is human-kicked today.
- **Blind loop (discovery skipped)** — partially present *by design*: the human labels the work. This is the trust boundary, not a bug.
- **Tangled loop (handoff skipped)** — avoided by sequential single-branch execution; would reappear the day Ralphy runs issues in parallel without worktrees.

## What this suggests, in priority order

1. **Scheduling is the real missing floor.** Everything else is built; a thin,
   first-class "run on a timer" story (even just documented cron/CI recipes, or a
   `ralphy schedule`) would move Ralphy from "Manual loop" to a complete loop by
   the paper's own checklist. Lowest effort, highest conceptual completeness.
2. **Behavioral verification is the deepest gap, by design handed to the human.**
   The deterministic gate proves "exit 0," not "behaves right." If Ralphy ever
   wants to narrow the human's merge burden, an *acting* evaluator (run the thing,
   not just the tests) is where the paper points — but note the ADR's deliberate
   stance that the gate stays deterministic.
3. **Parallelism would require worktrees.** If issue-level concurrency is ever on
   the roadmap, the in-place/no-worktree choice flips from a feature (warm cache)
   into the Tangled-loop trap; worktrees become mandatory at that point.

## Companion paper

[2603.23613v1.md](2603.23613v1.md) (LLMLOOP, ICSME 2025) is the relevant prior art
for evolving the verify gate from a pass/fail portal into a *feedback* loop —
realimenting compiler/test/static-analysis failures back to the agent. Ralphy
already does a bounded version of this (the `verify-failure.md` repair brief,
ADR-0011 amendment); LLMLOOP's five-loop decomposition is the fuller map if that
ever grows.
