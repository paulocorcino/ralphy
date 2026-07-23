# A shared outcome classifier over vendor-extracted completion signals

_Amends ADR-0004 D2 (Codex) and ADR-0005 D2 (OpenCode); ADR-0002's `Outcome`
seam is unchanged._

Every adapter maps a session's raw end state onto the core `Outcome`. ADR-0004 D2
deliberately kept that per-adapter and rejected a shared runner that produces an
`Outcome` from raw output. That was right about the *extraction*, but the mapping
grew a second half — a fixed precedence over already-extracted signals — that was
never vendor-specific, and copying it three times (four, counting Claude's
interactive and headless classifiers) let it drift where the compiler could not
see: the commit progress-guard, the limit-vs-done precedence, and the
timeout-vs-limit precedence each diverged. This ADR splits the concern in two and
shares only the half that was never vendor-essential.

## D1 — Two layers: signal extraction (vendor) vs precedence (shared)

- **Camada 1 — extraction (per-adapter, unchanged seam).** Each adapter reduces
  its raw end state to a `CompletionSignals` value. What counts as a *trustworthy*
  `limit` and what counts as a *trustworthy* exit (`exited_ok`) are vendor
  decisions and stay in extraction: Codex sets `limit` only on a non-clean exit
  (a real limit fails the process, so a clean exit *is* the "not a limit" proof);
  Claude sets it from a structural 429 line in the transcript regardless of exit;
  Codex/OpenCode normalize `exited_ok` to a zero exit, Claude headless to
  `!timed_out`. This is the vendor-essential core ADR-0004 D2 protects — untouched.
- **Camada 2 — precedence (shared).** One vendor-neutral
  `classify(CompletionSignals) -> Outcome` applies a single fixed ladder.

## D2 — The precedence ladder is fixed and shared

```
1. limit.is_some()               → Limit(reset)
2. done && exited_ok && !errored → Done
3. timed_out                     → Timeout
4. blocked.is_some()             → Blocked(reason)
5. _                             → Stuck
```

A trustworthy `limit` outranks both `done` and `timeout`: resume-after-reset is
the conservative error. Closing a throttled session (a false green) or reporting
it as a bare `Timeout` is the unsafe mistake; a genuinely-finished issue that also
shows a limit costs at most one wasted resume, never a false close. `done` requires
a trustworthy exit (`exited_ok`) and no error, and — see D3 — never a commit. This
preserves the Claude headless rule (`classify_exec_limit_beats_done_sentinel`) as
the reference behavior.

## D3 — Commit is a progress signal, not a green gate

The executor charter legitimately finishes a session with **no new commit**:

- completing the runner's protocol lint edits only gitignored `.ralphy/plan.md`
  (`prompt.execute.md` — plan durability comes from the file on disk, not git);
- repairing a flaky/environmental verify-gate failure re-passes with no code change
  (the plan's steps are already `- [x]` from prior sessions);
- a pure-investigation or `## Verify: none` issue may produce no diff at all.

Gating `Done` on `committed` misclassifies these hand-backs as `Stuck` and **halts
the run** exactly when the agent finished correctly. So `Done` never requires a
commit. `committed` feeds only the Claude headless multi-call no-commit **streak**
(ADR-0002's "no commit across the streak → Stuck" progress heuristic). The
anti-false-green protection rests on the runner's **verify gate** — it re-runs the
plan's `## Verify` commands over the committed state and refuses to close on a
failure — not on a per-session commit check.

This *corrects* ADR-0004 D2: its "no new commit across the streak → Stuck" was
hardened by the Codex adapter into a per-call `Done` conjunct
(`exited_cleanly && committed && done_sentinel`), stricter than the ADR intended
and incompatible with the hand-back flows above.

## D4 — Claude is the reference; the behavior change lands on Codex and OpenCode

Claude's two classifiers already embody the canonical ladder; they are refactored
to feed the shared `classify` with **no behavior change**. The guardrail is
concrete: every existing Claude classifier test passes against the shared code
**unmodified** — if any Claude test would need editing, that is a Claude behavior
change and the refactor stops.

- **Codex changes:** drop `committed` from the `Done` conjunct; a trustworthy
  `limit` now upgrades a `timeout` (was timeout-wins). Its limit-vs-done precedence
  is moot — a trustworthy Codex limit (non-clean exit) and a clean-exit `Done` are
  mutually exclusive by construction.
- **OpenCode changes:** drop `committed` from the `Done` conjunct; a clean, done
  run that also saw a `limit` event now resumes (`Limit`) instead of closing
  (`Done`).

Both moves are *toward* the Claude-validated rule. No deliberate existing
Codex/OpenCode test asserts the behavior being changed.

## D5 — What stays per-adapter (the ADR-0004/0005 boundary holds)

- raw→signal extraction: flag file, JSON event stream, exit code, `HEAD`-diff,
  transcript scan, `RALPHY_DONE_EXIT`/`RALPHY_BLOCKED_EXIT` sentinel parse;
- `limit` **trustworthiness** and `exited_ok` **normalization** — the vendor alone
  decides when those signals are set;
- the Claude headless drive loop (streak, `MaxCalls`) — genuinely single-vendor.

`ralphy-adapter-support` gains the pure `classify` function and the
`CompletionSignals` type. It still performs **no** raw-output detection of its own,
so ADR-0002's ban on a shared raw-output completion runner is intact — this shares
the *ordering*, not the *detection*.

## Consequences

- The ~30 scattered classifier tests collapse to one `(signals → Outcome)` table
  plus thin per-adapter extraction tests; the precedence is verified once instead
  of three times through three CLIs.
- Drift becomes a compile error: a new signal or a new vendor forces a match arm
  rather than silently omitting a guard (as the Claude/Codex commit-guard did).
- Behavior changes are confined to Codex and OpenCode and are guarded by Claude's
  unchanged test suite.
- P3 (a trustworthy limit upgrades a Codex timeout) aligns Codex with OpenCode's
  ADR-0005 D9 and with Claude.
