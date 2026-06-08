# ralphy-pty: PTY foundation (Windows ConPTY)

**Type:** AFK
**Triage:** needs-triage
**Spec:** [ADR-0002](../adr/0002-core-agnostic-adapter-boundary.md), [CONTEXT.md](../../CONTEXT.md)

## What to build

The shared `ralphy-pty` crate over `portable-pty`: open a child process inside a
PTY on Windows (ConPTY), stream its output, and send it input. This is the
load-bearing capability that replaces the ps1's "new console window" trick, and
the future home of on-screen / Tauri terminal and supervised sessions.

It is a shared crate (multiple consumers), **not** core infrastructure — the core
never depends on it. Verifiable on its own via a demo binary, independent of the
rest of Ralphy.

Scope:
- Thin PTY abstraction: spawn command + args + cwd + env in a PTY; read master
  output; write to master; resize; wait/kill the process tree.
- Windows ConPTY backend via `portable-pty`.
- Demo binary that runs an interactive program through the PTY and captures its
  TTY-rendered output.

## Acceptance criteria

- [ ] A demo binary spawns an interactive command through the PTY and captures its
      output on Windows.
- [ ] Input can be written to the child via the PTY master.
- [ ] The child process tree can be killed and waited on.
- [ ] `ralphy-core` does not depend on this crate.

## Blocked by

None - can start immediately (parallel to 0001).
