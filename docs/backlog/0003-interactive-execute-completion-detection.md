# Interactive execute + completion detection (one issue)

**Type:** AFK
**Triage:** needs-triage
**Spec:** [ADR-0002](../adr/0002-core-agnostic-adapter-boundary.md), [CONTEXT.md](../../CONTEXT.md)

## What to build

Drop `--dry-run`: `ralphy run --repo <r> --only-issue N` plans **and executes** one
issue interactively over a PTY, committing onto the run branch and classifying the
outcome. This is the heart of the subscription-billing path (interactive over PTY,
per [ADR-0002](../adr/0002-core-agnostic-adapter-boundary.md)). Parity oracle:
`ralphy.ps1 -OnlyIssue N` (interactive).

Completion detection ports the **current** mechanism faithfully (Q3): no PTY-stream
scraping — read the clean transcript.

Scope (all inside `ralphy-agent-claude`):
- Launch `claude` interactively over `ralphy-pty` with `--settings`,
  `--dangerously-skip-permissions`, `--model`, `--effort`, `--remote-control ralphy-<n>`,
  and the initial prompt (`.ralphy/exec.md` charter).
- Generate the run-scoped `ralphy.settings.json` (Stop hook entry pointing at
  `ralphy hook stop`).
- `ralphy hook stop` subcommand: reads `last_assistant_message` / transcript JSONL,
  writes `DONE` / `BLOCKED <reason>` to the flag file.
- Orchestrator polls the flag file, reclaims the process tree on signal; per-issue
  wall timeout (`--max-minutes-per-issue`).
- Tier↔model translation confined to a single point (`sonnet`/`opus`) per Q5.
- Commits land on the run branch; outcome returned to core as `Outcome`.

## Acceptance criteria

- [ ] `ralphy run --only-issue N` runs an interactive Claude session over the PTY
      (no separate console window) and the session is followable via Remote Control.
- [ ] A session emitting `RALPHY_DONE_EXIT` is classified DONE; `RALPHY_BLOCKED_EXIT`
      is classified BLOCKED with its reason.
- [ ] The sentinel is read from the transcript, never scraped from the PTY stream.
- [ ] Per-issue timeout reclaims a hung session.
- [ ] Commits made by the agent land on the run branch.

## Blocked by

- #1 (walking skeleton)
- #2 (ralphy-pty foundation)
