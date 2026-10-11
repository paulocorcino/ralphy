# A reviewer agent reads the change after the verify gate and can only veto it

Status: proposed
Kind: structural
Protects: integrity of change, recoverability of an unattended run

## Context

In an unattended run, an issue becomes **green** when the executor reports
done, the protocol gate accepts the plan (ADR-0015), and the verify gate runs
the issue's checks and sees them pass (ADR-0011). Then the runner closes the
issue (`close_and_record`, `crates/ralphy-core/src/runner.rs:518`), and a
human merges the **run branch** later.

Nothing between "the checks pass" and "the issue closes" reads the change
itself. The checks prove what they test. They do not see a change that does
more than the issue asked, skips part of the plan, weakens a test so it
passes, or edits a file the plan never named. Today a human finds that at
merge time, after the issue is closed and the run has moved on.

Facts this decision builds on (read on 2026-10-11):

- The verify gate gives the executor at most two repairs
  (`VERIFY_MAX_REPAIRS = 2`, `crates/ralphy-core/src/runner/phases/verify.rs:24`).
  Each repair is a new `execute` of the same plan, with the failure written
  to `.ralphy/verify-failure.md` (`crates/ralphy-core/src/runner/artifacts.rs:12`).
  The protocol gate bounces once the same way, through
  `.ralphy/protocol-failure.md`. After the last failure the issue stays
  open, a `## Verify` comment is posted, the result is
  `Skipped(VerifyFailed)`, and the run goes on to the next issue
  (`runner.rs:466-485`).
- The agent port has three methods: `name`, `plan`, `execute`
  (`crates/ralphy-core/src/agent.rs:13-28`). A **split run** wraps a planner
  and an executor from two adapters (`crates/ralphy-cli/src/split_agent.rs`,
  ADR-0009).
- The runner does not record the commit an issue started from. It reads the
  head before and after each execute only to count commits
  (`runner/phases/execute.rs:70, 86-97`); the only stored ref is the run's
  `compare_ref` (`runner/branch.rs:59`).
- The ledger's `phase` is free text, with the values `plan`, `execute`,
  `protocol-repair`, `repair` and `consolidate` in use
  (`crates/ralphy-core/src/ledger.rs:36-38`).

## Decision

**D1. The phase roles are a closed set of four:** triager, planner,
executor, reviewer. In code they are one Rust enum, matched exhaustively. A
fifth role is a decision, not a variant added in passing. The one-shot tasks
(diagnose, draft, consolidate) are not roles (ADR-0031).

**D2. Phase roles talk only through role artifacts.** One role writes a
file in `.ralphy/` and the next role reads it, at a point the runner
controls: the plan, the verify and protocol failure briefs, the handoff, and
the two files of D5 and D7. The runner also posts the outcome as an issue
comment, so the forge keeps the record. No role sends a live message to
another role, and no role reads another role's session.

**D3. The review gate runs after the verify gate and before
close-on-green.** It runs only on an issue whose verify gate is green. It is
off by default (`review.enabled`, default `false`). When it is off, the
runner behaves exactly as it does today.

**D4. The reviewer can only veto.** An approval lets the issue go on to
close-on-green as today. A rejection keeps it from being green. Nothing the
reviewer says can make an issue green that the verify gate did not see pass,
and the reviewer never closes, labels, pushes or merges.

**D5. The reviewer reads three things, prepared by the runner.** The issue,
the plan, and the diff of this issue's commits, written by the runner to
`.ralphy/review-input.diff`. The runner records the commit each issue starts
from, before its first execute, and keeps it across a resumed run. If that
commit is not known, the gate cannot review, and the issue is not green
(D8).

**D6. The reviewer starts with a fresh context.** It runs in a new process,
with no session, history or transcript of the executor. It may be the same
adapter and the same model as the executor; the independence comes from the
clean context, not from a different vendor.

**D7. The verdict is a file with a fixed first line.** The reviewer writes
`.ralphy/review.md`. Its first line is exactly `VERDICT: APPROVE` or
`VERDICT: CHANGES REQUESTED`; the findings follow. The runner reads only
that line to decide. A missing file, any other first line, an adapter error,
a timeout, or a commit made during the review (the head moved) is **not an
approval**. The runner checks the head before and after the review.

**D8. One round goes back to the executor.** On `CHANGES REQUESTED`, the
runner writes the findings to `.ralphy/review-findings.md` and calls
`execute` with the same plan, the way the verify repair works. Then the
protocol gate, the verify gate and the review gate run again. A second
result that is not `APPROVE` ends the issue's turn: it stays open, a
`## Review` comment with the findings is posted, the result is
`Skipped(ReviewRejected)`, and the run goes on to the next issue. A usage
limit during a review follows the limit rules of every other phase
(ADR-0003).

**D9. The agent port gains `review`.** The port becomes `name`, `plan`,
`execute`, `review`. `review` takes the issue and the workspace and returns
the verdict with its usage. An adapter that does not implement it returns a
"not supported" error. The run checks, before the first issue, that the
adapter chosen as reviewer supports it, and refuses to start otherwise; it
never finds out mid-run. A split run carries a reviewer as a third member.

**D10. Each role names its adapter, model and effort.** The reviewer is set
by `--review-agent`, `--review-model`, `--review-effort`, or the settings
keys `review.agent`, `review.model`, `review.effort`. With no reviewer set,
the reviewer is the executor's adapter, in a new process (D6).

**D11. The reviewer runs under the guard and does not change the tree.** Its
session carries the guard hook where the vendor allows one (ADR-0072 D6),
its charter says it only reads, and D7's head check catches a commit. Its
charter, `assets/prompts/prompt.review.md`, runs in every project Ralphy
works on, so it holds no rule that is only about Ralphy's own code.

**D12. The review is visible like every other phase.** The ledger records
`review` (the reviewer) and `review-repair` (the executor's round). The run
snapshot gains the phase state `reviewing`. The event vocabulary in
`core::emit` gains one event for the verdict and one for a rejected issue
(ADR-0039), and every sink follows: the console, Telegram, CloudEvents and
the run summary (as a skipped issue with the kind `review_rejected`).

**D13. Inside one run, the roles work one issue at a time.** No phase role
starts on the next issue while another role is still on this one, and no
two roles of different issues run at the same time in the run's checkout.
Two runs in one checkout are the separate problem ADR-0058 addresses.

## Consequences

- **ADR-0002 is amended:** the port, its one hard-to-reverse commitment,
  gains a fourth method. Core still names no vendor.
- **ADR-0011 is amended:** the gate order is protocol, verify, then review.
- **ADR-0008 is amended:** its D6 phase list was already behind the code
  (it misses `protocol-repair` and `repair`); this adds `review` and
  `review-repair` and states the list the code writes.
- **ADR-0009 is amended:** a split run has up to three adapters.
- **ADR-0039, ADR-0047:** two events and one snapshot state, additive.
- **ADR-0040 gains rows:** each adapter implements `review` (Tier 1), and
  the review charter joins the prompts (Tier 2). An adapter is not
  required to implement it at once; D9 refuses it as reviewer until it does.
- **Cost and time:** a reviewed issue costs one more session, and up to
  three more (review, the executor's round, the second review) when the
  first review rejects. That is why the gate is opt-in.
- **Throughput can drop on false alarms.** A reviewer that rejects good work
  leaves the issue open for a human. Integrity is kept; only speed is lost.
- **Scale waits for this gate.** One checkout per run (ADR-0058, deferred)
  stays deferred until this ADR is accepted, because more parallel runs only
  pile up more branches for a human to read. When it comes, the number of
  runs per repo is a setting with a default of 1.
- **Later:** the same reviewer role can run on a pull request once Ralphy
  pushes and opens one (ADR-0046); this ADR does not decide that trigger.

## Considered options

- **Review on the pull request, after the run.** Ralphy does not open pull
  requests, and its push on its own is still proposed (ADR-0046). The gate
  inside the run needs neither.
- **Let the reviewer approve an issue that the verify gate failed.** That
  would let a model's opinion replace the check the runner saw pass, which
  is the line that defines green.
- **More than one round.** Each round is a full execute and three gates. One
  round fixes what a review can name; a second rejection is a question for a
  human.
- **The reviewer talks to the executor while it works.** Two models
  coordinating with no gate between them; the runner could no longer say
  which state was reviewed.
- **A separate `Reviewer` trait instead of a method on the port.** The
  reviewer is chosen per run exactly like the planner and the executor, and
  `SplitAgent` already routes each method to its adapter. A method keeps
  one port.
- **Parse the whole review for a verdict.** A free-text reading is not
  deterministic (ADR-0015). One fixed first line is.

## Compliance

- D1: not checked by code: not built yet; the enum's exhaustive matches will
  list every place a new role must change.
- D2: not checked by code: the role artifact paths are constants in
  `runner/artifacts.rs`; a new path between roles is reviewed in the PR.
- D3: not checked by code: not built yet; a behaviour test will pin that a
  run with the gate off is unchanged.
- D4: not checked by code: not built yet; the planned check is that
  `close_and_record` is reached only on the verify gate's green and the
  review gate's approval, pinned by a runner test with a fake reviewer.
- D5: not checked by code: not built yet.
- D6: not checked by code: needs a live vendor to prove no shared session;
  the adapter's review call takes no session id from the executor.
- D7: not checked by code: not built yet; a test of the verdict parser will
  pin the two accepted first lines and the head check.
- D8: not checked by code: not built yet; a runner test with a fake reviewer
  that rejects twice will pin one round.
- D9: checked in part by `core_and_adapters_keep_their_dependency_edges`
  (`crates/xtask/tests/crate_dependencies.rs`): core still names no vendor
  after the port grows; the start-of-run refusal is not built yet.
- D10: not checked by code: manual, reviewed in the PR.
- D11: not checked by code: the guard needs a live vendor; the head check
  is D7's.
- D12: checked in part by `every_decoder_arm_has_a_pin`
  (`crates/ralphy-cli/src/runstate/capture/tests.rs`): once the two events
  have decoder arms, an arm without a pin fails it.
- D13: not checked by code: the runner's loop works issues in sequence
  today (`run_queue_with`, `runner.rs`); no check stops a future overlap.
  Across runs, the run lock only warns without `--if-idle`
  (`crates/ralphy-cli/src/run.rs`), as ADR-0058 records.
