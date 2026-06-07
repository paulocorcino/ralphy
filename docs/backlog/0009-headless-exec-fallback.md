# Headless `-p` execute fallback

**Type:** AFK
**Triage:** needs-triage
**Spec:** [ADR-0002](../adr/0002-core-agnostic-adapter-boundary.md), [CONTEXT.md](../../CONTEXT.md)

## What to build

The secondary execution path for environments with no TTY (CI, cron without a pty):
`ralphy run --headless-exec` drives the issue with a `claude -p` loop instead of an
interactive PTY session. Parity oracle: the ps1 `Invoke-ExecLoop`.

Per [ADR-0002](../adr/0002-core-agnostic-adapter-boundary.md), this is a second
**execution mode** of the Claude adapter — selected by the adapter, invisible to
the core (which still just receives an `Outcome`). Note: headless `-p` is metered
programmatically from 2026-06-15, so this is a fallback, not the default.

Scope:
- `--headless-exec`: loop `claude -p` (prompt on stdin, captured output, hard
  timeout) until DONE / BLOCKED / stuck / timeout / max-calls.
- Detect `RALPHY_DONE_EXIT` / `RALPHY_BLOCKED_EXIT` and the open-step count from
  output; usage-limit detection as in the interactive path.
- `--max-exec-calls` safety cap; stuck detection (no new commit across calls).

## Acceptance criteria

- [ ] `ralphy run --headless-exec --only-issue N` works an issue with no PTY.
- [ ] Outcome classification (DONE / BLOCKED / stuck / timeout / max-calls) matches
      `Invoke-ExecLoop`.
- [ ] The core receives the same `Outcome` type as the interactive path (no leak of
      the execution mode into the core).

## Blocked by

- #3 (interactive execute — establishes the adapter execute path and outcome types)
