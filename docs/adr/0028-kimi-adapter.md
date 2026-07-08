# The Kimi adapter: a per-run peer of Claude/Codex/OpenCode, native to `kimi --print`

Ralphy gains a fourth agent CLI vendor, `kimi` (Kimi Code CLI, moonshotai), as a
new isolated crate `ralphy-agent-kimi` that implements the same PTY-free `Agent`
trait ([ADR-0002](./0002-core-agnostic-adapter-boundary.md)). It is selected
**per run** by `--agent kimi`; the core keeps taking a single `&dyn Agent` and
never learns which vendor it holds ([ADR-0004](./0004-codex-adapter.md) D1). The
only shared surface is the core's `Agent` trait, `Outcome` enum, and the shared
`classify` ladder ([ADR-0023](./0023-shared-outcome-classifier.md)) — there is no
shared "headless runner" the vendors bend to fit. The adapter is built to Kimi's
best-fit mechanism; the only thing that must match is the `Outcome` the core
receives, not how it was produced.

Kimi's headless surface is close to Codex's, so **the Codex adapter is the
template** — headless-only, single child per phase via `run_headless_logged`,
sentinel-in-final-message + exit code + HEAD-diff, shared scaffold
(`run_plan_session`/`run_exec_session`) and `classify`. Where Kimi differs
(deterministic single model, token store, limit-by-exit-code, Windows I/O), the
adapter diverges deliberately.

This is grounded in the installed **`kimi 1.48.0`** CLI, probed hands-on
(logged-out and logged-in) and read at the source level for the limit/exit-code
behavior. The full evidence — command surface, stream/session formats, token
location, exit-code semantics, and Windows I/O traps — is in
[docs/research/kimi-cli-adapter-spike.md](../research/kimi-cli-adapter-spike.md);
this ADR records the decisions, the spike records the observations.

Status: **accepted** — implemented (#151–#154) and validated end-to-end against a
real repo (#155); see [0028-kimi-validation.md](./0028-kimi-validation.md) for the
live findings. Amends nothing; consistent with ADR-0002/0003/0004/0005/0008/0023.

## D1 — Selection is per run, via `--agent kimi`; the core is untouched

`CliAgent` gains a `Kimi` variant and `build_agent` boxes `KimiAgent` as
`Box<dyn Agent>`; `run_queue(…, agent.as_ref(), …)` is unchanged. This is the
same stance ADR-0004 D1 / ADR-0005 D1 settled and is not re-litigated: per-run is
the smallest surface, keeps the choice out of the core, and matches ADR-0002's
"the adapter is the isolated unit." The `--agent` value is pinned to `kimi` (one
word) so clap's derived spelling matches the documented invocation, the same fix
ADR-0005 folded in for `opencode`.

## D2 — Completion is the sentinel for *intent*, with exit code + HEAD-diff as the net

Both `plan` and `execute` run headless:

```
kimi --work-dir <ws> --print --input-format text --output-format stream-json -y \
     -m kimi-code/kimi-for-coding   < <charter on stdin>
```

driven by the same reader-thread + poll-`try_wait` + kill-on-timeout loop the
Codex adapter uses (`run_headless_logged`). The prompt goes in on **stdin**
(`--input-format text`), not as an argv element: Kimi is a Typer app and a stray
word in a split argv is parsed as a subcommand (`No such command 'hello'`,
exit 2). `--output-format stream-json` is **mandatory** (see D5).

The stdout stream is coarse OpenAI-role JSONL discriminated by top-level `role`
(`assistant` with `content[]` parts `think`/`text` and optional `tool_calls`;
`tool` results). The **final assistant text** is the last `role:"assistant"` line
carrying a `text` part and **no `tool_calls`** — that absence is the "turn
finished" marker; the stream then simply ends with **no explicit done/usage
envelope**. So a native "did it finish" signal is weak, and — as with Codex — a
clean finish and a deliberate "I'm blocked, giving up" are indistinguishable from
the stream alone. The **sentinel is the source of intent**; exit code + HEAD-diff
are the net:

- `RALPHY_BLOCKED_EXIT <reason>` in the final assistant text → `Blocked(reason)`.
- exit 0 **and** a HEAD-diff commit **and** `RALPHY_DONE_EXIT` → `Done`.
- **exit 75** (D9) → `Limit`.
- the per-issue wall timeout → `Timeout`.
- anything else — non-zero non-75 exit, no new commit, or no sentinel → `Stuck`.

`PROMPT_EXECUTE` is reused **verbatim** (it already names both sentinels and is
not Claude-specific). The custom sentinel path is validated: asked to print
`RALPHY_DONE` on the last line, Kimi's final assistant `text` ended with exactly
`RALPHY_DONE`. The HEAD-diff `committed` guard is load-bearing (a `Done` claim
with no new commit is downgraded to `Stuck`, the same progress guard the other
adapters use). Camada-1 signal extraction lives in `classify_kimi_outcome`; it
fills `CompletionSignals` and delegates the precedence ladder to the shared
`classify` (ADR-0023).

## D3 — Deterministic: one model, no complexity routing

Kimi Code exposes a single stable model id, `kimi-code/kimi-for-coding` (backend
remaps it to newer models; `display_name` "K2.7 Code"). There is no
sonnet/opus-style tier to route (Claude) and no `model_reasoning_effort` analog
worth wiring (Codex D3). So the Kimi adapter is **deterministic — no auto
complexity routing**, the same stance as OpenCode D8. `Plan.recommended_model` is
left `None`, and the Kimi plan prompt emits **no** `## Execution model` tier line
(D8). Kimi's own `--thinking` is left at its config default; the model already
thinks by default (`default_thinking = true`).

## D4 — Model resolution: the full config-key form is the default; `--exec-model` overrides

Kimi requires the **full `provider-key/model` form** `kimi-code/kimi-for-coding`.
The short form `kimi-for-coding` is **rejected** (exit 1, stdout `LLM not set`),
so the adapter must pass the full key. The adapter defaults to
`kimi-code/kimi-for-coding` and passes `-m` explicitly (rather than deferring to
the config `default_model`) so a run is reproducible regardless of the operator's
`~/.kimi/config.toml`; `--exec-model` overrides it verbatim for an operator who
has configured a different provider/model. Unlike Codex there is no `config.toml`
parsing to re-implement — the single canonical id is hardcoded, with the override
as the escape hatch.

## D5 — Full autonomy via `-y`; force `stream-json` + stdin (the Windows I/O contract)

The adapter always passes `-y` (`--yolo`, auto-approve all actions); `--print`
already auto-dismisses `AskUserQuestion` and auto-approves tools for the run.
There is no OS sandbox and no `PreToolUse` guard to port; safety rests on Ralphy's
existing net (every issue commits onto an isolated run branch a human merges by
hand, plus the reviewer self-review) — the same rationale as OpenCode D5.

Two Windows-specific hazards, observed in the spike, are baked in as hard rules
(this is the compatibility-shaped code the design *does* need, because it is
correctness, not cosmetics):

- **Always `--output-format stream-json`.** The default rich/TUI renderer writes
  box-drawing/emoji and **crashes on a cp1252-redirected stdout**
  (`'charmap' codec can't encode…`, exit 1). stream-json is ASCII-safe.
- **Never set `PYTHONIOENCODING=utf-8`** on a redirected/no-console child — it
  flips Kimi into trying to start the Textual TUI (`No Windows console found`).
  **Do set `PYTHONUTF8=1`** instead (resolved at validation, #155): it puts Kimi's
  stdio on UTF-8 so captured **tool-subprocess** output with a non-cp1252 char
  (e.g. Prisma's `✔`) can't crash it with `'charmap' codec can't encode` (exit 1),
  and — touching encoding, not console detection — it does not re-trigger the TUI.
- **Prompt via stdin**, never a split argv (D2).

The PTY, the Stop hook + flag file, the workspace-trust shim, and Codex's `-o`
final-message file are **not** ported — none apply to `kimi --print`, and
importing them would be compatibility-shaped bloat.

## D6 — Auth is the operator's; detection is behavioral (exit 1 + `LLM not set`)

Kimi Code auth is OAuth, owned by the operator via **`kimi login`** (a real
subcommand, not just a TUI `/login`); the token lives in
`~/.kimi/credentials/kimi-code.json` (`access_token`/`refresh_token`/`expires_at`),
**not** in `config.toml` (whose `api_key` stays `""`). The adapter manages no
provider key — the same stance as Codex D5 / OpenCode D6 — and there is no
`ANTHROPIC_API_KEY`/`OPENAI_API_KEY` auto-detect hazard to scrub (Kimi resolves
only its own OAuth), so **no env-key scrub is needed**.

A signed-out / no-model run surfaces as **exit 1 with `LLM not set` on stdout**.
`is_kimi_auth_error` keys on that pair and maps it to an actionable "run
`kimi login` and retry" stop, taking precedence over generic classification
because it won't self-heal — the same precedence the other adapters' auth
detectors use (ADR-0013). Detection stays **behavioral** rather than inspecting
the credentials file, which is simpler and matches the other adapters. Caveat:
`LLM not set` literally means "no model resolved"; because the adapter always
passes a valid `-m` (D4), post-login it should only appear on a genuine auth gap.

## D7 — Tokens come from `wire.jsonl` `StatusUpdate`, per step, snapshot-diffed

Per ADR-0008 (per-adapter token harvest, tokens-as-truth). Kimi does **not** put
usage on the stdout stream. It writes per-step `StatusUpdate` events into the
session's `wire.jsonl`:

```
message.payload.token_usage = { input_other, output, input_cache_read, input_cache_creation }
```

one `StatusUpdate` per LLM call, between `TurnBegin`/`TurnEnd`. Sessions live at
`~/.kimi/sessions/<workdir-hash>/<session-id>/wire.jsonl`; the session id is
recoverable from the **stderr resume hint** (`To resume this session: kimi -r
<id>`) and from `~/.kimi/kimi.json` → `work_dirs[].last_session_id`.

The adapter **snapshot-diffs `wire.jsonl`** (the "appeared-over-grew" rule,
ADR-0008 D10, via `session_files`) and sums `token_usage` across the run, mapping
the four fields to `Usage`: `input_other→input`, `output→output`,
`input_cache_read→cache_read`, `input_cache_creation→cache_creation`, with model
attribution `kimi-code/kimi-for-coding` (ADR-0008 D8). We reject reading usage
from the stdout stream (it isn't there) and `kimi export` (a ZIP, heavier than
tailing one file).

## D8 — Reuse the skill content; a `prompt.plan.kimi.md` variant; not native `--plan`

`PROMPT_EXECUTE` is reused verbatim. Planning gets a new
`assets/prompts/prompt.plan.kimi.md` — a variant that emits **no**
`## Execution model` tier line (routing is dropped, D3). As with the other
adapters, `plan()` runs a normal headless `--print` with a plan charter that
instructs the model to **write `.ralphy/plan.md` itself**; plan success is the
file appearing on disk (`plan::count_open_steps` is vendor-neutral and reused).

Kimi's **native `--plan` mode is deliberately not used**: it explores heavily and
is slow (>120s), signals completion via an `ExitPlanMode` tool call, and persists
to `~/.kimi/plans/` — none of which fits Ralphy's "write `.ralphy/plan.md`"
contract. Skills are materialized the Codex way (embedded `reviewer` +
`staged-plan` content) and pointed at with `--skills-dir` (a real repeatable Kimi
flag) under `.ralphy/`, keeping a stray `.agents/`/`.kimi/` dir out of the target
repo; the exact materialization path is settled at implementation time against
the Codex helper it clones.

## D9 — A usage limit is caught by exit code 75, not a text matcher

This is where Kimi is *cleaner* than Codex/OpenCode, and it is source-grounded,
not assumed. Kimi's `--print` mode defines semantic exit codes
(`kimi_cli/cli/__init__.py`): `SUCCESS = 0`, `FAILURE = 1`,
**`RETRYABLE = 75`** (`EX_TEMPFAIL`). `_classify_provider_error`
(`ui/print/__init__.py`) maps **429 and 5xx/timeout → exit 75**, everything else
permanent → exit 1, printing the provider error text to stdout first. (Kimi
already retried internally with `kosong` backoff before giving up; exit 75 is the
post-give-up signal.)

So the adapter treats **exit 75 as `Outcome::Limit(None)`** — a structured signal,
no fragile text scraping (contrast Codex D6's text matcher and OpenCode D9's
timeout backstop). There is **no structured reset/Retry-After at the chat level**
(`retry_after` exists only for OAuth refresh), so `None` is correct: the default
wait/resume (ADR-0003) with no schedulable reset. Because a limit carries no
reset hint and auto-resuming into a still-open rolling window risks a tight
re-limit loop, **`--stop-on-limit` is forced for Kimi** (extend
`effective_stop_on_limit` as it is forced for OpenCode, D9) — auto-resume is
pointless without a reset to wait on. An **optional** later upgrade — scan the
printed 429 text for a timestamp and, if present, emit `Outcome::Limit(Some(reset))`
and drop the forced stop — mirrors PR #145 but is **not required** for a correct
v1. We reject treating a limit as plain `Stuck` (loses the actionable re-run
signal).

## Consequences

- The core, `ralphy-agent-claude`, `ralphy-agent-codex`, `ralphy-agent-opencode`,
  the existing prompts/plugin, `hook.rs`, `guard.rs`, and the `ANTHROPIC_API_KEY`
  clearing all stay **untouched**. No-regression for the existing vendors is
  structural, not tested-in. The `ralphy-core` `pub` surface is unchanged.
- Wiring is the hand-maintained registry the other adapters already established:
  the `CliAgent` enum + `cli_name` (`cli.rs`), the `init::gate::Agent` enum + its
  `ALL`/`cli_name`/`accepts_images` (`init/gate.rs`), `build_agent` (`wiring.rs`),
  the three one-shot dispatch matches (`init/run.rs`, `init/issues.rs`,
  `triage.rs`), `models.rs` (`agent_slug`/`plan_action`), the workspace + CLI
  Cargo deps, and forcing `effective_stop_on_limit` for Kimi (D9). Plus the new
  `assets/prompts/prompt.plan.kimi.md`.
- Reused unchanged from `ralphy-adapter-support`: `run_plan_session`,
  `run_exec_session`, `run_headless_logged`, `classify`, `session_files`,
  `IssueBudget`, `resolve_program`. The binary is resolved via
  `resolve_program("kimi")`, which must also probe `~/.local/bin` (the `uv tool
  install` location — `kimi` was not on PATH in a fresh shell).
- `ACCEPTS_IMAGES`: **resolved to `false`** (#155). The model advertises
  `image_in`/`video_in`, but `kimi --print` exposes no image/attachment flag — the
  only input is a text/`stream-json` charter on stdin — so there is no verified
  multimodal delivery path. Revisit only if Kimi ships a `--print` image channel.
- Not ported, by design: the PTY, the Stop hook + flag file, `guard.rs`'s
  `PreToolUse` guard, the workspace-trust shim, Codex's `-o` file, native
  `--plan` mode, and complexity routing — none apply to `kimi --print`.
- Deferred-until-implementation, all resolved at validation (#155) except the
  optional 429-parser: the `--skills-dir` layout settled as a gitignored
  `.ralphy/skills` container (D8, confirmed loading the reviewer live); the
  `ACCEPTS_IMAGES` multimodal path resolved to `false` (no `--print` image channel);
  and the optional 429-text reset-hint parser (D9) still deferred. The one item that
  could not be forced without burning real quota — a live 429 — was resolved by
  source instead (D9, exit 75); if a real limit later shows a parseable timestamp,
  the optional upgrade applies.
- A Windows-only defect surfaced live and was fixed (#155,
  [validation](./0028-kimi-validation.md)): kimi crashes with a cp1252 `'charmap'
  codec can't encode` error, exit 1, when it captures **tool-subprocess** output
  carrying a non-cp1252 char (e.g. Prisma's `✔`) — a path D5's forced `stream-json`
  does not cover. Setting `PYTHONUTF8=1` on the child (alongside the existing
  `PYTHONIOENCODING` strip) fixes it without re-triggering the D5 Textual-TUI trap.
