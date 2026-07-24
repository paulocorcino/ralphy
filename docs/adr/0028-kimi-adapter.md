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

This was originally grounded in the installed **`kimi 1.48.0`** CLI
(`kimi-cli`, Python/Typer), probed hands-on (logged-out and logged-in) and read
at the source level for the limit/exit-code behavior. The full evidence —
command surface, stream/session formats, token location, exit-code semantics,
and Windows I/O traps — is in
[docs/research/kimi-cli-adapter-spike.md](../research/kimi-cli-adapter-spike.md);
that spike and [0028-kimi-validation.md](./0028-kimi-validation.md) are now
**historical records of `kimi-cli` 1.48**, superseded by the contract below.

The contract below is **`kimi-code` 0.28** — a different, native-binary CLI —
validated live on **both** Windows and WSL Ubuntu 22.04 in #239 (byte-identical
across targets except the argv ceiling, see the Amendment). The adapter code
was brought onto this contract in #241.

Status: **accepted** — implemented (#151–#154) and validated end-to-end against a
real repo (#155). **Amended 2026-07-20 (#240)** — see the Amendment section for
the `kimi-code` 0.28 rewrite of D4/D5/D6/D7/D9. Consistent with
ADR-0002/0003/0004/0005/0008/0023.

## D1 — Selection is per run, via `--agent kimi`; the core is untouched

`CliAgent` gains a `Kimi` variant and `build_agent` boxes `KimiAgent` as
`Box<dyn Agent>`; `run_queue(…, agent.as_ref(), …)` is unchanged. This is the
same stance ADR-0004 D1 / ADR-0005 D1 settled and is not re-litigated: per-run is
the smallest surface, keeps the choice out of the core, and matches ADR-0002's
"the adapter is the isolated unit." The `--agent` value is pinned to `kimi` (one
word) so clap's derived spelling matches the documented invocation, the same fix
ADR-0005 folded in for `opencode`.

## D2 — Completion is the sentinel for *intent*, with exit code + HEAD-diff as the net

**Historical (`kimi-cli` 1.48) invocation shape, superseded by D5's 0.28 contract:**

```
kimi --work-dir <ws> --print --input-format text --output-format stream-json -y \
     -m kimi-code/kimi-for-coding   < <charter on stdin>
```

driven by the same reader-thread + poll-`try_wait` + kill-on-timeout loop the
Codex adapter uses (`run_headless_logged`). The prompt went in on **stdin**
(`--input-format text`), not as an argv element: `kimi-cli` was a Typer app and a
stray word in a split argv was parsed as a subcommand (`No such command 'hello'`,
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

Kimi Code exposes a single stable model id, pinned as `kimi-code/k3` per D4
(historically `kimi-code/kimi-for-coding`, `display_name` "K2.7 Code"). There is
no sonnet/opus-style tier to route (Claude) and no `model_reasoning_effort`
analog worth wiring (Codex D3). So the Kimi adapter is **deterministic — no auto
complexity routing**, the same stance as OpenCode D8. `Plan.recommended_model` is
left `None`, and the Kimi plan prompt emits **no** `## Execution model` tier line
(D8). Kimi's own `--thinking` is left at its config default; the model already
thinks by default (`default_thinking = true`) — **unverified against 0.28**, not
covered by #239's evidence.

## D4 — Model resolution: pinned to `kimi-code/k3`; the config-key form is the default; `--exec-model` overrides

**Amended 2026-07-20 (#239/#240): the pin moves to `kimi-code/k3`.** 0.28 was
adopted by the vendor *for* Kimi 3, but neither the adapter's hardcoded pin nor
the operator's `config.toml` `default_model` (which still names *K2.7 Coding*)
routed a run there — so the upgrade would have been invisible to Ralphy, which
would have kept driving the previous generation while the operator believed
otherwise. `-m kimi-code/k3` was verified live on both Windows and WSL.

Kimi requires the **full `provider-key/model` form**, e.g. `kimi-code/k3`. The
historical rejection of the short form (`kimi-for-coding` → exit 1, stdout
`LLM not set`) described `kimi-cli` 1.48 (see D6 for the current auth-failure
text); the "full key required" rule itself survives, unverified as literally
re-triggerable against 0.28. The adapter defaults to `kimi-code/k3` and passes
`-m` explicitly (rather than deferring to the config `default_model`) so a run
is reproducible regardless of the operator's `config.toml`; `--exec-model`
overrides it verbatim for an operator who has configured a different
provider/model. Unlike Codex there is no `config.toml` parsing to re-implement —
the single canonical id is hardcoded, with the override as the escape hatch.

## D5 — Void as written; rewritten to the `kimi-code` 0.28 invocation contract (#239/#240)

**The rationale below this line described `kimi-cli` 1.48 (Python/Typer,
Textual TUI, cp1252-redirected-stdout crash) and is gone: `kimi-code` is a
native binary, so its historical Python-interpreter env-var workarounds are
inert and there is no
Typer subcommand parser, no Textual TUI, and no cp1252 charmap crash to guard
against.** The validated 0.28 contract, live on both Windows and WSL:

```
kimi -p <charter> --output-format stream-json -m <model> --skills-dir <dir>
```

- **The charter travels in `-p, --prompt <prompt>`, as an argv element — there
  is no stdin channel any more.** `echo … | kimi --output-format stream-json`
  answers `error: Output format is only supported in prompt mode.` This
  reverses D2's historical stdin rule; see the Amendment for the argv-ceiling
  consequence and the file-pointer decision this forces on the execute
  charter.
- **`--work-dir` is gone.** The working directory is the process cwd
  (`Command::current_dir`), with no flag to set it explicitly.
- **`-y`/`--yolo` and `--auto` are refused, not merely unnecessary**:
  `error: Cannot combine --prompt with --yolo.` (same for `--auto`). Prompt
  mode already auto-approves every action — the session wire records
  `{"type":"permission.set_mode","mode":"auto"}` — so the adapter passes
  neither flag; there is nothing left to grant.
- **`--output-format stream-json`, `-m` and `--skills-dir` are unchanged** in
  name and shape from the historical contract.

There is still no OS sandbox and no `PreToolUse` guard to port; safety rests on
Ralphy's existing net (every issue commits onto an isolated run branch a human
merges by hand, plus the reviewer self-review) — the same rationale as OpenCode
D5, carried over unchanged from the 1.48 decision.

The PTY, the Stop hook + flag file, the workspace-trust shim, and Codex's `-o`
final-message file are **not** ported — none apply to `kimi --prompt`, and
importing them would be compatibility-shaped bloat.

## D6 — Auth is the operator's; detection is behavioral (`auth.login_required`, #239/#240)

**The `LLM not set` line below is `kimi-cli` 1.48's text and no longer
appears.** Kimi Code auth is OAuth, owned by the operator via **`kimi login`**
(a real subcommand, not just a TUI `/login`); the historical token location
(`~/.kimi/credentials/kimi-code.json`, **not** `config.toml`) is
**unverified against 0.28** — #239's evidence never re-probed the on-disk
credential path, only the CLI's own error text, so this ADR does not guess a
`~/.kimi-code/credentials/...` location. The adapter manages no provider key —
the same stance as Codex D5 / OpenCode D6 — and there is no
`ANTHROPIC_API_KEY`/`OPENAI_API_KEY` auto-detect hazard to scrub (Kimi resolves
only its own OAuth), so **no env-key scrub is needed**.

A signed-out run surfaces as, verbatim, captured live on Windows (#239's
cross-platform WSL re-run confirmed the four *refusal* strings word-for-word —
`--work-dir`, `--yolo`, `--auto`, prompt-mode-only — but did not separately
re-capture this auth line on WSL):

```
error: failed to run prompt: auth.login_required:
OAuth provider "managed:kimi-code" requires login before it can be used.
```

This replaces the historical `exit 1` + `LLM not set` pair `is_kimi_auth_error`
keyed on — **that guard is dead against 0.28** and needs rewriting to key on
`auth.login_required` instead, mapping it to the same actionable "run
`kimi login` and retry" stop, taking precedence over generic classification
because it won't self-heal — the same precedence the other adapters' auth
detectors use (ADR-0013). Detection stays **behavioral** rather than inspecting
the credentials file, which is simpler and matches the other adapters. The guard
landed on this signal in #241; before that a logged-out run fell through as a
generic `kimi produced no plan` / `Stuck`, losing the actionable message.
Historical
caveat, no longer applicable: `LLM not set` meant "no model resolved"; 0.28's
`auth.login_required` line is unambiguous about the cause.

**#281 — the full-logout state is a second logged-out shape.** The
`auth.login_required` line above is only one of two logged-out signatures, and it
is the *narrower* one: an expired/invalid token with the model catalog still
intact. The #274 capstone (Phase 0) captured the other live on Windows
(`kimi-code` 0.28.0): a full `kimi logout` **strips the login-populated catalog**
(`default_model` and every `[models.*]` entry) from `config.toml`, and the run
then fails with no `auth.login_required` token at all —

- on the adapter's real path, where `-m kimi-code/k3` is pinned
  (`command.rs`): `config.invalid: Model "kimi-code/k3" is not configured in
  config.toml…`;
- on a bare invocation (no `-m`): `No model configured. Run \`kimi\` and use
  /login to sign in, then retry…`.

`is_kimi_auth_error` now matches all three via the shared multi-group
`auth_error` helper (Codex D5's precedent), keeping detection behavioral: groups
`["auth.login_required"]`, `["no model configured", "login"]`, and
`["config.invalid", "is not configured"]`.

This **supersedes** the earlier boundary note that a `No model configured…`
output was deliberately *not* claimed as logged-out (read then as "never
configured"). Two things changed the call: the full-logout state is a real,
now-observed logged-out shape that leaves `config.toml` present, and its message
carries the CLI's own `/login` hint — matched with an AND-guard
(`no model configured` **and** `login`) so it keys on the actionable signal, not
the bare "no model" prose. **The `config.invalid` group carries an accepted
conflation risk:** a genuine operator model-config typo on `kimi-code/k3` reads
as "run `kimi login`" too. Taken on purpose — login populates the catalog and the
adapter always pins the managed `kimi-code/k3`, so "model not configured" almost
always *means* logged-out, and re-login is the correct first action either way. A
tighter model-specific match was rejected as brittle against a future pinned-id
change.

## D7 — Tokens come from `wire.jsonl` `usage.record`, per step, snapshot-diffed (#239/#240)

Per ADR-0008 (per-adapter token harvest, tokens-as-truth). Kimi still does
**not** put usage on the stdout stream — that half of D7 survives. The record
vocabulary itself does not: **the `StatusUpdate`/`message.payload.token_usage`
shape below is `kimi-cli` 1.48's and is gone.** 0.28 writes a top-level,
dotted-lowercase, camelCase record — no `message` envelope:

```json
{"type":"usage.record","model":"kimi-code/k3","usage":{"inputOther":…,"output":…,"inputCacheRead":…,"inputCacheCreation":…},"usageScope":"turn"}
```

Two properties survive from the 1.48 contract and are **traps** an implementer
must preserve explicitly:

- **Records are per-step increments, not a cumulative snapshot** — validated
  live, two steps of one session recorded `3411/91` then `211/20` (Windows) and
  `2154/72` then `202/26` (WSL). D7's summing-across-the-run rule stands
  unchanged.
- **`context.append_loop_event` repeats the same numbers under `event.usage`.**
  Folding both double-counts a step. Only a top-level `usage.record` line with
  `usageScope == "turn"` may be counted; `event.usage` must be skipped.

Store layout moved one level deeper and to a different base dir:
`~/.kimi-code/sessions/wd_<repo>_<hash>/session_<uuid>/agents/<AGENT>/wire.jsonl`
(historically `~/.kimi/sessions/<workdir-hash>/<session-id>/wire.jsonl`). The
historical session-id recovery path (stderr resume hint, `~/.kimi/kimi.json`)
is **unverified against 0.28** — #239 observed a `{"role":"meta",
"type":"session.resume_hint","session_id":…}` line on 0.28's own stdout, which
would be authoritative rather than positional if adopted (see the Amendment,
decision 3).

The adapter still **snapshot-diffs `wire.jsonl`** (the "appeared-over-grew"
rule, ADR-0008 D10, via `session_files`) and sums usage across the run, mapping
the four fields to `Usage`: `inputOther→input`, `output→output`,
`inputCacheRead→cache_read`, `inputCacheCreation→cache_creation`, with model
attribution `kimi-code/k3` (ADR-0008 D8, D4). We reject reading usage from the
stdout stream (it isn't there) and `kimi export` (a ZIP, heavier than tailing
one file).

**`ralphy-usage-scan` (ADR-0033) needs no fix.** It already reads
`usage.record`, `usageScope == "turn"`, camelCase fields, and the
`agents/<AGENT>/` layout, and sums via `agg.add` — it was written ahead of a
real 0.28 sample and matches field for field now that one exists. Only the
adapter's in-run capture (this D7, `ralphy-agent-kimi`) was left behind; the
scan path has no regression.

## D8 — Reuse the skill content; a `prompt.plan.kimi.md` variant; not native `--plan`

`PROMPT_EXECUTE` is reused verbatim. Planning gets a new
`assets/prompts/prompt.plan.kimi.md` — a variant that emits **no**
`## Execution model` tier line (routing is dropped, D3). As with the other
adapters, `plan()` runs a normal headless `--print` with a plan charter that
instructs the model to **write `.ralphy/plan.md` itself**; plan success is the
file appearing on disk (`plan::count_open_steps` is vendor-neutral and reused).

Kimi's **native `--plan` mode is deliberately not used**: it explores heavily and
is slow (>120s), signals completion via an `ExitPlanMode` tool call, and
persisted to `~/.kimi/plans/` (historical, 1.48-only; not re-verified against
0.28 and moot either way since native `--plan` stays unused) — none of which
fits Ralphy's "write `.ralphy/plan.md`" contract. Skills are materialized the
Codex way (embedded `reviewer` +
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

**Unvalidated against 0.28 (#239/#240).** Neither the exit-75 sentinel nor
`is_kimi_limit_text`'s `access_terminated_error` match could be exercised: an
exhausted billing-cycle quota cannot be forced on demand, on either CLI. Given
that every other error string and exit-code path this ADR documented for 1.48
turned out to have changed on 0.28 (D5/D6/D7), **both are suspect and should be
treated as unverified, not settled**, until a live limit is actually observed
against `kimi-code` 0.28. Whoever hits one first should capture the exit code
and the literal stdout/stderr text and fold it back into this decision.

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
  install` location — `kimi` was not on PATH in a fresh shell). **Unverified
  against 0.28** (#239/#240): `kimi-code` ships as a native binary, not a `uv
  tool install`, so whether `~/.local/bin` is still where it lands (vs. a
  platform-native install path) was not re-probed in #239 — #239's hosts
  already had `kimi` resolvable.
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
- **Historical, `kimi-cli` 1.48-only.** A Windows-only defect surfaced live and
  was fixed (#155, [validation](./0028-kimi-validation.md)): kimi crashed with a
  cp1252 `'charmap' codec can't encode` error, exit 1, when it captured
  **tool-subprocess** output carrying a non-cp1252 char (e.g. Prisma's `✔`) — a
  path the historical D5's forced `stream-json` did not cover. Setting the
  Python interpreter's UTF-8 mode on the child (alongside stripping its I/O
  encoding override) fixed it without re-triggering the Textual-TUI trap.
  `kimi-code` 0.28 is a native
  binary with no Python runtime, so this whole defect class — and its fix — is
  inert against it (D5).

## Amendment (2026-07-20) — kimi-code 0.28: the argv ceiling, the charter pointer, and the supported baseline (#239/#240)

Validated live in #239 on both Windows and WSL Ubuntu 22.04, byte-identical
except where noted. This amendment lands **before** the code change (#240
first, the adapter fix follows), so the implementer reads a true contract
instead of discovering the drift mid-fix. It rewrote D4/D5/D6/D7/D9 above in
place; the three decisions below are new, not a rewrite of an existing one.

**(a) The argv ceiling, measured, not quoted from documentation.** D5 moved the
charter from stdin to argv (`-p, --prompt`), which turns the process's
single-argument limit into a real constraint:

| platform | single-arg ceiling | measured |
|---|---|---|
| Windows (`CreateProcess` `lpCommandLine`) | 32,767 chars | passes at 32,000, `WinError 206` at 33,000 |
| Linux (`MAX_ARG_STRLEN`, 32 pages) | 131,072 bytes | passes at 131,000, `E2BIG` at 131,072 |

These are host-measured ceilings from the #239 probe, not vendor-documented
constants — treat them as measured-on-that-host, not a portable guarantee.

**(b) Decision: the execute charter rides a `.ralphy/exec.md` file-pointer, not
argv.** `prompt.execute.md` is 23,698 chars today (measured in #239; 23,884
bytes on disk in this working tree — the gap is non-ASCII characters encoding
to multiple UTF-8 bytes, not a line-ending difference); escaped for Windows (46
embedded `"`, no backslashes) it is ~23,746,
leaving ~8.8 KB of slack once the program path and the other flags are
counted — it fits *today*. But it is a source file that can be edited past the
Windows ceiling **invisibly on Linux and fatally on Windows**, since Linux's
ceiling is 4x larger and CI running on Linux would never catch a Windows-only
regression. Rather than ship the charter in argv with a regression test
pinning it under a Windows-safe budget, the adapter passes a **file-pointer**:
write the charter to `.ralphy/exec.md` and pass a one-line pointer in `-p`,
mirroring the plan path's existing `PLAN_CHARTER`/`.ralphy/plan-charter.md`
mechanism. This is not new machinery, it is the pattern this adapter already
uses on the plan side, applied to the side that just lost its stdin channel.

**(c) Decision: `kimi-code` 0.28+ is the only supported baseline.** The legacy
`~/.kimi` session store is dropped — the adapter only ever reads sessions it
just created (D7), so straddling both the 1.48 and 0.28 stores buys nothing,
unlike `ralphy-usage-scan` (ADR-0033) which straddles because it reads
historical data written by whichever CLI generated it. One data point backing
this: the validated Linux host in #239 has **no `~/.kimi` at all** — it is a
clean `kimi-code` install that never ran `kimi-cli`, so any host provisioned
from here on simply will not have the legacy store to fall back to.

**(d) Ordering.** This document landed before the adapter code change. While
it was outstanding, `ralphy-agent-kimi` emitted the 1.48 invocation and failed
on `kimi-code` 0.28's first flag (`error: unknown option '--work-dir'`) — the
adapter was broken against the CLI version operators actually had installed.
**#241 closed that gap**: the argv, the auth signal, the `usage.record` token
capture, the `session.resume_hint` session id and the `ralphy init` login probe
all now target 0.28. This amendment exists so that fix was implemented against
a true contract rather than rediscovering #239's findings from scratch.
