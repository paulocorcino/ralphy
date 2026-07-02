# `ralphy init`: a Rust-orchestrated onboarding command

Status: accepted — implemented as `crates/ralphy-cli/src/init.rs` (staged
pipeline, gates, checkpoint, skills sparse-fetch).

A repo is not usable by `ralphy run` until a lot of scaffolding exists: the
`.ralphy/` workspace, the `docs/agents/*` config the engineering skills read, the
triage/queue **labels** on GitHub, optionally a backlog turned into issues, and
the engineering skills installed into the dev's agent directory. Today that is all
manual. `ralphy init` is the interactive onboarding command that brings an
*unprepared* repo to a runnable state.

The central design tension: the work splits cleanly into **deterministic**
(detect `python`/`gh`/agent CLIs, verify logins, `git` commit + branch, create
labels, download skills) and **judgment** (read a legacy backlog and reshape it
into standard issues, correlate milestones into PRDs). The deterministic half must
be a testable, idempotent Rust shell with hard gates; the judgment half can only
be done by an agent.

## Decision

**`init` is a Rust subcommand that orchestrates; the agent only executes.** The
binary owns all control flow, gates, git, labels, and the interactive prompts. It
spawns agent sessions only for the read/judgment work, and each session receives a
**fully-assembled, non-interactive prompt** — the interactivity lives in Rust, not
in the agent.

This deliberately **inverts `setup-pocock`'s conversational model**: that skill
explores-then-asks-then-writes inside an agent session. Under `init`, the *asking*
moves to a Rust-native console Q&A; `init` reuses the skill's templates and
write-rules but feeds it answers instead of letting it interview. Rationale:
deterministic, testable, and the operator controls exactly what is asked — a
gate the LLM cannot "decide" to skip.

### Stage order

1. **Deterministic validation (gate).** Hard-fail unless *all* hold: ≥1 agent CLI
   (`claude`/`codex`/`opencode`) present **and logged in** (proven by a
   hello-world call, run against *every* CLI found; the gate trips only if none
   pass), `gh` authenticated, a git repo with a GitHub `origin`, and `python`
   present. A dirty working tree is **not** a gate here (unlike `run`) — `init`
   expects it and handles it in step 4.
2. **Repo diagnosis (agent, read-only).** A session scans the repo and returns a
   **structured report against a Rust-defined schema** (existing project vs empty,
   language/build, backlog file/dir + location, milestone docs, existing
   `.agents`/`.claude`/`.codex`/`.cursor` skills dir, existing `CONTEXT.md`/ADRs,
   remote host). Run from a **neutral cwd** with the target repo passed as *data*,
   so the agent CLI does not auto-load the target's `CLAUDE.md`/`AGENTS.md` as
   system instructions and let them hijack the diagnosis (see Considered options).
3. **Interactive Q&A (Rust).** The console questions are **pre-filled by the
   diagnosis** — the dev confirms/corrects findings rather than answering blind.
4. **Git.** If the tree is dirty, show `git status` and ask permission to
   `git add -A && git commit` (`chore: snapshot before ralphy init`); **refusal
   aborts** (a clean tree is required to isolate `init`'s changes in a reviewable
   diff). Then create branch `ralphy/init` (recommended, declinable) so nothing
   touches the main branch — consistent with the product's "never push, merge by
   hand" stance.
5. **Scaffold** (setup-pocock templates) — writes on the branch.
6. **Skills download** — writes on the branch.
7. **Labels** (Rust, GitHub-side) — create all (queue + human + 5 triage +
   `stop-before`), idempotent (skip existing), after listing them for confirmation.
8. **Backlog/milestone → issues** (agent, conditional). Milestone docs first:
   `to-prd` → PRD + GitHub Milestone, then `to-issues` creates issues linked to it.
   Loose backlog: `to-issues` reshapes it to the tracer-bullet standard. Both
   **preview-then-confirm**: the agent materializes a local draft in `.ralphy/`,
   `init` summarizes it ("will create 14 issues, 3 in milestone X"), the dev
   confirms, *then* it publishes — a bulk external write is never done blind.
9. **Checkpoint.** Each completed stage is recorded in `.ralphy/init-state.json`
   (gitignored), including created issue numbers, so a re-run resumes rather than
   refaz and **never duplicates issues**.
10. **Static verification + handoff.** Confirm artifacts exist (`.ralphy/`, docs,
    labels, ≥1 queue-labeled issue) and print the next step
    (`ralphy run --only-issue <N> --dry-run`). A real `--dry-run` smoke test is
    **offered, not automatic** (it would spend an agent session). If the queue is
    empty, warn rather than report silent success.

### Execution shape

A **chained pipeline of verifiable agent sessions** (mirrors how `run` splits
plan/execute), not one monolithic prompt: each block is verified on completion, so
a failure is localized and resumable. Blocks 8 are conditional on the
diagnosis/Q&A confirming a backlog/milestone — which is also why empty-repo vs
existing-project needs **no separate mode**: an empty repo simply skips those
blocks. `init` accepts `--agent` like `run`, defaulting to the first logged-in CLI
(preferring `claude` when present).

## Considered options

- **`init` as a pure skill/prompt, no Rust.** Rejected: the environment gates
  could not be *enforced* (the LLM could skip them), it is untestable, and it
  duplicates the CLI detection the Rust side already has (`find_program`).
- **Agent conducts the Q&A (keep setup-pocock conversational).** Rejected for the
  judgment-shaping questions: non-deterministic and the operator loses control of
  what is asked. The agent still does the *reading* (diagnosis) and the *writing*
  (scaffold/issues); only the *asking* is pulled into Rust.
- **Diagnose in the repo's own cwd.** Rejected: agent CLIs auto-load
  `CLAUDE.md`/`AGENTS.md` as system instructions, which can sabotage or bias a
  diagnosis. Temporarily moving those files out was also rejected — it mutates the
  repo during a read-only phase and can leave it broken if the process dies. A
  neutral cwd with the repo as data is read-only-true.
- **Embed `agents_template/skills` in the binary** (like `assets/plugin`).
  Considered, but **download from the URL** was chosen: the dev already has an
  authenticated `gh`, so a `git` sparse checkout (pinned to the binary's release
  tag, `RALPHY_VERSION`) is low-friction and keeps the skills decoupled from the
  binary. Download failure is **warn-and-continue** — these skills are for the
  dev's own use and are not required by `ralphy run`.
- **Publish issues directly from the agent.** Rejected in favor of
  preview-then-confirm: a misread of a legacy backlog otherwise leaves dozens of
  wrong issues on GitHub to clean up by hand.

## Consequences

- One new subcommand and a new `.ralphy/init-state.json` checkpoint file; no change
  to the `Agent` trait or `run`'s flow.
- `agents_template/skills` gains a runtime consumer (the sparse-checkout download),
  so its layout becomes a compatibility surface for `init`.
- `setup-pocock` keeps working standalone (conversational); `init` is a second,
  non-interactive caller of the same templates/rules.
