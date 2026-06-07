# Backlog — Rust rewrite (parity-first, Windows)

Tracer-bullet vertical slices for the Rust rewrite of Ralphy. Each slice cuts
CLI → core → adapter → outcome end-to-end and is verifiable against `ralphy.ps1`
as the parity oracle. Design rationale: [ADR-0002](../adr/0002-core-agnostic-adapter-boundary.md)
and [CONTEXT.md](../../CONTEXT.md).

Sequence (this backlog): **parity Windows** → multi-OS → adapters (Codex/OpenCode)
→ Tauri. Only parity-Windows is decomposed here.

| # | Slice | Type | Blocked by |
|---|---|---|---|
| [0001](./0001-walking-skeleton-dry-run-plan.md) | Walking skeleton: dry-run plan for one issue | HITL | — |
| [0002](./0002-ralphy-pty-foundation-conpty.md) | `ralphy-pty`: PTY foundation (Windows ConPTY) | AFK | — |
| [0003](./0003-interactive-execute-completion-detection.md) | Interactive execute + completion detection | AFK | 0001, 0002 |
| [0004](./0004-pretooluse-guard-hook.md) | PreToolUse guard hook (`ralphy hook guard`) | AFK | 0003 |
| [0005](./0005-queue-loop-close-on-green-stop-on-nongreen.md) | Full queue loop: close-on-green + stop-on-non-green | AFK | 0003 |
| [0006](./0006-stop-before-usage-limit-reset-report.md) | stop-before + usage-limit stop + reset report | AFK | 0005 |
| [0007](./0007-staged-plan-routing-triage-labels-resolution.md) | Staged-plan routing + triage-labels.md resolution | AFK | 0005 |
| [0008](./0008-branch-modes-preconditions-run-wrapup.md) | Branch modes + preconditions + run wrap-up | AFK | 0005 |
| [0009](./0009-headless-exec-fallback.md) | Headless `-p` execute fallback | AFK | 0003 |

All slices carry triage `needs-triage`.
