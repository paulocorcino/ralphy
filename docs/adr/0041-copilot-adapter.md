# The Copilot adapter: a fifth vendor that defaults to the operator's own selection

Ralphy gains a fifth agent CLI vendor, `copilot` (GitHub Copilot CLI), as a new
isolated crate `ralphy-agent-copilot` implementing the same PTY-free `Agent`
trait ([ADR-0002](./0002-core-agnostic-adapter-boundary.md)). It is selected
**per run** by `--agent copilot`; the core keeps taking a single `&dyn Agent`
and never learns which vendor it holds ([ADR-0004](./0004-codex-adapter.md) D1).

The template is **OpenCode, not Codex**. Copilot shares OpenCode's two defining
traits: a SQLite session store rather than a file tree, and a model axis the
adapter must *not* decide for the operator. Where Copilot is richer — a minted
session id, a real terminal envelope, per-call usage rows with prices, an
in-band receipt for its own kill switches — the adapter takes the win.

Grounded in **GitHub Copilot CLI 1.0.71** on Windows 11, probed hands-on across
two rounds against `C:\Dev\FinCal`. Full evidence — command surface, stream
schema, session store, catalog, cost traps — is in
[docs/research/copilot-cli-adapter-spike.md](../research/copilot-cli-adapter-spike.md);
this ADR records the decisions, the spike records the observations.

Status: **accepted** — decisions settled and shipped in slices, then
**live-validated end-to-end** against `paulocorcino/FinCal` on 2026-07-22
([#272](https://github.com/paulocorcino/ralphy/issues/272);
[validation note](./0041-copilot-validation.md),
[evidence](../evidence/272-copilot-capstone-live.md)). The capstone ran a paid
plan-then-execute to green, reconciled tokens against the real AI-credit bill, and
confirmed the interactive-scan inversion; one item is deferred by maintainer ruling
(see D11). Consistent with ADR-0002/0003/0004/0005/0008/0023/0030/0040; applies the
[ADR-0040](./0040-agent-adapter-onboarding-contract.md) onboarding contract for
the first time.

Implementation status: D5/D5a shipped in #233 (`ralphy-agent-copilot`'s
`effort.rs`, the persisted `copilot.plan_effort`/`copilot.exec_effort` keys, and
the post-hoc check against the vendor's `assistant_usage_events.reasoning_effort`).
D7's in-band receipt guard and D11's `continueOnAutoMode` preflight shipped in
#234 (`guards.rs`, plus the `copilot.allow_builtin_mcp_servers_i_understand_the_risk`
escape hatch) — see the `**Enforced**` notes on those two decisions.

## D1 — Selection is per run, via `--agent copilot`; the core is untouched

`CliAgent` gains a `Copilot` variant and `build_agent` boxes `CopilotAgent` as
`Box<dyn Agent>`. Same stance as ADR-0004 D1 / ADR-0005 D1 / ADR-0028 D1, not
re-litigated.

ADR-0040 names the trap and it is load-bearing here: there are **three
independent agent enums** that share no definition, and
`crates/ralphy-daemon/src/dispatch.rs::agent_flag` knows only three vendors —
Kimi was never added, so Kimi is unreachable from the workbench today. Copilot
must be wired into **all three**, and the pre-existing Kimi gap is tracked
separately rather than silently fixed here.

## D2 — The prompt goes in on stdin, and this is not negotiable

```
copilot --allow-all-tools --output-format json --session-id <uuid> \
        --no-remote --no-remote-export --disable-builtin-mcps --no-ask-user \
        [--model <id>] [--effort <level>]   < <charter on stdin>
```

No `-p`. Ralphy's `prompt.execute.md` is 23 884 bytes *before* the issue body is
appended, against a Windows argv ceiling of ~32 KB — argv is a latent truncation
bug with no margin. The spike verified stdin end-to-end: a 24 250-byte payload
piped in returned markers planted on **both** its first and last line, with
`input_tokens = 31594` in the usage row.

This matches Kimi (ADR-0028 D2), which also feeds the charter on stdin, for a
different reason. Two of five vendors now require it; a third (Claude) tolerates
it. Argv is the exception, not the rule.

## D3 — Completion: the sentinel for intent, with a real envelope as the net

Copilot is the only vendor that ends its stream with an unambiguous "the run is
over" record:

```json
{"type":"result","sessionId":"…","exitCode":0,
 "usage":{"premiumRequests":1,"totalApiDurationMs":2919,"codeChanges":{…}}}
```

The ladder stays the shared one ([ADR-0023](./0023-shared-outcome-classifier.md)):

- `RALPHY_BLOCKED_EXIT <reason>` in the final `assistant.message` → `Blocked`.
- exit 0 **and** a HEAD-diff commit **and** `RALPHY_DONE_EXIT` → `Done`.
- the per-issue wall timeout → `Timeout`.
- anything else → `Stuck`.

The final assistant text is the last `assistant.message` with
`data.toolRequests: []` — the same shape as Kimi. Records carrying
`ephemeral: true` are the streaming deltas; **a parser that drops them loses
nothing.**

### `codeChanges` is a false friend and must not be read as progress

In probe P2 the agent created, staged and committed a file — HEAD advanced,
`git show --stat` confirms it — and the envelope still reported
`codeChanges: {linesAdded: 0, linesRemoved: 0, filesModified: []}`, because the
work went through the **shell** tool rather than the write tool. `codeChanges`
counts the vendor's own write-tool activity, not repository change. **The
HEAD-diff `committed` guard remains load-bearing exactly as for the other four
vendors.** This is the single most dangerous record in Copilot's stream and the
adapter must never consult it.

## D4 — Model is `Option<String>`, omitted when unset; omission means "the operator's current"

`model: Option<String>`, passed as `--model <id>` only when `Some`. This is the
OpenCode D4 shape ([ADR-0005](./0005-opencode-adapter.md),
[ADR-0010](./0010-settings-and-opencode-model-default.md)) reused verbatim,
including `resolve_opencode_model`'s precedence rule (flag → persisted → `None`).
It is **not** the Kimi/Codex shape of a hardcoded default id.

Two independent reasons, both evidenced:

1. **A hardcoded id is not portable across plans.** Model pinning is a plan
   entitlement. On a free account `--model` rejects *every* id including the
   ones the CLI itself routes to; on a paid account the same flag works. An
   adapter with a baked-in default works on one and hard-fails **every single
   run** on the other.
2. **Omission is the semantics the operator expects.** Probe P3 passed no
   `--model` and got the account's own default (`[INFO] Using default model:
   claude-sonnet-5`) with **no `session.auto_mode_resolved` event** — so
   omitting the flag is not a degraded fallback into auto mode, it is "whatever
   I have selected is what runs."

So the default posture is: **`--agent copilot` with nothing else runs the
operator's current model, for both plan and execute.** Pinning is available per
phase through the CLI flags that *already exist* — `--plan-model`,
`--exec-model` ([cli.rs](../../crates/ralphy-cli/src/cli.rs)) — and through a
persisted `CopilotSettings` section, mirroring `ClaudeSettings` field-for-field
but with `None` defaults instead of hardcoded `opus`/`sonnet`. **No new CLI
flags are introduced.**

## D5 — Effort is `Option<String>`, omitted when unset; never defaulted

`--effort <level>` only when `Some`. A fixed `medium` — the Codex
`DEFAULT_CODEX_EFFORT` shape — is **rejected**, because effort is not a
universal axis:

```
$ copilot … --model kimi-k2.7-code --effort medium
Error: Model "kimi-k2.7-code" does not support reasoning effort configuration.
exit = 1
$ copilot … --model kimi-k2.7-code          # same run, flag omitted
exit = 0
```

Four picker-enabled models — `kimi-k2.7-code`, `claude-haiku-4.5`,
`claude-sonnet-4.5`, `gemini-2.5-pro` — carry
`capabilities.supports.reasoning_effort: null` and fail pre-flight on every run
if the flag is sent.

This is **the same rule ADR-0005 D3 settled for OpenCode's `--variant`** —
*omitted when unset so the adapter never sends a value the provider rejects* —
reached independently from a different vendor's evidence. Two vendors, one rule;
it is now the house default for any optional passthrough knob.

## D5a — When effort *is* requested, the adapter clamps it; the vendor fallback inverts intent

Only the binary "does this model do effort at all" is validated by the CLI. An
out-of-range **level** is accepted, exits 0, and is silently dropped to the
**model's own default** — it is *not* clamped to the nearest supported level,
and this holds in both directions. Probe P6 against `gpt-5-mini`, which supports
exactly `low, medium, high`:

| requested | recorded in `assistant_usage_events.reasoning_effort` |
|---|---|
| `xhigh` — above the ceiling | **`medium`** |
| `minimal` — below the floor | **`medium`** |
| `high` — in range | `high` |

So **asking for `xhigh` on a model that stops at `high` yields *less* effort
than asking for `high`.** The operator asks for more, gets less, exit 0, nothing
in the stream mentions it. A passthrough adapter would inherit that inversion,
which is why effort gets normalised here rather than forwarded verbatim.

The ordering is `none < minimal < low < medium < high < xhigh < max`, and
`low`/`medium`/`high` are supported by **every** model that supports effort at
all. So a clamp that never exceeds the request is always satisfiable. The rule:

> Clamp the requested level to the nearest supported level **at or below** it.
> If nothing supported sits at or below, use the lowest supported level. If the
> model supports no effort at all, **omit the flag** (D5).

Stated as one rule rather than a list of special cases, it covers `xhigh → high`
and `minimal → low`, and it handles `claude-sonnet-4.6` — which has `max` but
*not* `xhigh` — by degrading to `high` rather than escalating to `max`. **The
clamp never buys more than was asked for**, so it can never surprise the
operator with cost.

### The support table is read, never hardcoded

The per-model `capabilities.supports.reasoning_effort` list comes from the live
CAPI catalog, **not** from a table baked into the adapter. Hardcoding it would
repeat precisely the mistake this spike documented: `copilot help config` lists
21 model ids under the `model` key and 8 of them are not selectable at any tier.
Vendor help text is not a contract, and a static table would go stale the same
way.

The catalog is obtainable for **zero model calls**: a run with a deliberately
invalid `--model` plus `--log-level all --log-dir` fails *after* the catalog
fetch and *before* any paid call, dumping all 46 entries verbatim. Since Ralphy
already performs a per-run auth preflight
([ADR-0013](./0013-run-auth-preflight.md)), this is not new machinery — it is
the existing preflight answering auth, entitlement, model catalog, effort
support and pricing in a single free subprocess, which is also ADR-0040's C4/C5
answer.

### Verification is post-hoc, because the request is not the truth

Even clamped, the adapter cannot assume the effort it asked for is the effort
that ran. `assistant_usage_events.reasoning_effort` is the only record of what
actually happened and is read after the fact (D10).

### Scope: this is a clamp, not a vocabulary

D5a normalises *within* the Copilot adapter. It deliberately stops short of
making `low|medium|high|xhigh` **Ralphy's** effort vocabulary, because effort is
currently an opaque passthrough in every adapter — Claude forwards the operator's
string, Codex (as of this ADR's writing) hardcoded `medium`, OpenCode's
`--variant` is documented as an opaque passthrough — and a normalised vocabulary
honoured by one vendor out of five is worse than none: the operator cannot tell
where the word means anything. Promoting effort to a core concept touches
`CONTEXT.md` and all five adapters and is tracked separately, so it does not sit
on this adapter's critical path. (Codex's freeze was later lifted — see Amendment
2026-07-23 on ADR-0004 and below.)

## Amendment (2026-07-23): flags feed `resolve_effort`/`clamp_effort`; D5a clamp unchanged

D5a's persisted-only composition (#227 open question) is lifted: `--plan-effort`/
`--exec-effort` now feed the existing `resolve_effort`/`clamp_effort` path at
`build_agent`, with persisted `copilot.*_effort` remaining as the fallback for
seven-rung extensions (`none`/`minimal`/`max`, ADR-0044 D6). The clamp
logic and the `clamp_lives_only_in_the_copilot_adapter` guard are unchanged —
only the composition-root wiring merges the resolved word ahead of the
persisted keys.

## D6 — No complexity routing in v1

`plan()` returns `recommended_model: None`. Neither routing axis survives
contact with Copilot:

- Routing to a **model id** is not portable — ids are plan-gated (D4), and the
  documented list in `copilot help config` demonstrably diverges from the
  entitled one.
- Routing to **effort** breaks on the four models that reject the flag (D5).

There is also a cost argument. `request_multiplier` is per-model and
**independent of the rate card**: `gemini-3.5-flash` billed **14 premium
requests** for one trivial call while costing *less* per token than
`claude-sonnet-5`. Cost cannot be inferred from the catalog, so an adapter that
picks models on the operator's behalf cannot reason about what it is spending.

Consequence for the prompt assets: the Copilot planning charter must **not**
emit the `## Execution model:` line, or the plan would promise a routing the
executor ignores. This reuses the existing per-adapter plan-prompt slot
mechanism — the same one that produced `prompt.plan.opencode.md`
([ADR-0005](./0005-opencode-adapter.md)) — rather than inventing a mechanism.

Deferred, not rejected: if routing is wanted later, the honest form is to route
**effort only, gated on the resolved model's catalog entry** — never blind.

## D7 — Blast radius is forced closed by default, asserted in-band, and escapable on purpose

Copilot ships capabilities that violate Ralphy's never-push / never-open-a-PR
ethos, **on by default**. The sharp edge is the bundled `github-mcp-server`,
which holds the operator's GitHub credential and lets an agent under a
`--allow-all-tools` charter **open a PR without ever shelling out to `git
push`** — bypassing every guard Ralphy has, all of which operate on the working
tree and the process boundary. That is not an extra capability; it is a route
around the product's ethos.

The adapter therefore always passes `--disable-builtin-mcps --no-remote
--no-remote-export --no-auto-update --no-ask-user`. Probes confirmed this costs
nothing functionally (exit 0, tool use, a real commit).

**The kill switch is verifiable, so it is verified.** `--disable-builtin-mcps`
does not merely omit the server, it emits a receipt:

```json
{"type":"session.mcp_servers_loaded",
 "data":{"servers":[{"name":"github-mcp-server","status":"disabled",
                     "source":"builtin","transport":"http"}]}}
```

The adapter **fails the run if it ever observes `status: "connected"`** for a
builtin server. No other vendor offers this; hardening elsewhere is asserted
only by the flags passed.

This is defaulted, not mandated. The operator's recorded posture is *ship the
strongest configuration as the recommended default, but opt-in — never deny
capability to the operator*. So `CopilotSettings` carries an explicit,
deliberately verbose escape hatch for the operator who genuinely wants the MCP
surface. Forcing it with no opt-out would have been the exception; it is not
needed, because the receipt makes the default honest rather than merely hopeful.

**Enforced** (#234): `crates/ralphy-agent-copilot/src/guards.rs` scans the run's
stdout after each phase and fails the run on a `connected` builtin server — and
on an ABSENT receipt too, since an unverifiable kill switch is not a verified
one; the escape hatch is the persisted
`copilot.allow_builtin_mcp_servers_i_understand_the_risk`, which drops
`--disable-builtin-mcps` from the argv and suppresses the failure together.
(The live receipt is `ephemeral: true` on every copy, so the scan must not reuse
the stream parser's ephemeral filter.)

## D8 — The three GitHub token env vars are scrubbed from the child

Copilot's precedence is `COPILOT_GITHUB_TOKEN` > `GH_TOKEN` > `GITHUB_TOKEN`.
Ralphy's own GitHub work runs through `gh`, and CI/automation contexts routinely
export `GH_TOKEN`/`GITHUB_TOKEN`. If any is set when Ralphy spawns Copilot, the
child **silently authenticates as that identity**, overriding the operator's
`copilot login`, and the run *succeeds under the wrong account*.

All three are removed from the child's environment. This is the direct analog of
the `ANTHROPIC_API_KEY` scrub, and worse in one respect: that failure is loud,
this one is silent. An operator who wants token-based auth sets it in
`CopilotSettings`, where the intent is recorded rather than inherited by
accident.

## D9 — Skills reuse the Codex pattern, targeting `.agents/skills`

**Enforced** (#235). The dance is shared as
`ralphy_adapter_support::{link_or_copy_dir, ensure_gitignore_entries, remove_path}`
— both adapters call it, and `skills::tests::the_dance_is_not_reimplemented_locally`
in the Codex crate reds if a local copy reappears. The load receipt is asserted by
`skills_load_violation` in `ralphy-agent-copilot/src/skills.rs`, reached through
the `CopilotAgent::check_skills_loaded` seam on both the plan and the execute path.

Copilot auto-discovers `.github/skills/`, `.agents/skills/` and
`.claude/skills/`, but **not** `.ralphy/skills` where Ralphy materializes. This
is exactly Codex's situation, and Codex already solved it: materialize into
`.ralphy/skills` via `materialize_assets`, then expose each skill into
`.agents/skills/<name>` by symlink with a Windows copy fallback, merging precise
per-entry lines into `.agents/skills/.gitignore` so user-owned sibling skills
survive and the tree stays clean for the next run's clean-tree check.

The adapter does **not** re-implement the PRIMITIVES. `link_or_copy_dir`,
`remove_path` and `ensure_gitignore_entries` were private to `ralphy-agent-codex`;
they are lifted into `ralphy-adapter-support` and both adapters call the shared
version. Two vendors needing the identical dance is the threshold for promoting
it out of a vendor crate.

The per-skill exposure **loop** deliberately stays in each adapter (#235): it is
~25 lines, and Copilot's diverges — it returns the exposed names so the load
receipt below has a required set, which Codex has no use for. That leaves the two
loops near-identical today, which is a known and accepted duplication: promote it
to a shared `expose_skills()` when a THIRD vendor needs it, or sooner if the two
start drifting in behaviour rather than in return type.

Rejected: materializing directly into `.agents/skills` — `materialize_assets`
does a clear-and-replace `remove_dir_all(dest_dir)` and writes a blanket `*`
gitignore, which would wipe the operator's own skills. Also rejected:
`copilot skill add`, which mutates global user state outside the repo.

Copilot then gives what Codex never had — a **load receipt**:
`session.skills_loaded` lists every discovered skill with its resolved path, so
the adapter asserts the Ralphy skills actually loaded instead of assuming it. The
live shape (`copilot 1.0.71`, 2026-07-20) is `data.skills[]`, each entry keyed
`name`; the record is `"ephemeral":true`, so — exactly as with D7's receipt — the
scan applies no `ephemeral` filter. Copilot injects its own skills into the same
array, so the guard checks each required name is PRESENT, never set equality.
`require_receipt` follows D7's split: an absent receipt fails closed only for a run
that exited cleanly, so a `Limit`/`Timeout` is never overwritten with
"skills receipt missing".

## D10 — Usage: mint the session id, read the store by primary key

The adapter generates a UUID and passes `--session-id <uuid>`, so ADR-0008 D10's
snapshot-diff is unnecessary — usage lookup is a primary-key read against
`assistant_usage_events` in `~/.copilot/session-store.db`. This is the OpenCode
topology (a database), not the Claude/Codex/Kimi file-tree one, so
`list_session_files` / `session_files_appeared` do not apply.

Rows are **incremental — sum them, do not keep-last**. `turn_index` is *not* a
per-call key (two distinct calls both carried `turn_index: 0`); only `id` is.
Field mapping needs no invention: `input_tokens→input`, `output_tokens→output`,
`cache_read_tokens→cache_read`, `cache_write_tokens→cache_creation`,
`model→model`. `reasoning_tokens` has no `Usage` slot and appears to be a subset
of `output_tokens`.

Reading discipline (ADR-0033's stateless-scan family): the store is WAL-mode, so
a reader must account for `-wal`/`-shm` — copying the `.db` alone returns an
empty store — and must never write to the live database.

Two currencies coexist: the stream reports **AI credits**
(`result.usage.premiumRequests`) while the database reports **tokens**. `Usage`
is token-denominated, so **the database is the source of truth** and the
envelope is a cross-check only. `token_details_json` carries the actual rate
card per call, which satisfies [ADR-0034](./0034-robust-read-time-pricing.md)
read-time pricing for free.

## D11 — Limits map to `Limit(None)` plus the synthetic cadence

No quota exhaustion was induced, so the surface of an exhausted limit is
unobserved: no semantic exit code equivalent to Kimi's 75 was found, no
`Retry-After`, no documented reset hint. The only defensible mapping is
`Limit(None)` with [ADR-0030](./0030-synthetic-reset-for-unschedulable-limits.md)'s
synthetic cadence. Claiming a reset time the vendor never gave would be worse
than admitting there isn't one.

One guard is not optional. Copilot's `continueOnAutoMode` config key silently
switches model and retries on eligible rate-limit errors — **a vendor-internal
retry that hides the limit from the caller**, the same failure mode that makes
OpenCode burn a full 60-minute timeout while reporting `saw_error = false`. It
defaults to `false`; the adapter asserts it stays false rather than trusting the
default.

**Enforced** (#234): `guards.rs`'s `continue_on_auto_mode_violation` reads the
vendor's GLOBAL config (`$COPILOT_HOME/config.json`, else
`<home>/.copilot/config.json` — which is JSONC, so line comments are stripped
before parsing) as a **preflight in `run_copilot`, before any child is spawned**,
so a violation costs no tokens. An absent or unparsable config is a pass: the
documented default is `false`, and failing every run over an unreadable
machine-managed file would trade one silent risk for a loud outage. The runtime
limit surface is unchanged — still `Limit(None)` plus ADR-0030.

**Capstone (#272, 2026-07-22).** The `continueOnAutoMode` preflight was confirmed to
stop a run *before any child spawns* (with the key set `true`, the D11 message won
over even the logged-out auth error). But the **real account-quota ceiling remains
unobserved**: `--max-ai-credits` is *agent-aware* — the model is told its remaining
budget and self-throttles rather than blowing the cap — and a session cap is a
different surface from account exhaustion. `is_copilot_limit_text` therefore stays
**class-validated** (its predicates unit-tested and proven disjoint from the auth
predicate) with the real wording still uncaptured; a maintainer ruling keeps it
deferred. One live billing note lands here: GitHub has moved to **AI credits** and
`premiumRequests` is the *legacy* platform — `is_copilot_limit_text` already matches
`out of ai credits`, so the class matcher is forward-compatible with the new surface.

## D12 — `ACCEPTS_IMAGES` is true

`--attachment <path>` is verified end-to-end: a real PNG plus a prompt asking
for the most prominent word in the image returned a word that exists only in the
image's pixels. The flag is documented "only valid in non-interactive mode",
which is precisely Ralphy's mode, and may be repeated. Vision is near-universal
in the catalog — true for every picker-enabled model but one.

**Enforced** (#236, #237): `build_copilot_command` (`command.rs`) takes an
`images: &[PathBuf]` parameter and emits `--attachment <path>` once per entry;
an empty slice emits nothing. Both production `plan`/`execute` call sites pass
`&[]` — Copilot runs no interactive triage of its own. The triage path now
feeds the channel: `ralphy-cli/src/triage.rs` → `TriageRequest::image_paths` →
`ralphy_agent_copilot::triage_issues` (`tasks.rs`) →
`build_copilot_init_command` (`command.rs`), which forwards `images` straight
into `build_copilot_command`.

## What this ADR deliberately does not decide

- **Hooks** as a deterministic completion signal. Event names and payload schema
  are undocumented and unexercised. The sentinel already works (D3), so this is
  an optimisation with no forcing function.
- **Native plan mode** (`--plan`, `--mode plan`). The other four vendors all
  rejected native plan mode; no evidence yet that Copilot's differs enough to
  revisit.
- **Background tasks** outliving process exit. `session.background_tasks_changed`
  fired six times in one probe and no disabling flag was found. Ralphy's
  per-issue budget assumes the process boundary is the run boundary; if that
  assumption is false, it is false for the budget, not for the adapter's shape.
- **The `COPILOT_PROVIDER_*` BYOK family.** Setting `COPILOT_PROVIDER_BASE_URL`
  bypasses GitHub auth and model routing entirely — and with them every
  assumption in this document. Out of scope, not supported.
