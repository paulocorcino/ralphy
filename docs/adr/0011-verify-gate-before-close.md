# Runner-enforced verify gate before close-on-green

Status: accepted.

Today **green = the agent emitted `RALPHY_DONE_EXIT`** — a self-report. The runner
closes the issue on the agent's word ([runner.rs](../../crates/ralphy-core/src/runner.rs)
`if outcome == Outcome::Done`). For a tool that closes issues unattended overnight,
that is the central trust gap: the agent can declare "done" without the work
actually being verifiable.

This ADR adds a **runner-side verify gate**: between the executor returning
`Outcome::Done` and the runner closing the issue, the runner re-runs a set of
commands the plan declares, over the committed state, and **only closes if they
pass**. Green stops meaning "the agent said so" and starts meaning "the runner
*saw* the verification pass on the code you will merge".

The gate is deliberately **technology-agnostic**: it runs whatever commands the
plan names and checks their exit codes. It knows nothing about Rust, Node, Python,
or any ecosystem — the same machinery verifies a `cargo test`, a `pytest`, an
`npm test`, or a `make check`.

## Decision

### The `## Verify` plan section

The planner emits a `## Verify` section in `.ralphy/plan.md` — vendor-neutral
markdown, parsed by the same molecule as the acceptance ledger
([acceptance.rs](../../crates/ralphy-core/src/acceptance.rs)): `section_after_heading`
plus a line split. **One command per line**, code-fence-tolerant:

```markdown
## Verify

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p ralphy-core
```

The explicit opt-out, the only way to skip the gate from the plan:

```markdown
## Verify

none
```

Parsed shape:

```rust
pub enum VerifySpec {
    None,                       // `none` — planner judged nothing is machine-verifiable
    Commands(Vec<Vec<String>>), // one or more commands, each tokenized into argv
    Unspecified,                // section absent or present-but-empty → runner falls back
}
```

### Execution semantics

- **Direct argv execution, no shell.** Each line is tokenized and run as `argv`
  directly — not through `pwsh`/`sh`. This is what makes `## Verify` portable for
  free: `cargo test -p ralphy-core` runs identically on Windows and Linux. The
  consequence is deliberate: **no `&&`, no pipes, no globs**. Chaining is the
  runner's job, not the markdown's. A command that genuinely needs a shell writes
  `sh -c "…"` explicitly — the discouraged path, not the default.
  - **Windows shim resolution (a no-shell exception that proves the rule).** A bare
    `CreateProcess` (Rust's `Command`) only appends `.exe`, so it never finds — and
    could never execute — the `.cmd` shims the Node ecosystem ships (`pnpm`, `npm`,
    `yarn`, `npx`). Without help the gate fails to even spawn them ("program not
    found"), which is exactly how a Node monorepo's gate went red while the agent's
    own shelled `pnpm install` had passed. So on Windows the runner resolves the
    program through `PATHEXT` and routes a resolved `.cmd`/`.bat` through `cmd /C`.
    This is **not** the rejected shell-the-whole-line path: the tokenized args still
    pass as separate `argv` entries, so no user `&&`/pipe is reinterpreted — only the
    one resolved script runs. Unix is unaffected.
- **Sequential, all must exit 0, stop on first failure** — an implicit `&&`. The
  first non-zero exit fails the gate.
- **Runs in `repo_root`, over the commit.** Monorepos scope inside the command
  itself (`cargo test -p foo`, `npm --prefix packages/x test`); the runner invents
  no `cwd` directive.
- **Runs within the issue's remaining time budget** (`--max-minutes-per-issue`).
  Exceeding it fails the gate like any non-zero exit — a hung verification cannot
  become green by silence.

### Resolution precedence

```
## Verify in plan.md        (per-issue, planner-emitted)   → strongest
settings.json verify.command (per-repo default)            → middle
(nothing resolves)          → close on self-report + loud warn → weakest
```

`## Verify: none` is the **only** explicit opt-out and skips the fallback. A
section that is *absent or present-but-empty* is treated as a planner omission and
falls through to `settings.json`; if that is also unset, the runner closes on the
agent's self-report but emits a loud `warn!` ("issue closed without a verify
gate"). Coherent with the product's no-silent-caps ethos: the absence of
verification is always a visible decision, never a silent hole.

### Gate outcome

- **Pass** → the existing close path runs unchanged (close-on-green, acceptance
  evidence, handoff, knowledge note).
- **Fail** → the issue does **not** close. The runner hands the failing commands
  back to the agent for a bounded number of **repair attempts** (`VERIFY_MAX_REPAIRS`,
  currently 2; see the amendment below) and re-runs the SAME gate after each. When
  the repair budget is exhausted the issue is left **open** and the run **moves on
  to the next issue** — a red gate no longer halts the whole queue.

## Amendment (2026-06-26): bounded repair, then skip-and-continue

The original decision stopped the *entire run* on the first gate failure. Two
problems in practice: (1) many gate failures are *fixable in place* — a stale
lockfile, a missing dependency, a trivially broken test — so stopping outright made
a human step in for something the agent could have closed itself; and (2) halting
the whole queue for one bad issue starved every later, independent issue of its
turn. The gate now (1) gives the agent a bounded chance to repair, and (2) on a
still-red gate, skips that issue and continues the queue instead of stopping.

Mechanics, all runner-side so the **trust model is unchanged**:

- On a failed gate, the runner writes `.ralphy/verify-failure.md` (a vendor-neutral
  repair brief: the failing command(s), the output tail, and a blunt instruction to
  fix the *root cause* and never weaken the gate) and re-runs `execute()` against
  the unchanged plan. The exec charter reads that file as its top priority.
- After each repair the runner re-runs the **same** `## Verify` commands. A repair
  earns the close **only** by making the runner *see* the gate pass — it never gets
  to self-report past a red gate. The deterministic commands stay the authority;
  this is explicitly *not* the rejected "gate as a shipped skill".
- The budget is `VERIFY_MAX_REPAIRS = 2` attempts. Repairs run within the issue's
  existing `--max-minutes-per-issue` budget (no new time knob).
- **Budget exhausted → skip, not stop.** The issue is left open with its commits on
  the branch and the failing artifact comment, and the run continues with the next
  issue. The miss is reported as a `verify failed` **skip** (not a close, not a
  silent hole) so it is visible in the live card and the final counts — consistent
  with the no-silent-caps ethos. `StopReason::VerifyFailed` is therefore retired.
- **The one thing that still stops:** a usage limit *during* a repair. That is a
  global resource exhaustion, not this issue's fault — there are no tokens left to
  work the rest of the queue — so it stops on the limit's reset, the same stance
  the execute path already takes.
- Repair tokens are accounted as their own `repair` ledger phase (ADR-0008), so the
  initial `execute` line stays truthful and the repair cost is never hidden.
- The brief is cleared when the gate goes green and at each issue's start, so it
  reflects only the current run's gate state — never bleeding into a later run on
  the same worktree.

**Consequence to weigh:** because the run branch accumulates, a later issue builds
on top of a skipped issue's (unverified) commits. This matches how the queue already
accumulates *passing* issues on one branch; the skipped issue's work is not rolled
back. A human reconciles the branch at merge time, guided by the per-issue verify
artifacts.

### The honesty artifact

On a gate run (pass or fail) the runner posts a comment on the issue recording
**each command, its exit code, and (on failure) a tail of its output**:

```
## Verify (Ralphy run 2026-06-17-…)
✓ cargo fmt --check            exit 0
✓ cargo clippy … -D warnings   exit 0
✗ cargo test -p ralphy-core    exit 101
  <tail of the failing output>
```

This is what you read in the morning to understand why an issue did not close, and
it is the executable backing behind the acceptance ledger's `[verified]` criteria —
the ledger says *which* criterion was proven; `## Verify` is *how* it was proven.

## Considered options

- **Per-ecosystem auto-detection** (`Cargo.toml` → `cargo test`, `package.json` →
  `npm test`, …). Rejected: it is the one part that would make the gate
  language-aware, and it guesses where the plan can simply state. The plan already
  knows what proves the issue; let it say so. Dropping auto-detection keeps the
  gate technology-agnostic and removes a maintenance surface that grows with every
  ecosystem.
- **A new ledger column for the verify result.** Rejected as overengineering: the
  token ledger (ADR-0008) records per-phase *usage*; the gate has no token usage
  and is not a phase. The issue comment plus the `StopReason` in the run report are
  a sufficient and honest record. The ledger stays about tokens.
- **Bullets with metadata** (`- [verified] … — evidence: …`, like the acceptance
  ledger). Rejected: a verify command carries no metadata — it is just "run this".
  Raw lines are the honest representation; bullets would be empty ceremony.
- **A gate as a shipped skill** (agent-side). Rejected: the gate exists precisely
  *not* to trust the agent. A skill is agent behavior and would collapse back into
  a self-report. The gate must be runner-enforced and deterministic.

## Consequences

- **Vendor-neutral and split-run-safe.** `## Verify` is plan markdown, so any
  planner emits it and the **runner** — not the executor — runs it. Under a split
  run (ADR-0009) Claude can author the `## Verify` and the OpenCode executor need
  not know it exists. Consistent with the core/adapter boundary (ADR-0002, 0004):
  the gate lives in the runner, vendor-neutral; adapters still only classify their
  own output into an `Outcome`.
- **One planner-prompt addition:** "emit `## Verify` with the command(s) that prove
  the 'Done when', or `none` if nothing is machine-verifiable." In many issues the
  `## Verify` lines are simply the union of the commands already named in the
  `[verified]` criteria's `evidence:`.
- **`QueueConfig` / settings grow by one knob each:** `verify.command` in
  `settings.json` (ADR-0010's schema already tolerates new keys) and a
  `StopReason::VerifyFailed` variant. No change to the `Agent` trait or the
  `run_queue`/`run` signatures.
- **Default stance is on** whenever a command resolves; `## Verify: none` (per
  issue) or an unset `verify.command` with no plan section (per repo, with a loud
  warn) are the documented ways to run without it.
- **The gate does not replace the acceptance ledger** — `[review-only]` criteria
  remain the human's job at merge time. The gate hardens only the machine-verifiable
  `[verified]` half.
