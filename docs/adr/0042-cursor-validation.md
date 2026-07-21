# Cursor adapter — live end-to-end validation plan

Companion to [ADR-0042](./0042-cursor-adapter.md). Where the Kimi and OpenCode
validation notes ([0028](./0028-kimi-validation.md), [0005](./0005-opencode-validation.md))
record what *was* run, this file is written **before** the adapter exists: it is
the plan the capstone run must execute, and it becomes the note once it has.

It exists because one decision was deliberately left provisional. **D11 (usage)
cannot be settled from a spike.** The spike proved no local store carries
tokens; whether capturing `result.usage` from the stream is *sufficient* — across
a resumed session, a run that hits its budget, and a run the operator later
inspects with `ralphy usage` — is a question only a real run against a real
repository answers. Phase 3 is that question.

Status: **plan — not executed.** Implementation is not authorized; this file is
the acceptance contract that authorization will be measured against.

## Environment the run must have

- Cursor Agent CLI, **both** builds: `2026.07.16-899851b` (Windows,
  `%LOCALAPPDATA%\cursor-agent\agent.cmd`) and `2026.07.17-3e2a980` (WSL,
  `~/.local/bin/cursor-agent`) — **off `PATH` on both**, so the run is also the
  proof of ADR-0042 D14's probe list.
- Auth: `agent login` (browser OAuth), credential at `%APPDATA%\Cursor\auth.json`.
  Tier recorded from `about --format json` at the start of every phase — the
  entitlement of D4 is tier-dependent and the note must say which tier it saw.
- Model: `--model auto` passed **explicitly** (D4). `~/.cursor/cli-config.json`
  is captured before and after every phase; a diff in the four model keys is a
  finding, not noise.
- Target repo: `C:\Dev\FinCal` (`paulocorcino/FinCal`), the same lab every other
  vendor was validated against. Run branches cut as `afk/run-*`.
- **`.cursorindexingignore` present in the working tree before the first run**,
  and `~/.cursor/projects/<slug>/worker.log` inspected after every phase. A
  non-zero `Applying change` count anywhere in this validation **fails the whole
  note** — D6 is the decision this repository's contents pay for.

## Phase 0 — the preflight gate refuses (D6, D8)

Before anything green, prove the two refusals fire:

1. Remove `.cursorindexingignore`, run `ralphy run --agent cursor --dry-run`.
   Expect an ADR-0013 stop naming the file, its one-line content, and what it
   prevents. **No child process is spawned.**
2. Restore the file, log out of Cursor, run again. Expect the auth stop quoting
   `agent login`, driven by `status --format json` → `isAuthenticated: false`
   **with exit code 0** — the trap D8 exists for.
3. Set `CURSOR_API_KEY` to garbage and run. Expect the *third* auth string
   (`The provided API key is invalid`) to be classified as an auth failure, not
   a generic one.

A phase that cannot produce all three refusals means the gate is decorative.

## Phase 1 — plan-only dry run

```
ralphy run --repo C:/Dev/FinCal --only-issue <n> --agent cursor \
  --base-branch <base> --dry-run --verbose
```

Acceptance:

- `.ralphy/plan.md` written **by the agent**, in execution mode — not
  `--mode plan` (D9). The plan has open steps, a feasibility verdict, an
  acceptance ledger and `## Verify` commands.
- The minted `create-chat` id equals `system/init.session_id` (D10). A mismatch
  is a hard error, not a warning.
- The run prices out — no "unknown model" — through the family normalization of
  D5. Record which family the `auto` route actually chose, recovered from the
  store blob (`providerOptions.cursor.modelName`), and confirm the price table
  resolved it.
- Repo returned to the base branch; the empty run branch removed.

## Phase 2 — full non-dry-run

Same invocation without `--dry-run`, on an issue that requires real edits **and**
at least one shell-driven change.

Acceptance:

- `DONE_SENTINEL` is the last line of `result.result`, and `result.is_error` is
  `false` (D3).
- The classification ladder (ADR-0023) behaves: commits without a sentinel must
  **not** buy a green close — the Kimi precedent.
- **The progress asymmetry is measured, not assumed.** D3/§C2 says
  `editToolCall` reports `linesAdded`/`diffString` and `shellToolCall` reports
  nothing about files. Compare the stream's accounting against `git diff HEAD`
  and record the delta. If Ralphy surfaces the stream's number anywhere an
  operator reads it, that number is wrong by exactly the shell-driven work.
- A deliberate kill mid-run: confirm the **absence** of the `result` envelope is
  classified as failure, per Cursor's documented "stream may end early" contract.

## Phase 3 — the usage question (D11, the provisional decision)

This is the phase the plan exists for. Four measurements, in order:

1. **Single run.** Capture `result.usage` from the envelope. Compare against the
   store: confirm — again, on a real workload — that
   `~/.cursor/chats/<hash>/<sid>/store.db` and the `agent-transcripts` JSONL
   contain no token count. If a future CLI build has added one, D11 is rewritten
   rather than worked around.
2. **Resumed session.** Run, then `--resume` the same chat for a second turn.
   Does the second envelope report that turn's tokens, or the session's running
   total? The spike had exactly one `result` per run and could not tell. **Get
   this backwards and the bill is silently multiplied or divided** (ADR-0040
   C6); write the test so the wrong choice fails.
3. **`ralphy usage` after an interactive session.** Run `agent` interactively by
   hand, then `ralphy usage`. Expect `scan_cursor` to enumerate the session and
   report tokens as **unavailable** — an explicit gap, never a zero and never an
   invented number. A zero is a bug; the absence of the session is a worse bug.
4. **The unit mismatch, stated.** Record the run's token counts alongside what
   Cursor's dashboard says it cost in credits. The note must state plainly that
   Ralphy's tokens are not Cursor's bill, with both numbers from the same run.

If measurement 2 shows cumulative envelopes, D11 gains a keep-last rule (the
Codex shape). If it shows incremental, D11 gains a sum. If `--resume` turns out
to report neither coherently, D11 becomes "usage is per-invocation only" and the
adapter must not resume a session mid-issue.

## Phase 4 — the token cost of the foreign harvest (D12)

The spike measured a trivial "reply OK" run at **18 212 input tokens**, almost
all of it 78 harvested skills from other vendors' directories. On a real charter
this is a fixed tax on every call.

- Record `inputTokens` for a plan pass and an execute pass, and estimate the
  harvest's share by counting skills in the request blob.
- Compare against the same issue driven by another vendor, and state the
  multiple.
- Feed the result into ADR-0038: **a per-issue budget tuned on another vendor
  will read wrong for Cursor.** If the multiple is large, this validation
  produces a recommended Cursor-specific default rather than leaving the
  operator to discover it.

## Phase 5 — cross-platform parity

Repeat Phase 1 on WSL against the same issue. The two installs differ by a
version, which is itself the point: the note records whether a version skew
changed the stream shape, the envelope fields, or the auth strings.

## What fails this validation outright

- Any `Applying change` line in a `worker.log` during any phase.
- A `.cursorignore` written by Ralphy, ever (D6 — it breaks the edit tool and the
  agent routes around it via the shell).
- `~/.cursor/cli-config.json` differing before and after a run in the four model
  keys (D4's write-back reaching the operator's state through Ralphy).
- `ralphy usage` reporting a token number for an interactive Cursor session.
  There is no source for one; a number there is fabricated.
