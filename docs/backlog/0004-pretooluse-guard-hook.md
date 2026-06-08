# PreToolUse guard hook (`ralphy hook guard`)

**Type:** AFK
**Triage:** needs-triage
**Spec:** [ADR-0002](../adr/0002-core-agnostic-adapter-boundary.md), [guard.ps1](../../guard.ps1)

## What to build

The destructive-command deny-list, ported as a subcommand of the Ralphy binary
itself (`ralphy hook guard`) and wired into the run-scoped `--settings` as a
`PreToolUse` hook. This is the only safeguard standing between the agent and a
destructive command under `--dangerously-skip-permissions`, so it must be a hook
(it intercepts *before* the tool runs).

One binary, two roles: the same executable orchestrates and guards, so there are
no loose script files to locate — the cross-OS-portable replacement for the ps1
`guard.ps1`.

Scope:
- `ralphy hook guard`: read the PreToolUse JSON payload from stdin; block (exit 2 +
  reason on stderr) or allow (exit 0).
- Port the command deny-list (git push / reset --hard / clean / rebase /
  checkout / worktree, gh pr merge|close, recursive force-delete, pipe-to-shell,
  disk-level commands) and the file-write deny-list (`.git/`, `.env`, secrets,
  credentials, key files, **Ralphy's own tooling** anchored on the binary's dir).
- Wire it into `ralphy.settings.json` (PreToolUse matcher Bash|Edit|Write|MultiEdit|NotebookEdit).

## Acceptance criteria

- [ ] A `git push` (and each other denied command) is blocked during execution
      with a reason fed back to the model.
- [ ] A write to a protected path (`.git/`, `.env`, secrets) is blocked.
- [ ] A write into Ralphy's own install directory is blocked.
- [ ] An unparseable payload fails safe (allows) rather than stalling the loop.
- [ ] Behaviour matches `guard.ps1` for the parity command set.

## Blocked by

- #3 (interactive execute — provides the settings/execute path the hook plugs into)
