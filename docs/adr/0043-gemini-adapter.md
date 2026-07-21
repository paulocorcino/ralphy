# The Gemini adapter: a seventh vendor, driven from a config root Ralphy owns

Ralphy gains a seventh agent CLI vendor, `gemini` (Google Gemini CLI), as a new
isolated crate `ralphy-agent-gemini` implementing the same PTY-free `Agent`
trait ([ADR-0002](./0002-core-agnostic-adapter-boundary.md)). It is selected
**per run** by `--agent gemini`; the core keeps taking a single `&dyn Agent` and
never learns which vendor it holds ([ADR-0004](./0004-codex-adapter.md) D1).

The template is **Claude, structurally, and nobody, operationally**. Gemini
shares Claude's shape — a mintable session id, a hook system with a documented
Claude migration path, skills, a streaming JSON transport — but it is
headless-first in a way Claude is not, and it carries a governance surface no
previous vendor had: **autonomy is not expressible in argv**. Three separate
mechanisms outside the command line can revoke `--approval-mode yolo`.

That single fact drives the shape of this ADR. Where earlier adapters negotiate
with the operator's environment, this one **moves out of it** (D4).

Grounded in **Gemini CLI 0.51.0** on Windows 11 and WSL, probed hands-on across
three rounds (~55 paid API requests) against `C:\Dev\FinCal`, plus the
version-matched documentation and the esbuild-bundled source shipped inside the
npm package. Full evidence — command surface, stream schema, exit-code
enumeration, session store, cost model — is in
[docs/research/gemini-cli-adapter-spike.md](../research/gemini-cli-adapter-spike.md);
this ADR records the decisions, the spike records the observations.

Status: **proposed** — decisions settled, implementation not started.
Consistent with ADR-0002/0004/0005/0008/0023/0025/0030/0033/0034/0040/0042;
applies the [ADR-0040](./0040-agent-adapter-onboarding-contract.md) onboarding
contract for the third time, and amends its wiring inventory (see that file's
Amendment 2).

[ADR-0042](./0042-cursor-adapter.md) (Cursor) is the closest precedent and is
cited throughout, because the two vendors fail in the same direction: both ship
defaults that must be refused before they run, and both are driven from an
isolated config root (D4). Where this ADR diverges from Cursor's answer — the
seeding question in D4, the hook in D3 — it says so.

## D1 — Selection is per run, via `--agent gemini`; the core is untouched

`CliAgent` gains a `Gemini` variant and `build_agent` boxes `GeminiAgent` as
`Box<dyn Agent>`. Same stance as ADR-0004 D1 / ADR-0005 D1 / ADR-0028 D1 /
ADR-0041 D1, not re-litigated.

All **three** independent agent enums must be wired —
`cli.rs::CliAgent`, `init/gate.rs::Agent` (whose `ALL` array length is
hardcoded — currently `5`, and it must account for Cursor as well as Gemini),
and `daemon/src/session.rs::Agent`. ADR-0041 D1 recorded that Kimi was missing
from the third; that is now fixed, and this adapter must not recreate the gap.

**Sequencing:** [ADR-0042](./0042-cursor-adapter.md) (Cursor) **goes first** — the
maintainer's call. Cursor bumps `ALL` from `5` to `6` and adds its variant to all
three enums; this adapter rebases on that result and takes `7`, rather than
assuming any of the three counts.

Two edit sites ADR-0040 did not list are load-bearing here and are added to it
by amendment: **`crates/ralphy-cli/src/config.rs`** (~10 distinct places — the
settings struct import, `KEYS`, a `with_gemini` helper, and arms in `set`,
`unset`, `print` and the JSON emitter) and **`crates/ralphy-cli/src/run.rs`**.

## D2 — The prompt goes in on stdin, and the ordering is a discovered contract

```
gemini --approval-mode yolo --skip-trust --session-id <id> \
       --output-format stream-json --policy <ralphy-owned.toml> \
       [--model <id>]   < <charter on stdin>
```

stdin is a first-class, documented channel here, with an **8 MB ceiling** — so
unlike every previous vendor, argv truncation is not even a latent risk. The
spike piped a 25 404-byte charter through and got both planted markers back,
with `input_tokens = 30 073`.

Three properties of the channel are **not** what the documentation says, and the
adapter depends on all three:

1. **stdin is prepended, not appended.** The CLI builds
   `` `${stdin}\n\n${-p text}` `` — verified in source and observed verbatim in a
   `message/user` record. Both `--help` and the docs say "appended". Ralphy's
   charter therefore goes on **stdin** and any per-run addendum on `-p`, which is
   the order Ralphy wants — but it is load-bearing, so the assembly test must
   assert it rather than trust it.
2. **A 500 ms grace timer** governs the read: if nothing arrives on a non-TTY
   stdin within 500 ms, the CLI stops waiting and proceeds with an empty prompt.
   **The adapter must write the payload immediately and close stdin** — never
   spawn first and compute the prompt after.
3. `-p` and `-i` are mutually exclusive, as are `--resume`/`--session-id`/
   `--session-file`; each is a hard argv error, so they fail fast.

## D3 — Completion: exit code first, envelope second, deltas joined

`--output-format stream-json` is mandatory. The terminal envelope is
`{"type":"result","status":"success"|"error","stats":{…}}`.

**Exit code is the primary signal, not the envelope**, because the envelope is
not always present: a **pre-flight** failure (auth) emits no `result` record at
all, while a **mid-run** failure does. A parser that waits for the envelope hangs
on the one case it most needs to detect.

Gemini offers the richest exit-code taxonomy of any vendor — ten codes, six of
them undocumented, recovered from the bundled `FatalError` hierarchy and two
confirmed live:

| Code | Class | Ralphy's reading |
|---|---|---|
| `0` | — | success |
| `1` | — | generic failure, incl. API errors |
| `41` | `FatalAuthenticationError` | auth stop (D6) |
| `42` | `FatalInputError` | bad argv — a Ralphy bug |
| `44` | `FatalSandboxError` | sandbox off by policy; unexpected |
| `52` | `FatalConfigError` | our own config root is malformed (D4) |
| `53` | `FatalTurnLimitedError` | turn ceiling — a budget stop, not a failure |
| `54` | `FatalToolExecutionError` | tool failure, distinct from model failure |
| `55` | `FatalUntrustedWorkspaceError` | actionable stop; D5 should prevent it |
| `130` | `FatalCancellationError` | Ralphy killed it — not a crash |
| `199` | — | internal self-relaunch sentinel; should never be observed (D18) |

**The set is not closed.** `extractErrorCode()` passes any numeric `.code` or
`.status` straight to `process.exit()`, so a raw HTTP `429` is a reachable exit
code. The match needs a catch-all arm, and `429` is a limit candidate (D11).

Two parsing rules the spike forced:

- **The final assistant message is the concatenation of consecutive `message`
  records with `role: "assistant"`.** There is no non-delta final record, and
  **deltas split mid-word** — one observed run emitted `"RAL"` then
  `"PHY_SKILL_LOADED_B4D2…"`. A per-record sentinel match fails. Join first,
  match second; the test must use a boundary-straddling fixture.
- **Both streams must be captured.** Fatal errors are well-typed
  (`error.type: "FatalTurnLimitedError"`) but arrive on **stderr** even under
  `-o json`; mid-run errors arrive on stdout but are **untyped**
  (`type: "unknown"`, message `"[API Error: An unknown error occurred.]"`), with
  the real diagnosis on stderr. Neither channel alone is sufficient.

`CompletionSignals` is filled from the sentinel plus `result.status`; ordering
still delegates to `classify` ([ADR-0023](./0023-shared-outcome-classifier.md)).

### The hook is the better signal, and is deliberately deferred

Gemini fires `AfterAgent` in headless mode, synchronously, handing the finished
answer on stdin as `prompt_response` alongside `session_id` and
`transcript_path`. That is deterministic completion without text scraping — the
prize [ADR-0040](./0040-agent-adapter-onboarding-contract.md) C3 asks for.

It is **not** adopted in v1. The sentinel and envelope already work, and D4's
config root is what makes a hook safe to install at all (hooks fire only at user
scope — a workspace-scoped hook is silently ignored). Shipping both at once
couples two new mechanisms. Recorded as the upgrade path, to be taken once D4 is
in place and proven.

Note this is **deferred, not rejected**, unlike [ADR-0042](./0042-cursor-adapter.md)
D3, which rejects Cursor's `stop` hook outright. The difference is evidence:
Gemini's `AfterAgent` was observed firing headless and delivering the finished
text, so the mechanism is known to work and only the integration is unbuilt.

## D4 — Ralphy drives Gemini from a config root it owns, and does **not** seed it

**This is the central decision.** The adapter sets `GEMINI_CLI_HOME` to a
Ralphy-owned directory and writes a minimal `settings.json` into it.

This is the second vendor to need it: [ADR-0042](./0042-cursor-adapter.md) D17
does the same with `CURSOR_CONFIG_DIR`. Two of seven vendors now require a
scratch config root, which makes it a **pattern rather than a special case**, and
`ralphy-adapter-support` is the right home for whatever the two implementations
turn out to share.

The alternative — driving the CLI against the operator's `~/.gemini` — is
untenable, because four separate holes open there at once:

| Hole in the default root | Closed by an owned root |
|---|---|
| `~/.gemini/policies/*.toml` (user tier, base 4) **outranks `--yolo`** (default tier, 1.998) | not read |
| `~/.gemini/GEMINI.md` is concatenated into **every prompt**, competing with the charter, with no documented off switch | not read |
| Hooks fire **only at user scope**, so installing one means mutating the operator's file | Ralphy's root *is* the user scope |
| The root is **shared with Google's Antigravity IDE** | fully separated |

This works because the credential does not live under the root: on an API-key
install the secret is in the **OS credential store** (Windows Credential
Manager, target `gemini-cli-api-key/default-api-key`, via keytar). Relocating
the root loses only the *pointer*, which a four-line `settings.json` restores —
verified end-to-end.

### Not seeded — and this is where Gemini and Cursor diverge

ADR-0042 D17 **seeds** its scratch directory from the operator's own
`cli-config.json`, so that their deliberate `permissions.deny` policy still
applies: *"Policy flows in; mutations die with the run."* That reasoning is
sound for Cursor, where the imported artifact can only ever **restrict**.

It does not transfer. Gemini's policy tiers can also **expand** autonomy — the
vendor's own documentation ships a rule named *"Allow pr-creator to push code"*
with `commandPrefix = "git push"`, `decision = "allow"` — and the same root
carries `GEMINI.md`, which is concatenated into every prompt and is precisely
what D4 exists to exclude. Seeding wholesale would re-import two of the four
holes it closes.

So: **the root starts empty**, and the adapter writes only what it intends. To
avoid discarding protective operator intent along with the rest, the one thing
it may import is **`deny` rules from `~/.gemini/policies/*.toml`, with `allow`
and `ask_user` rules dropped**. Restriction flows in; expansion does not; nothing
flows back.

This is a deliberate asymmetry, not an oversight, and it is the honest reading of
Ralphy's posture: the operator's *"never do this"* is respected, their
*"always allow this"* is not a grant Ralphy may accept on their behalf while
running unattended.

### 🔬 The root is persistent and per-workspace, not per-run

This is the lifetime, and it is measured rather than assumed — the distinction
matters because "scratch config root" reads as *create, use, discard*, and that
would be wrong here.

- **The root carries installation identity.** `installation_id` is minted *inside*
  it and is stable within it: a root kept across two runs held
  `b54f6a30-…` both times, while a freshly created root minted a different
  `d58afb66-…`. **A per-run root would mint a new install identity on every
  turn** — needless fingerprint churn against the vendor.
- **Nothing forces a discard.** The CLI **does not rewrite Ralphy's
  `settings.json`** — after two runs it was byte-identical to what was written.
  There is no vendor mutation to contain, which is the opposite of the situation
  that motivates [ADR-0042](./0042-cursor-adapter.md) D17.
- **Discarding buys nothing either.** A cold root still saw **8 138 cached
  tokens** on its first call: implicit context caching is server-side and
  indifferent to local root age.

So the module's shape is **idempotent reconciliation**, not construction:
`ensure(workspace) -> Root`, safe to call on every run, writing only what has
drifted. A run leaves behind four stable files (`installation_id`,
`projects.json`, `settings.json`, `.project_root`) plus its session JSONL;
session growth is bounded by setting `general.sessionRetention` in the same file.

Consequences and limits, stated plainly:

- The root is Ralphy state, under its existing conventions, and is **created and
  owned by the adapter**, never merged with the operator's.
- 🔬 **Skills and hooks both resolve from the relocated root**, so D4's claim that
  it *is* the user scope is verified, not assumed: `gemini skills list` reported
  the skill from the relocated path, `activate_skill` loaded its body, and an
  `AfterAgent` hook declared there fired with a 1 278-byte payload.
- The operator's user-scope skills at `~/.gemini/skills/` are invisible to a
  Ralphy run — the same behaviour change ADR-0042 D17 documents for Cursor, and
  worth surfacing in `ralphy init` for the same reason.
- **This is not verified for OAuth auth**, where the credential *is* file-based
  under the root and relocation would orphan it. The preflight
  ([ADR-0013](./0013-run-auth-preflight.md)) must therefore validate against the
  relocated root and surface exit 41 normally — which it does, unchanged.
- Ralphy gains the ability to *deny* the operator their own Gemini
  customisation. That is the point for a supervised run, and it is the opposite
  of the opt-in posture Ralphy takes on security features — justified because
  this root governs a **child process Ralphy is accountable for**, not the
  operator's own interactive use, which is untouched.

## D5 — Autonomy is asserted three ways, because argv alone cannot hold it

`--approval-mode yolo` is necessary and **not sufficient**. The spike found
three independent ways it is revoked, none visible on the command line:

1. A higher-tier **policy** rule. YOLO is itself just a rule, in the *default*
   tier at final priority `1.998`; any user (4.x) or admin (5.x) rule outranks
   it. D4 removes the user tier from play; **admin policies remain sovereign**
   and cannot be outranked.
2. Enterprise **Strict Mode**, which is *enabled by default* on managed machines
   and removes yolo entirely.
3. An **untrusted folder**, which silently prints
   `Approval mode overridden to "default"` and demotes the run.

So the adapter always passes `--approval-mode yolo` **and** `--skip-trust`
(against #3 and exit 55), and ships its own `--policy` file on argv.

### 🔬 `--policy` outranks the user tier — measured, not inferred

The open question was which tier an argv policy lands in. It was settled by
staging a conflict: a **user-tier** rule inside the run's own root
(`policies/allow-shell.toml`, `decision = "allow"`, `priority = 900` → final
`4.900`) against an **argv** rule (`decision = "deny"`, `priority = 100`).

The deny won. The model reported no shell tool existed at all.

So argv policy is **sovereign over anything the operator's configuration can
say** — which is what makes D5 a real mitigation rather than a hopeful one, and
it holds independently of D4. Note the two combine: D4 removes user-tier policy
from the picture, and `--policy` would beat it even if it did not.

⚠ Untested, and presumed to remain true: an **admin**-tier deny (base 5) is
expected to outrank argv. Admin controls stay out of reach by design.

The policy denies `run_shell_command` only where a run should not shell out, and
**always denies `invoke_agent`** — see D15's correction for why that, and not a
settings key, is the control.

`--yolo` is not used: the documentation marks it deprecated in favour of
`--approval-mode=yolo`, even though `--help` does not.

**The policy must constrain `invoke_agent`, not only `run_shell_command`.**
When the spike denied the shell, the model immediately called
`invoke_agent{agent_name: "generalist"}` and asked the subagent to run the same
command. The deny held only because subagents inherit the policy. A deny surface
that names one tool is one indirection wide.

Where #1 and #2 cannot be defeated, they must be **detected, not worked around**:
a run whose tools are refused wholesale is a stop with an actionable message,
never a silent degradation.

## D6 — Auth detection is behavioural, on exit 41

`GEMINI_AUTH_ERROR_MSG` names no login command, because **the CLI has none** —
no `login`, `logout`, `auth` or `status` subcommand exists. Authentication is
either an interactive browser OAuth flow on first run or an environment
variable, and the adapter cannot perform either.

The stop message therefore reproduces the CLI's own sentence, which is good:

> Please set an Auth method in your `<root>/settings.json` or specify one of the
> following environment variables before running: `GEMINI_API_KEY`,
> `GOOGLE_GENAI_USE_VERTEXAI`, `GOOGLE_GENAI_USE_GCA`

Detection is **exit code 41**, not text — unambiguous, and unlike Cursor no
auth-adjacent verb exits 0 while logged out. `is_gemini_auth_error` keeps
`Please set an Auth method` as a secondary marker only.

Never inspect the credential store or `oauth_creds.json`; behavioural detection
is the settled house style and here it is also the only honest option, since an
API key leaves nothing on disk to inspect.

## D7 — Env hygiene: the child gets an explicit allowlist

`getAuthTypeFromEnv()` resolves auth by **priority, not by specificity**:
`GOOGLE_GENAI_USE_GCA` → `GOOGLE_GENAI_USE_VERTEXAI` → gateway →
`GEMINI_API_KEY` → Cloud Shell / ADC. An inherited
`GOOGLE_GENAI_USE_VERTEXAI=true` therefore **outranks the operator's own API
key** and silently redirects billing to a different account.

`GOOGLE_*` is a namespace gcloud, Firebase tooling and CI runners all populate,
so this is the `ANTHROPIC_API_KEY` cross-contamination shape
([ADR-0040](./0040-agent-adapter-onboarding-contract.md) C5) with a wider
blast radius. The child is therefore built with an **explicit allowlist**:
`GEMINI_CLI_HOME` (D4) and whichever single auth variable the operator's
selected method needs — everything else in `GEMINI_*` / `GOOGLE_GENAI_*` /
`GOOGLE_CLOUD_*` / `GOOGLE_API_KEY` is scrubbed.

Note the comparison in source is exactly `=== 'true'`: `1`, `TRUE` and `yes` do
not work, so the adapter must emit the literal string.

## D8 — Model is `Option<String>`, omitted when unset — but omission has a price

Model resolution is `Option<String>`, omitted from argv when `None`, never a
hardcoded default (the ADR-0041 D4 stance, and ADR-0040's C4 rule). There is no
free way to enumerate models — no `--list-models` exists — and entitlement
cannot be inferred: the spike's key served `gemini-3.1-pro-preview` despite the
documentation calling that tier Flash-only.

What is new here is that **omission is not free**. Leaving the model as `auto`
spends a *second, paid model call* on routing — a `utility_router` turn on
`gemini-3.1-flash-lite` costing 3 000–11 000 tokens and, critically, **one extra
API request** on a tier metered in requests. Pinning `-m` removes it entirely.

So the default stays `None` (Ralphy does not choose the operator's model), and
`config.rs` gains `gemini.plan_model` / `gemini.exec_model` so an operator can
pin and roughly halve their request consumption. The cost is documented in
`ralphy init`'s output rather than buried.

An unknown id is **not** rejected locally — `resolveModel()` passes unknown
strings through verbatim, so a typo costs a round trip and returns
`ModelNotFoundError … code: 404` on stderr with `type:"unknown"` on stdout.
There is no cheap actionable stop to build on; the 404 text is matched instead.

## D9 — Usage comes from the stream envelope, never the session store

`result.stats` is the source of truth. The store is not, and this is not a
preference:

- Every run makes a routing call whose tokens **never reach disk**. Two measured
  runs: 18 273 streamed vs 14 567 stored, and 32 281 streamed vs 20 924 stored —
  the store under-reports by **20–35 %**.
- Subagent turns land in a *nested* session file
  (`chats/<parent-id>/<uuid>.jsonl`, `kind: "subagent"`), which a flat glob
  misses.

Three arithmetic traps, each of which must have a test that fails on the naive
choice:

1. **Billable output is `total_tokens - input_tokens`, not `output_tokens`.**
   `StreamStats` has no thinking-token field, but the residual is real and
   Google bills thinking at the output rate. One run reported `output_tokens: 88`
   against 2 208 tokens of actual billable output — a **25× under-count**.
2. **`input_tokens` already includes `cached`**; the uncached remainder is the
   separate `input` field (`64 901 = 16 273 + 48 628`). Adding them double-counts.
   Cache-read is thus separable per [ADR-0008](./0008-token-usage-tracking.md) D2;
   there is **no** cache-creation counter to separate.
3. `stats.models` is a **map keyed by concrete model name** — a single run
   routinely spans two or three. `Usage::fold_usage` heaviest-model attribution
   is load-bearing, not cosmetic.

The session id is minted by Ralphy via `--session-id`, which accepts any
`^[a-zA-Z0-9-_]+$` string despite advertising a UUID. Lookup is a direct key and
the [ADR-0008](./0008-token-usage-tracking.md) D10 snapshot-diff is unnecessary.

## D10 — `scan_gemini` reports a lower bound, and says so

[ADR-0033](./0033-interactive-usage-stateless-scan.md) wants a pure, read-only,
never-erroring scan of interactive sessions. Gemini permits one — sessions are
JSONL at `~/.gemini/tmp/<project-dir-name>/chats/`, with per-turn, per-model
`tokens` records carrying `input`, `output`, `cached`, `thoughts` and `tool`,
and a sibling `.project_root` file mapping the directory back to a repo path (no
hash to reverse, unlike Cursor).

Two constraints:

- Usage is **incremental — sum it, do not keep-last** (the Kimi convention, not
  the Codex one).
- The scan **must recurse** into `chats/<session-id>/` for `kind: "subagent"`
  files, and even then it is a **lower bound**, because the router's tokens are
  never written. `scan_gemini` reports it as such rather than presenting a total
  it knows is short.

## D11 — Limits map to `Limit(None)` plus the synthetic cadence, and Ralphy adds no retry

Gemini reserves **no exit code for quota** despite having ten semantic codes,
publishes no machine-readable reset hint, and its documented "switch to a
fallback model?" flow is **interactive-only**: `setFallbackModelHandler` is
registered exclusively in a React hook in the TUI, and returns `null` headless.
So a quota failure headless does not downgrade and does not prompt — the request
simply fails.

The limit predicate matches a **limit class** — a regex over
`rate limit | quota exceeded | too many requests | resource exhausted` and a bare
`429` exit code — never one phrasing (the OpenCode `usage_limit_regex`
reference). It emits `Limit(None)`, and
[ADR-0030](./0030-synthetic-reset-for-unschedulable-limits.md)'s synthetic
~30-minute cadence applies.

**Ralphy adds no retry layer.** The CLI already wraps calls in
`retryWithBackoff`, absorbing transient 429s silently — twelve parallel runs
(24 requests) never surfaced one. Whatever reaches Ralphy has already exhausted
the vendor's own retries and is a hard failure. Retrying it again would multiply
an already-invisible quota burn.

⚠ True daily-quota exhaustion was **not** observed; reproducing it costs a day's
allowance. This decision is therefore the most likely in this ADR to need
revising, and it is deliberately the cheapest one to revise — `Limit(None)`
requires no reset parsing to be correct.

## D12 — The native plan mode is rejected

`--approval-mode plan` runs cleanly headless, and is still unusable: it writes
the plan artifact to the **vendor's private store**
(`~/.gemini/tmp/<project>/<session>/plans/plan.md`) regardless of instruction.
The spike asked explicitly for a different path *and* granted `write_file` there
by policy at priority 200; the model never attempted the requested path, because
plan mode's own system prompt sends it elsewhere. Permitting the path is not
enough to move the artifact.

Ralphy's planner therefore runs in the **same yolo mode as the executor**, with
the Ralphy charter, writing `.ralphy/plan.md` itself — the norm ADR-0040 C8
predicts, and the same conclusion [ADR-0042](./0042-cursor-adapter.md) D9 reached
for Cursor. **Every vendor with a native plan mode has now had it rejected**, for
the same underlying reason each time: the mode persists its artifact where the
vendor wants it, not where the caller asks.

Two facts are kept for later rather than discarded: plan mode pins a Pro model
and **pays no router tax**, which makes it interesting for D8's cost story if the
artifact problem is ever solved upstream.

## D13 — Skills materialize into the owned root

Skills activate headless — `activate_skill` is auto-approved under yolo, and the
spike verified a skill's body genuinely loaded by planting a token that existed
only inside `SKILL.md`. `gemini skills list` confirms discovery for free, with
no paid call, which makes it a usable post-materialization assertion.

Skills are written to `<owned root>/skills/<name>/SKILL.md` (D4) with `name` and
`description` frontmatter — the [agentskills.io](https://agentskills.io) format,
the same one Ralphy already targets for Codex and Copilot via `.agents/skills`.
Nothing is written to the operator's root.

⚠ Two open risks, neither blocking: activation was always *explicitly requested*
in the probes, so description-matching alone is unproven; and enterprise
"Unmanaged Capabilities" disables Agent Skills by default on managed machines,
which D5's detect-don't-defeat rule covers.

## D14 — `ACCEPTS_IMAGES` is true, delivered in the prompt rather than in argv

Gemini has no attachment flag. The delivery channel is the **`@<path>` syntax
inside the prompt text**, and it works headless: a 64×64 red PNG referenced as
`@.ralphy-probe/red.png` was described correctly, with no `read_file` tool call —
the CLI resolves the reference into an inline image part before the request.

So a triage attachment fetched per [ADR-0025](./0025-triage-attachment-evidence-fetch.md)
§4 has a real delivery path, and `ACCEPTS_IMAGES = true`. Unlike Copilot's
`--attachment <path>` per image (ADR-0041 D12), **attachment delivery is coupled
to prompt construction**, not to argv — `command.rs` interpolates.

The corollary was probed rather than assumed: `@` is live syntax and issue
bodies are full of `@mentions`. It **fails safe** — the CLI resolves `@` only
when the path exists, leaving `@octocat`, `@nonexistent-file.md` and
`foo@bar.com` as literal text. The residual hazard is narrow and documented: an
issue body containing `@README.md`, where that file exists in the target repo,
silently injects it.

## D15 — Blast radius is forced closed in the owned root

Ralphy never pushes and never opens PRs. Gemini ships no push or PR **verb** —
better than Cursor — but reaches the same place through `run_shell_command`, so
D5's policy is the real control. Beyond it, the owned root's `settings.json`
forces:

| Setting | Value | Why |
|---|---|---|
| `experimental.enableAgents` | `false` | Set defensively, **but not relied upon** — see the correction below. Remote A2A agents are on by default and are defined by repo-local `.gemini/agents/*.md` with an arbitrary `agent_card_url`, so a cloned repo could aim delegation at a third-party endpoint. |
| `privacy.usageStatisticsEnabled` | `false` | On by default. Ralphy does not opt the operator's supervised runs into vendor analytics. |
| `telemetry.enabled` | left `false` | Already the default; if ever enabled, `logPrompts` defaults to `true` and would ship prompt text. |
| `experimental.autoMemory` | left `false` | Already off; would spend background model calls mining transcripts. |
| `tools.sandbox` | left off | The Windows native sandbox sets **persistent** low-integrity ACLs that survive the session. |
| `experimental.worktrees` | left off | Ralphy owns its branches. |

### 🔬 Correction: `experimental.enableAgents: false` does **not** disable delegation

An earlier draft treated that setting as the control for subagents and remote
agents. **Measured, it does not work.** With
`{"experimental":{"enableAgents":false}}` in the run's own root, the model still
called `invoke_agent{agent_name: "generalist"}` and the tool returned
`status: "success"` — the subagent ran.

**The working control is the D5 policy deny, and it is strictly better.** With
`toolName = "invoke_agent"`, `decision = "deny"` passed via `--policy`, the tool
was **removed from the model's schema entirely**: the model reported that
`invoke_agent` *"is not defined or available in the current environment's tool
schema"* and went searching the codebase for what the name meant.

That property is the reason to prefer the policy — a global deny excludes the
tool from the model's memory rather than refusing it at call time, so the model
cannot burn turns arguing with a refusal it never sees. The blast radius is
therefore closed by **one mechanism, not two**, and the settings key above is set
defensively but never relied upon.

`--approve-mcps` has no equivalent here and no MCP servers are configured in the
owned root, so the repo-local `.gemini/mcp.json` vector is closed by D4 rather
than by a flag.

**Admin-tier controls are out of reach by design** — admin policies, enforced
auth type, and required MCP servers with `trust: true` all outrank anything
Ralphy can set. They are detected and reported, per D5.

## D16 — Binary resolution must reject the WSL `/mnt/c` shim

`ralphy-proc-util::resolve_program` already handles the Windows case: Gemini
installs as an npm shim trio (`gemini`, `gemini.cmd`, `gemini.ps1`) with no
`.exe`, which is precisely what that function and its `opencode.cmd` fixtures
were written for. **No new Tier 1 work on Windows.**

Linux and WSL need two additions, both observed:

1. On WSL, `PATH` interop makes `which gemini` resolve to the **Windows** shim
   at `/mnt/c/Users/.../npm/gemini`, which then dies with `exec: node: not found`
   (exit 127). This is worse than the Kimi precedent, where `which` merely
   returned nothing: here it returns a **falsely positive** path, so detection
   and execution agree in being wrong together. `locate_program` must reject
   `PATH` entries under `/mnt/c/` when running on Linux.
2. The working install is under `~/.nvm/versions/node/<version>/bin/`, which the
   existing `~/.local/bin` fallback does not cover.

Both changes live in `ralphy-proc-util` and benefit every future npm-distributed
vendor, so they are made there rather than in the adapter.

## D17 — The vendor's terms were reviewed, and driving the binary is the sanctioned path

Gemini's terms state that *"directly accessing the services powering Gemini CLI
… using third-party software, tools, or services (for example, using OpenClaw
with Gemini CLI OAuth) is a violation"*, naming a competing agent-runner.

Ralphy **spawns the vendor's own binary as a child process** and never reads,
copies or replays its credential — which is why D6 forbids credential-file
inspection and D4 relies on the CLI resolving its own secret from the OS store.
That is the distinction the clause draws: reusing the *token* against Google's
endpoints, versus running the *tool* Google ships.

This was put to the maintainer with the clause quoted verbatim before any code
was authorized, and accepted on that basis. It is recorded here because it is a
business risk rather than an engineering one, and because the distinction must
survive future refactors: **any change that talks to a Google endpoint directly,
rather than through the `gemini` binary, invalidates this decision** and needs a
new one.

## D18 — Termination reuses `kill_tree` unchanged; the budget builders need nothing new

The three budget builders (`with_max_minutes_per_issue`, `with_idle_minutes`,
`with_run_deadline`) all end in killing the child, and Gemini is Node — so the
concern was a shell-tool grandchild outliving the run. It does not.

The tree is **five levels deep** —
`cmd.exe` → node (npm shim) → node (self-relaunched with a 16 GB heap) →
`pwsh.exe` (the shell tool) → the command itself — and
`ralphy_proc_util::kill_tree`'s `taskkill /F /T` clears all five, verified with a
120-second `ping` still running at kill time and a follow-up sweep finding no
survivors.

Two consequences: **no new termination machinery**, and `kill_tree` is
**mandatory rather than defensive** here — a plain `child.kill()` on the direct
child would strand four processes, including a live shell.

Note the self-relaunch is also the mechanism behind the `199` sentinel in D3's
table: the wrapper re-execs and loops rather than exiting, which is why that code
should never be observed.

## What this ADR deliberately does not decide

- **The eight overlay slots.** `overlay.gemini.md` exists and the assembly test
  registers it, but which slots it fills is a prompt-engineering question
  settled during implementation, not here.
- **Whether to adopt the `AfterAgent` hook** (D3) — deferred with a stated
  trigger, not rejected.
- **`PriceTable` tiering.** Gemini's Pro models price differently above a 200 k
  prompt, and Ralphy's charter alone is 30 k of it; whether
  [ADR-0034](./0034-robust-read-time-pricing.md)'s table grows a tier dimension
  is a change to that ADR, not this one. Three id-level traps are recorded in the
  spike and must be honoured whatever the shape: `gemini-3-pro-preview` is
  **retired upstream** yet still a CLI constant; `gemini-3-flash` is a CLI-local
  alias for `gemini-3.5-flash` whose price differs **3×** from Google's
  similarly-named model; and `gemini-3.1-pro-preview-customtools`, which served
  two probe runs, **has no published price**.
- **Quota behaviour under true exhaustion** (D11), which remains unobserved.
