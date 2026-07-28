# The orchestrator pushes the run branch; the agent still cannot

Status: **partially accepted** — the *operator-initiated* push is **accepted and
built** (see the amendment at the end, #320). The *orchestrator-initiated* push
below stays **proposed**: its **OPEN** sections carry questions a maintainer
must still settle, and none of that automation is implemented.

Ralphy's loop ends with commits on a run branch and stops there. The operator
finds them by opening the repo — which works when a human ran `ralphy run` and
is sitting in front of it, and works badly for the mode Ralphy is being pointed
at: `ralphy schedule install triage` feeding `ralphy schedule install run`, both
on a timer, with the operator asleep. Work lands on a branch nobody can see from
anywhere but that one working copy, on that one machine.

The restriction that produces this is usually stated as "Ralphy never pushes",
but that is not what the codebase encodes. The guard's own reason is narrower
and more useful:

> `crates/ralphy-cli/src/guard.rs:44` — `r"(?i)\bgit\s+push\b"`,
> *"pushing is the orchestrator's job, not the agent's"*

The `PreToolUse` hook blocks the **agent**, mid-loop, inside a session whose
working tree may be half-edited and whose verify gate has not run. It says
nothing about the orchestrator, which acts only after the gate. The capability
was assigned, not forbidden — and never built.

## Decision

### 1. Push belongs to the orchestrator, and the agent's deny rule stays

`ralphy` pushes the run branch itself, after the verify gate. The `git push`
deny rule in the guard is **kept verbatim**, as is the `prompt.execute.md`
hard rule that tells the agent it is on a shared run branch and must only commit
onto it. This is not a compromise to soften a relaxation: the two are different
capabilities. An agent that can push can push a red tree mid-session, before the
gate that decides whether the work is even coherent; the orchestrator pushes a
known state at a known point. Relaxing the guard would buy nothing the
orchestrator does not already do better, and would remove the only mechanical
stop between a wedged session and a remote branch.

Consequence worth naming: `prompt.execute.md:377-379` currently explains the
rule with "a human reviews and merges by hand". That stays true (see §5), but
the *push* half of that sentence becomes wrong and must be reworded — the agent
does not push because it is the orchestrator's job, not because nobody pushes.

### 2. Opt-in, never a default

Push is off unless the operator turns it on. A tool that starts pushing after an
upgrade, in a repo whose remote the operator never intended to receive
automated branches, is a violation of the (ADR-0032) launcher ethos — Ralphy
does what it was pointed at, on the schedule it was given.

**OPEN 1 — the opt-in surface.** `settings.json` key (`push.enabled`, per repo,
which a scheduled run inherits with no flag) versus a `--push` CLI flag
(explicit per invocation, but a scheduler's crontab is where it would live, so
it becomes a permanently-set flag pretending to be a per-run choice) versus
both. Recommendation: **settings key as the seat of the decision, plus
`--no-push` to suppress it for one run**; a scheduled run is exactly the case
where the setting belongs to the repo, not to the command line.

### 3. Never to a protected ref — the hazard this ADR must not create

The runner has two branch modes (`runner/types.rs:26`). In `BranchMode::New`
the run cuts a fresh `afk/run-*` branch and pushing it is inert: a new remote
branch nobody's tooling watches. In `BranchMode::Current` the run commits onto
**whatever branch the repo is already on**, which may be `main`.

Pushing in `Current` mode without a guard would turn `ralphy run --branch-mode
current` on `main` into an unattended push to `main`. That is the single way
this feature can do real damage, and it must be refused in code, not in prose:
the push is skipped, loudly, when the run branch equals the repository's
default branch or the configured `base_branch`.

**OPEN 2 — is refusing enough, or should `Current` mode never push at all?**
Refusing on the default branch still pushes a `Current`-mode run that happens
to sit on some other long-lived branch (`develop`, a release branch), where an
unattended push is just as unwelcome. The narrower rule — *push only branches
this run created* (`BranchMode::New`) — is trivially safe and costs the
`Current`-mode operator a manual push. Recommendation: **the narrow rule**,
widened later if someone asks with a real case.

### 4. After the gate, and never on a dry run

The push happens at the end of the run (`runner.rs:444-495`), where the loop
already counts what it produced over the compare ref and assembles the
`QueueReport`. A dry run pushes nothing — it has no commits by construction,
and the empty-branch cleanup already deletes what it made.

**OPEN 3 — per issue or once per run?** Per-issue push (right after each
`close_and_record`) makes work visible as it lands and survives a run that dies
mid-queue; once-per-run pushes one coherent state and cannot leave a half-worked
queue on the remote. Recommendation: **once per run**, including runs that
stopped early — a stopped run's commits are exactly the ones an operator wants
to see without walking to the machine. If per-issue visibility is wanted later
it is an additive change, not a redesign.

**OPEN 4 — does a run with a failed verify gate push?** The gate failing means
the issue was NOT closed; its commits still exist on the branch. Pushing them
publishes work Ralphy itself judged unfinished. Not pushing hides the evidence
an operator needs to diagnose the failure remotely — the case that motivated the
whole feature. Recommendation: **push**, because the branch is a proposal and
nothing consumes it automatically; the run report and the events already say the
gate failed.

### 5. Push is not a pull request, and PRs wait for CI

This ADR grants push only. Opening a PR stays out — and not merely for caution:
`.github/workflows/ci.yml:3-7` triggers CI on `push` to `main` and on
`pull_request` targeting `main`. A run branch pushed under this ADR triggers
**no CI at all** (cost: zero). A PR would trigger the `build · test` job that
currently hangs until the hosted runner is killed (#295, red on `main`
continuously since 2026-07-08). Opening PRs unattended today would burn roughly
an hour of runner time per PR and decorate every one of them with a dead check.

Sequence: this ADR → #295 fixed → a separate decision for PRs, alongside the
independent review process that will approve merges. **Merging remains outside
Ralphy entirely**, in this ADR and after it.

### 6. Failure is reported, not fatal

A push that fails (no `origin`, credentials absent, non-fast-forward, network
down) is a warning recorded in the run report and the run's events — never an
error that fails the run or discards work. The commits are already durable
locally; the push is a convenience over them. Ralphy never force-pushes, and
never retries a rejected push by rewriting history — `git push --force` and
`git rebase` stay as forbidden to the orchestrator as they are to the agent.

**OPEN 5 — what does the run report and the event stream say?** The events
vocabulary is ADR-0019/ADR-0039 territory and a new emit site is a real change
there, not a footnote. Minimum: `QueueReport` gains the pushed ref (or the
failure reason) so the CLI's end-of-run summary can state it. Whether a
CloudEvent is emitted, and under which name, is a question for the events
vocabulary owner.

### 7. Where the code goes

`Repo` (`repo.rs:22`) is the runner's git seam, with `git::*` free functions as
its implementation and default method bodies so fakes override only what they
script. Push follows that shape exactly: a `git::push` free function, a `Repo`
method with a neutral default, and the run-end call site in `runner.rs`. The
CLI's composition root resolves the setting and passes it in `QueueConfig`; the
core stays `gh`-free and the loop stays unit-testable against the fake.

## Amendment (2026-07-26, #320) — the operator pushes from the workbench

Everything above is about the **orchestrator**: an unattended actor, on a timer,
pushing after a gate the operator did not watch. Its OPEN questions are about
how to make an *unattended* push safe, and they stay open — nothing in this
amendment settles them or implements any of §2, §4, §5's automation.

This amendment adds a **different actor** with a **different trigger**, decided
by the maintainer:

- **Actor: the operator, in person.** A click on Push in the workbench's Changes
  panel, or a typed `ralphy sync push`. Never a run, never a scheduler, never an
  agent — nothing on any code path here is reachable from the loop.
- **Trigger: the click itself is the consent.** §2's opt-in exists because an
  unattended push happens *without* anyone deciding at that moment; an operator
  pressing a labelled button in their own workbench has decided at that moment,
  which is exactly what the setting would have been standing in for. So there is
  **no `push.enabled` key and no `--push` flag on this path** — a switch whose
  only purpose is to gate a decision the operator is already making by hand is
  ceremony, and one more state that can be silently wrong. §2 still governs the
  orchestrator, which has no operator to ask.
- **Opt-in surface: none, because there is nothing to opt into.** Upgrading
  Ralphy grants no repo an automated push: the code added here runs when, and
  only when, a person invokes it. §2's promise — "a tool that starts pushing
  after an upgrade" must not exist — is kept by construction rather than by a
  default.
- **The protected-ref rule holds, and is the operator's guard too.** §3 is not
  softened by the actor being human: the mis-click is precisely the hazard.
  `sync::push` refuses when the current branch is the remote's own default
  branch (`refs/remotes/<remote>/HEAD`) or the configured `base_branch`, both
  ASKED of the repo rather than guessed from the names `main`/`master`. The
  refusal happens before any git write.
- **Never a force-push, and no credential remediation.** A remote that moved on
  is a refusal telling the operator to pull first; a rejected credential is a
  refusal telling them to authenticate in a terminal. There is no credential
  prompt and no credential UI on this path — `GIT_TERMINAL_PROMPT=0` is pinned,
  as it is for `fetch`, because this runs under a console-less daemon child.
- **§1 is untouched.** The agent's `git push` deny rule in `guard.rs` stands
  exactly as written. The operator gaining a button is not the agent gaining a
  capability.
- **§5 still holds: push is not a pull request.** Nothing in this amendment
  opens one.

Where the code went, correcting §7 for this actor: the seam is
`ralphy_core::sync` (beside `fetch`/`pull`), not `Repo`/`runner.rs` — this push
has no run to hang off. `ralphy sync push` is the run-lock-aware CLI verb, and
the daemon's `sync.push` Mutate verb is what the workbench button spawns. The
orchestrator-side call site of §7 remains unbuilt.

## Consequences

- The AFK loop closes: a scheduled triage promotes, a scheduled run works the
  issue, and the result is visible from anywhere instead of from one working
  copy. This is the point of the change.
- One containment argument disappears and must not be re-cited. "Ralphy never
  pushes" was load-bearing in the ADR-0018 amendment's reasoning about how
  willing `promote` can afford to be; that amendment already avoids leaning on
  it, and other prose that does (`CLAUDE.md`'s contributing note, the product
  ethos line) needs the same correction.
- The undo tag weakens. A run's undo marker assumes the branch is local and
  disposable; once pushed, deleting the local branch no longer un-does anything
  a collaborator has already fetched. The tag stays useful for the local
  working copy and stops being a complete undo.
- Branch proliferation on the remote becomes real (one `afk/run-*` per run).
  Nothing in this ADR prunes them. **OPEN 6** — retention is a policy question
  worth answering before the first hundred branches, not after.
