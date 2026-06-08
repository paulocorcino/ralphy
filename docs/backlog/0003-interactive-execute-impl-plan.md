# Implementation plan — #3 Interactive execute + completion detection

**Issue:** [0003](0003-interactive-execute-completion-detection.md) ·
**Spec:** [ADR-0002](../adr/0002-core-agnostic-adapter-boundary.md), [CONTEXT.md](../../CONTEXT.md) ·
**Oracle:** `ralphy.ps1 -OnlyIssue N` (interactive), `stop_exit_hook.ps1`

## Goal

`ralphy run --repo <r> --only-issue N` (no `--dry-run`) **plans and executes** one
issue: launches `claude` interactively over `ralphy-pty`, lets it commit onto the
run branch, detects completion from the transcript, and returns an `Outcome`.

## Where the boundary falls (ADR-0002)

| Layer | Change |
|---|---|
| `ralphy-core` | **None.** `Outcome` already exists; `runner::run` already calls `agent.execute()` for non-dry runs. PTY, timeout, sentinels are adapter-owned. |
| `ralphy-cli` | New `ralphy hook stop` subcommand; exec-model/effort/timeout/remote-control flags; build `ClaudeAgent` with exec config; clear `ANTHROPIC_API_KEY`. |
| `ralphy-agent-claude` | Implement `execute()` over `ralphy-pty`; settings w/ Stop hook; flag-file polling; reclaim + timeout; outcome classification; one tier→model point. |

## Stages

### Stage 1 — `ralphy hook stop` subcommand (CLI, fully unit-testable)
Ports `stop_exit_hook.ps1`. No PTY, no `claude` — pure I/O, so it lands first and
is verifiable without billing.

- New `Command::Hook { Stop }` subcommand in [main.rs](../../crates/ralphy-cli/src/main.rs).
- Reads the Stop-hook JSON payload from **stdin**; pulls `last_assistant_message`,
  falling back to the last `assistant` `text` block in the `transcript_path` JSONL
  (version-robust, exactly as the ps1 does).
- Writes `DONE` or `BLOCKED <reason>` to the path in `$RALPHY_FLAG_FILE`; no-op if
  the env var is unset (harmless if it leaks into a normal session). Always `exit 0`.
- Factor the parsing into a small `hook` module (e.g. `classify_stop(payload, read_transcript) -> Option<FlagWrite>`) so it unit-tests against fixture JSON without touching the filesystem or env.

**Tests:** payload with inline `RALPHY_DONE_EXIT`; payload with only `transcript_path`
pointing at a fixture JSONL; `BLOCKED <reason>` extraction; neither sentinel → no
write; missing `RALPHY_FLAG_FILE` → no-op.

### Stage 2 — Settings + charter + exec-model selection (agent, unit-testable)
The deterministic scaffolding `execute()` needs, isolated from the live session.

- Extend `ClaudeAgent` with exec config: `exec_model: Option<String>`,
  `exec_effort: Option<String>`, `default_exec_model: String`,
  `max_minutes_per_issue: u64`, `remote_control: bool`.
- `resolve_exec_model(plan)` — **the single tier→model point (Q5)**:
  explicit `exec_model` > `plan.recommended_model` > `default_exec_model`.
  Returns the literal `sonnet`/`opus` string. Unit-tested.
- `write_exec_settings(run_dir)` — writes `ralphy.settings.json` with the skip
  flags **and** a `Stop` hook whose command invokes *this* binary's
  `hook stop` (`std::env::current_exe()` + `["hook","stop"]`), quoted for the
  platform. **No `PreToolUse` guard yet — that is #4.** Assert the JSON shape in a test.
- Copy `prompt.execute.md` → `.ralphy/exec.md` (embed via `include_str!` like the
  plan prompt, so the binary stays a self-contained global tool).

**Tests:** `resolve_exec_model` precedence; settings JSON contains a Stop hook
pointing at `hook stop` and the skip flags, and contains no `PreToolUse` key.

### Stage 3 — Interactive `execute()` over the PTY (agent, the live core)
The load-bearing path. Reuses the `ralphy-pty` drive loop (and the
`CURSOR_POSITION_*` DSR reply) from #2.

- Add `ralphy-pty` to `ralphy-agent-claude/Cargo.toml`.
- Build the `claude` argv: `--settings <path>`, `--dangerously-skip-permissions`,
  `--model <exec>`, `--effort <e>`, `--remote-control ralphy-<n>` (unless disabled),
  and the initial prompt charter (`Read .ralphy/exec.md … Emit RALPHY_DONE_EXIT …`).
- Spawn via `PtySession::spawn`, set `RALPHY_FLAG_FILE` in the child env (per-issue
  flag under `run_dir`), drain the master on a thread (tee to `run_dir/exec.log`,
  answer DSR), and run the **orchestrator poll loop**:
  - every ~2s: if the flag file exists → read it, kill the tree, classify;
  - if the child exits on its own → check flag, else inspect transcript;
  - per-issue wall timeout (`max_minutes_per_issue`) → kill → `Outcome::Timeout`.
- **Classification** → `Outcome`:
  - flag `DONE` → `Done`; flag `BLOCKED <r>` → `Blocked(r)`;
  - timeout → `Timeout`; usage-limit text in transcript → `Limit`;
  - exited with no flag / no sentinel → `Stuck`.
- **Reclaim:** `PtySession::kill()` + dropping the session (closes the ConPTY).
  *Risk:* whether that reliably kills `claude`'s **child tree** on Windows (Node +
  spawned tools). If not, add a `taskkill /T /F /PID` fallback — see Risks.

**Tests:** a `FakeSession`-style seam isn't worth faking the real CLI; instead
unit-test the *classifier* (flag contents / transcript text → `Outcome`) directly,
and gate the full live run behind a manual/`#[ignore]` integration test (Stage 5).

### Stage 4 — CLI wiring
- Add flags to `RunArgs`: `--exec-model`, `--exec-effort` (default `medium`),
  `--default-exec-model` (default `sonnet`), `--max-minutes-per-issue` (default 45),
  `--remote-control`/`--no-remote-control` (default on).
- Build `ClaudeAgent` with the exec config; clear `ANTHROPIC_API_KEY` before the
  run (guarantee subscription billing, as the ps1 does).
- Print the executed `Outcome` (already wired at [main.rs:106](../../crates/ralphy-cli/src/main.rs#L106)).

### Stage 5 — Verification
- **Automated (no billing):** `cargo test --workspace`, `cargo clippy`. Covers the
  hook classifier, model selection, settings shape, and outcome classification.
- **Live (needs `claude` + a real issue, spends subscription) — your call:** one
  manual `ralphy run --only-issue N` against a scratch repo to confirm AC #1
  (interactive session, no separate console), #2 (`RALPHY_DONE_EXIT` → DONE),
  #4 (timeout reclaim), #5 (commits on the run branch). I'll prep the scratch repo
  and walk through it with you rather than auto-spending quota.

## Acceptance-criteria mapping

| AC | Stage | Verified by |
|---|---|---|
| Interactive over PTY, no separate console, Remote-Control-followable | 3,4 | live run (5) |
| `RALPHY_DONE_EXIT`→DONE / `RALPHY_BLOCKED_EXIT`→BLOCKED+reason | 1,3 | unit (classifier) + live |
| Sentinel read from transcript, never the PTY stream | 1 | unit (hook reads transcript only) |
| Per-issue timeout reclaims a hung session | 3 | unit (timeout→Timeout) + live |
| Agent commits land on the run branch | 3,4 | live run (5) |

## Risks / open questions

1. **Process-tree kill on Windows** — does `PtySession::kill()` + ConPTY close
   take down `claude`'s descendants? If a live run leaves orphans, add a
   `taskkill /T /F` fallback (and possibly a small `kill_tree` helper in
   `ralphy-pty`). Decide after the first live run.
2. **`claude` blocking on DSR like `cmd.exe` did** — the #2 drive loop already
   answers `\x1b[6n`; if `claude` issues other queries (DA, DECRQM) and stalls,
   extend the reply table. Surfaces only in a live run.
3. **`--effort` flag** — present in the ps1; confirm the installed `claude`
   accepts it (the plan path already passes it, so assumed valid).
4. **Sandbox** — ConPTY children need the sandbox off in this environment (learned
   in #2); the live run must be invoked with that in mind.

## Out of scope (later issues)

PreToolUse **guard** hook (#4) · full **queue loop** / close-on-green /
stop-at-first-non-green (#5) · usage-limit **reset-time report** (#6) · staged-plan
routing (#7) · branch modes (#8) · headless `-p` fallback (#9).
