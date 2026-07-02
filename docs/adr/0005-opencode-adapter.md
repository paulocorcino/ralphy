# The OpenCode adapter: a per-run peer of Claude/Codex, native to `opencode run`

Ralphy gains a third — and final — agent CLI vendor, `opencode` (opencode.ai),
as a new isolated crate `ralphy-agent-opencode` that implements the same PTY-free
`Agent` trait (docs/adr/0002). It is selected **per run** by `--agent opencode`;
the core keeps taking a single `&dyn Agent` and never learns which vendor it holds
(docs/adr/0004 D1). As with Codex, the only shared surface is the core's `Agent`
trait and `Outcome` enum — there is no shared "headless runner" the two bend to
fit. The adapter is built to OpenCode's best-fit mechanism even where that makes
it internally divergent, because the only thing that must match is the `Outcome`
the core receives, not how it was produced.

This is grounded in the installed `opencode 1.16.2` CLI, against which the
invocation, the JSON event stream, the `--variant`/`--dangerously-skip-permissions`
flags, the `skills.paths` config key, and the auth model below were verified
directly, plus a source-level study of OpenCode's retry/limit handling
(`packages/opencode/src/session/retry.ts`, `cli/cmd/run.ts`, the SDK error types,
and issues #8203/#10432/#15562).

Status: accepted — implemented as `crates/ralphy-agent-opencode` and validated
live against a real OpenCode install (see
[ADR-0005-opencode-validation](./0005-opencode-validation.md)). D4's model
default was later amended by [ADR-0010](./0010-settings-and-opencode-model-default.md).

## D1 — Selection is per run, via `--agent`; the core is untouched

`main.rs` adds `OpenCode` to the `CliAgent` enum and boxes `OpenCodeAgent` as
`Box<dyn Agent>`; `run_queue(&cfg, &queue, agent.as_ref(), …)` is unchanged. This
is the same stance ADR-0004 D1 settled for Codex and is not re-litigated here:
per-run is the smallest surface, keeps the choice out of the core, and matches
ADR-0002's stance that an adapter is the isolated unit. Per-issue routing and a
global env/config switch were already rejected there.

## D2 — Completion is the sentinel for *intent*, with OpenCode's native JSON as the net

Both `plan` and `execute` run headless `opencode run --format json
--dangerously-skip-permissions`, prompt piped on stdin, child cwd = the repo root,
driven by the same reader-thread + poll-`try_wait` + kill-on-timeout loop the
Codex adapter uses (`run_codex`). `--format json` emits line-delimited events
(verified: `step_start`, `text`, `tool_use`, `step_finish` with a `reason`,
`error`, `session.idle`); the assistant's message arrives as `text` parts.

OpenCode gives a *native* terminal signal — `step_finish reason:"stop"` and an
`error` event — so a "did it finish" sentinel is technically redundant. But the
native signal does **not** distinguish a clean finish from a deliberate
"I'm blocked, giving up": both look like `reason:"stop"`. So the sentinel stays
the source of **intent**, and the native signal is the **safety net**:

- `RALPHY_BLOCKED_EXIT <reason>` in the concatenated `text` parts → `Blocked(reason)`.
- exit 0 **and** a HEAD-diff commit **and** `RALPHY_DONE_EXIT` → `Done`.
- a usage/rate match (D9) → `Limit`.
- the per-issue wall timeout → `Timeout`.
- anything else — a JSON `error` event, a non-zero exit, no new commit, or no
  sentinel → `Stuck`.

The HEAD-diff `committed` check is load-bearing: OpenCode creates its own internal
**snapshots** (the `snapshot` hash seen in `step_start`/`step_finish`), **not** git
commits, so a `Done` claim with no new commit is distrusted and downgraded to
`Stuck` — the same progress guard the Claude headless loop and the Codex adapter
already use. `PROMPT_EXECUTE` is reused **verbatim**; it already names both
sentinels and is not Claude-specific. We rejected "native JSON only" (it loses the
Blocked-vs-Done distinction and the blocked reason, and would force an
OpenCode-specific charter) and "sentinel only like Codex" (it would ignore the
`error` event that catches a crash emitting no sentinel).

## D3 — Deterministic: a fixed model, with `--variant` only as an operator knob

Claude routes complexity by swapping `sonnet`/`opus`; Codex routes by
`model_reasoning_effort` (ADR-0004 D3). OpenCode's analog is `--variant`, but its
vocabulary is **provider-specific and non-portable** (Anthropic `high`/`max`;
OpenAI `none`…`xhigh`; providers such as `kimi-for-coding` expose none). Auto-
routing a neutral `low|medium|high` tier onto `--variant` would need a fragile
provider-aware table and would silently break on a provider that rejects the
chosen value.

So the OpenCode adapter is **deterministic — no auto complexity routing.** A fixed
model (D4), with `--variant` passed through **only when the operator sets it**
(`--exec-variant` / equivalent) and omitted otherwise, so the adapter never sends
a value the provider rejects. This is the **effort** knob of CONTEXT.md — a
deterministic value the operator sets — not auto-judged **complexity routing**,
and CONTEXT.md already blesses a deterministic adapter (fixed model + fixed effort)
as a first-class citizen. The OpenCode plan prompt therefore emits **no**
`## Execution model` tier line at all (the mirror-image of why the Codex prompt
emits one).

## D4 — Model resolution defers to OpenCode; `--exec-model` overrides

Claude defaults to `sonnet`; Codex parses its `config.toml` then falls back to
`gpt-5-codex` (ADR-0004). OpenCode has **no natural default** — the provider is
the operator's choice (`-m provider/model`) — and it *already* resolves a default
itself: explicit `-m` → config `model` key → last-used → first-by-priority.

The adapter therefore **omits `-m` entirely unless `--exec-model` is set**,
deferring to OpenCode's own resolution. Re-implementing config parsing (as Codex
does for `config.toml`) would only duplicate OpenCode's native logic and risk
drifting from it, and there is no portable hardcoded fallback to justify it. The
operator owns their `opencode.json` and auth; the adapter respects them. A setup
with no resolvable model surfaces OpenCode's `error` event as an actionable
"configure a model or pass `--exec-model`" stop — the same shape as the auth-error
path (D6).

## D5 — Full autonomy via `--dangerously-skip-permissions`; safety is the isolated branch

This is the one OpenCode-specific hazard with no Claude/Codex analog: in
non-interactive `run` mode any permission left at `ask` (`external_directory` and
`doom_loop` default to `ask`) **blocks the session forever** with no UI to answer
it (issues #14473/#16367). The adapter therefore always passes
`--dangerously-skip-permissions` (auto-approve everything not explicitly denied) —
the flag OpenCode documents *as* the automation escape hatch. There is no Codex-
style OS sandbox in OpenCode; safety rests on Ralphy's existing net: every issue
commits onto an isolated run branch a human merges by hand, plus the reviewer
self-review. The Claude `PreToolUse` guard is not ported.

We rejected injecting an `OPENCODE_CONFIG_CONTENT` permission map (more moving
parts for the same effective full-autonomy) and "flag + config" belt-and-suspenders
(complexity unwarranted for v1). The only config we inject is the skills path (D7).

## D6 — Auth is the operator's; the adapter scrubs the keys OpenCode auto-detects

OpenCode is multi-provider; the operator owns `opencode auth login` (credentials
in `~/.local/share/opencode/auth.json`) and the adapter manages no provider key —
the same stance ADR-0004 D5 took for Codex. The defensive twist OpenCode forces:
it auto-detects **both** `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` from the
environment *and* a project `.env`, either of which can silently override the
operator's chosen provider and switch the run to **metered API billing**. The
adapter therefore `env_remove`s both keys on the child `Command` — the mirror of
Codex's defensive `OPENAI_API_KEY` removal, extended to the two keys OpenCode
actually picks up — so the operator's `auth.json` choice (including a subscription
OAuth) stays authoritative. (`main.rs` already sets `ANTHROPIC_API_KEY=""`
globally; scrubbing on the child is adapter-local and unambiguous regardless.)

A signed-out / revoked account surfaces as a `ProviderAuthError` (or the
logged-out error on stderr) and is mapped to an actionable "run `opencode auth
login` and retry" stop, taking precedence over generic classification because it
won't self-heal — the same precedence the Claude and Codex auth-error detectors
use. Per issue #15562, a Claude-OAuth-via-OpenCode usage-limit *reset* can itself
masquerade as a `ProviderAuthError` requiring re-login; this path handles that
correctly (the operator re-auths and re-runs). The **exact** auth-error strings
are deferred until observed against a live signed-out run.

- *Attempted, not reproducible (live, 2026-06-10, issue #29)*: a genuine
  `ProviderAuthError` could **not** be provoked in the validation environment.
  Moving `auth.json` aside still let `opencode run` succeed (the credential
  resolves from a resilient/cached source), and requesting an unconfigured
  provider returned the same opaque `UnknownError` the gateway emits for any
  failure — **not** a typed `ProviderAuthError`. So the matcher
  (`is_opencode_auth_error`, keyed on the documented case-insensitive
  `providerautherror` substring) stays as-designed and unverified-against-live,
  the conservative branch. A practical corollary surfaced: against the Kimi/Zen
  hosted gateway, an auth failure may arrive as `UnknownError` and therefore fall
  through to the generic "no plan" / `Stuck` path rather than the actionable auth
  stop — acceptable, since `UnknownError` is also the transient-blip shape and
  must not be force-mapped to "run `opencode auth login`".

## D7 — Skills ride under `.ralphy/`, pointed at via injected `skills.paths`

OpenCode auto-discovers Claude-format `SKILL.md` skills from `.opencode/skills/`,
`.claude/skills/`, and `.agents/skills/`, but Ralphy keeps all of its working
state under `.ralphy/` (the Claude adapter's `.ralphy/plugin`, the run dir). To
stay consistent and avoid a stray `.opencode/` or `.agents/` dir in the target
repo, the adapter materializes the **same embedded `reviewer` + `staged-plan`
skill content** into **`.ralphy/skills/`** (clearing and re-extracting each call,
a retargeted near-clone of `materialize_codex_skills`) and points OpenCode at it
by injecting per-run

```
OPENCODE_CONFIG_CONTENT={"skills":{"paths":["<abs>/.ralphy/skills"]}}
```

`skills.paths` ("Additional paths to skill folders") is a real key in OpenCode's
config schema (`https://opencode.ai/config.json`), and `OPENCODE_CONFIG_CONTENT`
is the highest-precedence config source — so this provisions the skills **without
touching the operator's global `~/.config/opencode`** and without depending on
whatever skills happen to be installed globally (the self-contained-binary
guarantee). It is the only config the adapter injects (permissions are the flag,
D5).

Two prompt spots are vendor-isms and get OpenCode variants (D8). All existing
Claude/Codex assets stay untouched.

- *Resolved (live-validated against opencode 1.16.2 on the real repo, 2026-06-10,
  issue #29)*: each `skills.paths` entry is the **container** dir
  (`.ralphy/skills`) — opencode discovers the individual skills inside it. A live
  execute run loaded the reviewer skill (a `tool_use` whose `part` is
  `{"tool":"skill","state":{"title":"Loaded skill: reviewer-v2"}}`) from the
  injected container path, confirming the natural reading. The materialized layout
  and injected path are unchanged.

## D8 — Reuse the skill content; re-target only the plan prompt's two vendor-isms

`PROMPT_EXECUTE` is reused verbatim across all three vendors. Planning gets a new
`assets/prompts/prompt.plan.opencode.md` — a variant of the plan prompt that
(a) emits **no** `## Execution model` tier line (routing is dropped, D3), and
(b) rephrases the reviewer step's "spawn the reviewer as an independent subagent"
(a Claude Task-tool idiom) to OpenCode-neutral dispatch. The single source of
truth stays under `assets/prompts/`; the new prompt is embedded with `include_str!`
the same way the Codex plan prompt is.

- *Resolved (live-probed against opencode 1.16.2, 2026-06-10)*: the reviewer
  self-review runs as the **inline `reviewer` skill** (auto-discovered via
  `skills.paths`), **not** a subagent. True headless custom-subagent dispatch is
  blocked upstream: the Task tool's `subagent_type` enum is hardcoded to
  `explore`/`general` and `@name` routing does not fire for custom agents
  (`opencode#29616`, `opencode#20059`), independent of config- vs markdown-defined
  agents. The inline skill is the only working headless mechanism today;
  subagent isolation awaits the upstream fix. `prompt.plan.opencode.md` now names
  this mechanism (the mirror of ADR-0004's `$reviewer` resolution).

## D9 — A usage limit is caught by the wall timeout, not a text matcher

This is where OpenCode diverges most from Codex's D6, and the divergence is
evidence-based, not assumed. OpenCode's retry engine
(`packages/opencode/src/session/retry.ts`) backs off on retryable errors
(429s are flagged `isRetryable`) **honoring `Retry-After`, with no attempt cap and
`RETRY_MAX_DELAY ≈ 24.8 days`.** Consequences:

- A **short** limit → OpenCode silently waits and self-recovers inside the
  per-issue window. Invisible; no adapter action needed.
- A **long** limit → OpenCode hangs inside the retry loop past the per-issue wall
  budget, frequently emitting **no error event at all** (issue #10432, closed
  not-planned: 429 surfaces no detectable plugin/JSON event). Our existing
  timeout-kill reclaims it → `Outcome::Timeout`.

So the **per-issue wall timeout is the primary limit backstop** — the same
`issue_deadline()` mechanism already in the design, clamped to the run deadline.
A Codex-style limit-*text* matcher cannot be the primary path because the common
case emits nothing to match. The adapter adds only a **best-effort upgrade** of
`Timeout`/`Stuck` to `Outcome::Limit` *when* the JSON stream does emit an `error`
with `name:"APIError"` + `statusCode:429` (or the literal rate-limit strings
OpenCode's own `retryable()` matches, or Zen's `*UsageLimitError`), extracting a
reset hint only if one is present. `--stop-on-limit` is **forced** for OpenCode
(extend `effective_stop_on_limit` exactly as it is forced for Codex): auto-resume
is pointless when OpenCode is already self-waiting short limits and long ones carry
no parseable reset. We rejected auto-resume (ADR-0003's hours-long-hang failure
mode) and treating a limit as plain `Stuck` (loses the actionable re-run signal).

- *Partially resolved (live against opencode 1.16.2, 2026-06-10, issue #29)*: the
  **error-event envelope** is now observed and the parser is fixed to it — a real
  error event is `{"type":"error","error":{"name":<n>,"data":{"message":<m>,…}}}`
  (the `name`/`statusCode`/`message`/`retryAfter` live under `error.data`, not at
  the top level), captured live as
  `{"type":"error","error":{"name":"UnknownError","data":{"message":"Unexpected
  server error.…","ref":"err_…"}}}`. `parse_opencode_limit`/`parse_opencode_events`
  now read fields through `error_detail()`/`error_name()` so the 429 matcher works
  against this shape (the previous code read top-level `name`/`statusCode` and would
  have missed every real limit). A genuine **429 was not reproducible** — the
  Kimi/Zen hosted gateway surfaced *all* transient failures as an opaque
  `UnknownError` rather than a typed `APIError`/`429`. This **reinforces D9's
  thesis**: the text matcher cannot be the primary path, the per-issue wall timeout
  is. The exact 429 string set / reset parser stays best-effort until a real
  rate-limit event is captured.

## Consequences

- The core, `ralphy-agent-claude`, `ralphy-agent-codex`, the existing
  prompts/plugin, `hook.rs`, `guard.rs`, and the `ANTHROPIC_API_KEY` clearing all
  stay untouched. Core-side changes are limited to `main.rs`: the `OpenCode`
  `--agent` arm and extending `effective_stop_on_limit` to force it for OpenCode.
  No-regression for Claude and Codex is structural, not tested-in.
- `plan::count_open_steps` is vendor-neutral and reused. Because routing is
  dropped (D3), the OpenCode adapter leaves `Plan.recommended_model` `None` and the
  core `Plan` shape is unchanged.
- Not ported, by design: the PTY, the Stop hook + flag file, `guard.rs`'s
  `PreToolUse` guard, the workspace-trust shim, Codex's `-o` final-message file
  (replaced by parsing the `--format json` stream), and complexity routing — none
  apply to `opencode run`, and importing them would be the compatibility-shaped
  code this design avoids.
- A known residual risk: if the model invokes OpenCode's interactive `question`
  tool, `--dangerously-skip-permissions` does not suppress it and the run can hang
  (issue #11899) — caught by the per-issue wall timeout (→ `Timeout`), the same
  backstop as the limit hang (D9). A future hardening could deny the `question`
  tool via the injected config.
- Deferred-until-live items, status after the issue #29 capstone (live against
  opencode 1.16.2 on a real repo, 2026-06-10): **resolved** — the reviewer
  dispatch shape (D8, inline skill) and the `skills.paths` granularity (D7,
  container dir, reviewer skill loaded live); **partially resolved** — the
  error-event envelope is now observed and parsed (`error.data` nesting, D9);
  **attempted but not reproducible** — the exact auth-error strings (D6) and a
  real 429 (D9), because the Kimi/Zen hosted gateway surfaced every failure as an
  opaque `UnknownError`. For the unreproduced items the adapter keeps the
  conservative branch (documented-substring match, timeout backstop).
- Corrections folded back from the issue #29 live validation, none re-opening a
  decision:
  - The `--agent` value is pinned to `opencode` (one word) in `main.rs`; clap
    would otherwise derive the kebab-cased `open-code` from the `OpenCode` variant
    and reject the documented invocation. The derived spelling is kept as an alias.
  - The binary is resolved through a shared `resolve_program("opencode")` (new in
    `ralphy-adapter-support`, mirroring the Claude adapter's private resolver)
    rather than a bare `Command::new("opencode")`: on Windows opencode ships as an
    npm `.cmd` shim with no `.exe`, so the bare name was "program not found", and
    the extensionless `opencode` shell shim next to it was "not a valid Win32
    application" (os error 193) — the resolver honours `PATHEXT` and skips the
    extensionless shim. This is shared OS plumbing, the same seam as `run_headless`
    (it does not reopen ADR-0004).
  - The `--format json` event parsing is fixed to the real envelope: every event
    is `{type, timestamp, sessionID, part:{…}}` with the payload (text, tool,
    reason) under `part`, and an error carries `{error:{name,data:{…}}}`. The
    adapter previously read these fields at the top level and would have extracted
    **empty** assistant text (breaking every execute-path sentinel scan) and
    missed every typed error; `event_payload()`/`error_detail()`/`error_name()`
    now read through both the live and the flat shapes. This resolves the "exact
    event JSON deferred until live" caveat in D2.
- One defect surfaced that is **not** opencode-specific and is filed as a
  follow-up: when `.ralphy/plan.md` is *tracked* in the base branch (it was
  accidentally committed to the real repo's `origin/main` by an earlier run),
  `.gitignore` cannot un-track it, so the planner's overwrite leaves a modified
  tracked file and the clean-run return-to-orig — a **non-force** `git checkout`
  in `ralphy-core` (unlike the dry-run/error `restore()`, which forces) — aborts,
  stranding the repo on the run branch. The green deliverable and close-on-green
  are unaffected; only the final branch hand-back is. Fix is repo hygiene
  (`git rm --cached .ralphy/plan.md`) plus optionally hardening the core
  checkout-back to tolerate tracked scratch
  ([#41](https://github.com/paulocorcino/ralphy/issues/41)).
