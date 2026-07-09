# A build-environment brief the runner hands to every agent

Status: accepted (implemented 2026-07-08).

Autonomous coding agents default to a POSIX/Linux mental model. Left unsaid, a
planner writes `## Verify` steps and smoke scripts that assume tools the host may
not have — a `netstat`-based port check, a bare `python3` — and on a mismatched
machine the plan then bounces the verify gate forever, because the failure is
environmental, not a code bug the executor can repair. This was observed live on
Windows: a `smoke-dev.sh` failing on an absent `netstat` under a shell that could
not see Node, looping the run indefinitely (no per-issue time budget to stop it).

The runner already knows the machine; the agent does not. So the runner tells it.

## D1 — A vendor-neutral `.ralphy/environment.md`, written by the runner

`ralphy_core::environment::ensure_brief` writes a short markdown brief at the top
of a run, through the same channel as `verify-failure.md` / `handoffs.md`: the
runner writes it, any adapter's charter reads it. The plan template and the
execute charter each carry a one-line pointer to it — so all adapters (claude,
codex, kimi, opencode) inherit it from the single assembled template, not
per-vendor prose.

The brief names the OS (name + version + arch, plus a `(WSL1)`/`(WSL2)` tag when
under WSL) and the common toolchains found on `PATH`, each with its detected
version. Detection is cross-platform by construction: `os_info` for the OS label,
the existing PATHEXT-aware `find_program` for resolution, and one `--version`
spawn per found tool (normalized to a bare `x.y.z` token so each banner's wording
is irrelevant). It runs on Windows, Linux, and macOS from one code path.

## D2 — The list is a lead, not a whitelist; omission means "verify", not "absent"

The brief is deliberately **not** exhaustive of the machine — it probes a curated
set for signal, not coverage. So the prose does not claim the list is complete;
it tells the agent to match commands to this OS and to **verify any unlisted tool
before a command depends on it**, and not to install new tools unless the task
asks. This keeps the guard (don't assume `netstat` exists because it is common)
without lying that the machine has nothing else, and without inviting the opposite
failure — hunting for or auto-installing tools mid-run.

## D3 — Written once, cached; best-effort

`ensure_brief` is a no-op when the file already exists, so a resumed run reuses
the first detection and a hand-edited brief is never clobbered. Every failure path
(detection, write) is swallowed with a `warn!`: a missing brief leaves the charter
exactly as it was before this decision, never a failed run.

## Consequences

- The environmental verify-loop trap is addressed at its root (the planner is
  told the host) rather than at the symptom (the executor repairing a gate it
  cannot fix). The bounded verify-failure repair loop remains the backstop.
- The brief is advisory: it makes the agent *avoid* the trap, but cannot fix a
  host where the verify shell genuinely cannot reach a required runtime (e.g. Node
  unreachable under WSL1) — that remains an environment the operator must correct.
- Adds two facts the agent could not derive from the repo, so it lives in
  `ralphy-core` (not an adapter): the OS and the confirmed toolchains. New crate
  dep: `os_info` (OS label); tool resolution reuses `ralphy-proc-util`.
