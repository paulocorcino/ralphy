# Walking skeleton: dry-run plan for one issue

**Type:** HITL
**Triage:** needs-triage
**Spec:** [ADR-0002](../adr/0002-core-agnostic-adapter-boundary.md), [CONTEXT.md](../../CONTEXT.md)

## What to build

The thinnest end-to-end path of the Rust rewrite: `ralphy run --repo <r> --only-issue N --dry-run`
plans one issue and stops, with no source changes. This establishes the Cargo
workspace and the boundary that every later slice builds on.

It cuts through every layer: CLI arg parsing → core (queue/branch/run lifecycle)
→ Claude adapter (`claude -p` planning) → `.ralphy/` scratch → logging. Parity
oracle: `ralphy.ps1 -OnlyIssue N -DryRun`.

Scope:
- Cargo workspace: `ralphy-core`, `ralphy-cli`, `ralphy-agent-claude`, `ralphy-pty` (stub).
- `ralphy-core` defines the **PTY-free** `Agent` trait (`plan`/`execute`) and the
  domain types (`Issue`, `Plan`, `Outcome`, `Workspace`). The core never names
  `claude`, PTY, or model names.
- `ralphy-cli` parses the core flags (`--repo`, `--only-issue`, `--dry-run`,
  `--base-branch`, `--plan-model`, `--plan-effort`), resolves the git toplevel,
  fetches the issue via `gh`, writes `.ralphy/issue.json`.
- Run branch `afk/run-<stamp>` cut off `--base-branch` (default `origin/main`).
- `ralphy-agent-claude` runs `claude -p` with `prompt.plan.md` piped on stdin →
  `.ralphy/plan.md`; counts open steps.
- Dry-run cleanup: return repo to original branch, drop the empty run branch.

## Acceptance criteria

- [ ] `ralphy run --repo <r> --only-issue N --dry-run` produces `.ralphy/plan.md`
      with the same shape the ps1 produces for the same issue.
- [ ] The `Agent` trait signature contains no PTY type and no `claude`-specific types.
- [ ] `ralphy-core` has no dependency on `portable-pty`, Tauri, or the Claude adapter.
- [ ] On a clean dry-run the repo is returned to its original branch and the empty
      run branch is removed.
- [ ] Aborts cleanly if the working tree is dirty or the base branch is missing.

## Blocked by

None - can start immediately.
