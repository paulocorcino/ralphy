# Configuration reference

Every knob Ralphy persists, what it defaults to, and where it lives. This page
is the complete map of `ralphy config`; the design rationale is in
[ADR-0010](./adr/0010-settings-and-opencode-model-default.md) (settings store),
[ADR-0011](./adr/0011-verify-gate-before-close.md) (verify gate),
[ADR-0015](./adr/0015-deterministic-protocol-gate.md) (`require_verify_gate`),
[ADR-0019](./adr/0019-cloudevents-event-sink.md) (events sink), and
[ADR-0021](./adr/0021-assignee-scoped-queue.md) (assignee filter).

## The one rule: precedence

Every setting resolves the same way, and this order never changes:

> **per-run flag > `settings.json` > built-in default**

A flag always wins for a one-off; a persisted setting is the per-repo default
you stop retyping; the built-in default is what you get out of the box. An empty
string (`""`) on either the flag or the setting is treated as **unset** and falls
through to the next slot — so a blank value never accidentally overrides a default.

## Where settings live

Not everything lives in the same file. There are three stores, by concern:

| Store | Path | Scope | Holds |
| --- | --- | --- | --- |
| **Repo settings** | `<repo>/.ralphy/settings.json` | Per repo, **gitignored** | Everything except the two below |
| **Events sink** | `~/.ralphy/events.toml` | Global, keyed by `owner/repo` slug | `events.url`, `events.token` |
| **Telegram** | Its own global TOML | Global | The Telegram monitor config (`ralphy telegram …`, not `ralphy config`) |

The events sink deliberately lives **outside** `settings.json` so the endpoint
travels with you across every repo and never lands in a per-repo file. It is
still keyed by repo slug, so different repos can post to different endpoints.

`settings.json` tolerates and round-trips **unknown keys** — an older binary
never drops a key a newer one wrote, so the file is forward-compatible.

## Managing settings

```powershell
ralphy config set <key> <value>   # persist a key
ralphy config get                 # print every persisted value (token masked)
ralphy config unset <key>         # clear a key back to its default
```

All three take `--repo <path>` (default: the current directory, resolved to its
git top level). Setting any repo key also ensures `.ralphy/` is gitignored.

## Agent-agnostic keys

These apply to every agent (`--agent claude`/`codex`/`opencode`).

| Key | Flag | Values | Default | Meaning |
| --- | --- | --- | --- | --- |
| `base_branch` | `--base-branch` | any git ref | `origin/main` | The base the run branch is cut from (`new` mode). |
| `branch_mode` | `--branch-mode` | `new` \| `current` | `new` | `new` cuts a fresh `afk/run-<stamp>` branch; `current` commits onto the branch you're on. Both require a clean tree. |
| `remote_control` | `--remote-control` / `--no-remote-control` | `true` \| `false` | `false` | Opt into Claude mobile Remote Control (follow/intervene). Codex/OpenCode ignore it. |
| `queue.assignee` | `--assignee` / `--no-assignee` | a GitHub login, or `@me` | none (no filter) | Build the queue only from issues this login is assigned to. `@me` = the authenticated user. `--only-issue`/`--issues` ignore it. |
| `verify.command` | — | one command line | none | The fallback verify gate, used only when a plan has **no** `## Verify` section. Tokenized into argv and run directly (no shell). See [Verify gate](#the-verify-gate). |
| `verify.require_verify_gate` | — | `true` \| `false` | `false` | When `true`, an issue that resolves to **no gate at all** is parked as `ready-for-human` and left open instead of closing on the agent's self-report. |

```powershell
ralphy config set base_branch origin/develop
ralphy config set branch_mode current
ralphy config set remote_control true
ralphy config set queue.assignee @me
ralphy config set verify.command "cargo test"
ralphy config set verify.require_verify_gate true
```

## Claude run defaults (`claude.*`)

The model/effort/budget knobs are **Claude-only today** (a Codex/OpenCode
equivalent is deferred — Codex has no persisted model/effort defaults yet, and
OpenCode's model lives under `opencode.model` below). Each is `None` out of the
box, leaving the hardcoded run default in place.

| Key | Flag | Values | Default | Meaning |
| --- | --- | --- | --- | --- |
| `claude.plan_model` | `--plan-model` | `opus` \| `sonnet` \| … | `opus` | Model the planner uses. |
| `claude.plan_effort` | `--plan-effort` | `low` \| `medium` \| `high` \| … | `medium` | Reasoning effort while planning. |
| `claude.default_exec_model` | `--default-exec-model` | `sonnet` \| `opus` | `sonnet` | Execution model used **only when the plan emits no `## Execution model` judgment** (complexity routing). An explicit `--exec-model` or the plan's own judgment overrides it. |
| `claude.exec_effort` | `--exec-effort` | `low` \| `medium` \| `high` \| … | `medium` | Reasoning effort while executing. |
| `claude.max_minutes_per_issue` | `--max-minutes-per-issue` | non-negative integer | `60` (finite backstop) | Per-issue wall-clock cap in minutes. **`0` disables the cap** — the issue is then bounded only by `--deadline-hours`. |

```powershell
ralphy config set claude.default_exec_model opus
ralphy config set claude.plan_effort high
ralphy config set claude.max_minutes_per_issue 90   # opt into a 90-min cap
ralphy config set claude.max_minutes_per_issue 0    # explicit opt-out: unbounded
```

> The per-issue model choice is resolved as: explicit `--exec-model` > the plan's
> `## Execution model: sonnet|opus` judgment > `claude.default_exec_model`. This
> is why `default_exec_model` only bites when the planner declined to route.

## OpenCode model default (`opencode.*`)

| Key | Flag | Values | Default | Meaning |
| --- | --- | --- | --- | --- |
| `opencode.model` | `--exec-model` | any model id OpenCode offers | none | The persistent OpenCode execution model. When unset, OpenCode resolves its own default. |

```powershell
ralphy config set opencode.model kimi-for-coding/k2p7
ralphy models --agent opencode   # list the models OpenCode offers
```

Resolution: `--exec-model` (per-run) > `opencode.model` (persisted) > omit `-m`
so OpenCode picks its own. The model that **actually** ran is read back into the
usage ledger, so the ledger is always truthful even when you let OpenCode decide.
OpenCode effort is set per-run with `--exec-variant` (not persisted).

## Copilot run defaults (`copilot.*`)

| Key | Flag | Values | Default | Meaning |
| --- | --- | --- | --- | --- |
| `copilot.plan_model` | `--plan-model` | any model id Copilot offers | none | The persisted planning-phase model. When unset, `--model` is omitted (ADR-0041 D4). |
| `copilot.exec_model` | `--exec-model` | any model id Copilot offers | none | The persisted execution-phase model. When unset, `--model` is omitted (ADR-0041 D4). |
| `copilot.plan_effort` | none | `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` | none | The reasoning effort *requested* for the planning phase. When unset, `--effort` is omitted (ADR-0041 D5). |
| `copilot.exec_effort` | none | same | none | The reasoning effort *requested* for the execution phase. When unset, `--effort` is omitted (ADR-0041 D5). |
| `copilot.allow_builtin_mcp_servers_i_understand_the_risk` | none | `true`, `false` | `false` | **Danger.** The D7 escape hatch: drops `--disable-builtin-mcps` from the argv AND suppresses the connected-server failure. See below. |

```powershell
ralphy config set copilot.exec_model gpt-5
ralphy config set copilot.exec_effort high
```

Resolution per phase: `--plan-model`/`--exec-model` (per-run) > `copilot.plan_model`/
`copilot.exec_model` (persisted) > omit `--model` — an omitted `--model` runs the
account's own current selection, the correct default rather than a degraded
fallback (ADR-0041 D4).

The two effort keys have **no per-run flag**: they are persisted-only, because
whether Ralphy's `--plan-effort`/`--exec-effort` become valid for every adapter is
still open (#227).

### The builtin-MCP escape hatch

By default Ralphy passes `--disable-builtin-mcps` and then *verifies* it in band:
Copilot's `session.mcp_servers_loaded` receipt must report every builtin server
off, and a **missing** receipt fails the run too (fail closed — an unverifiable
kill switch is not a verified one, ADR-0041 D7).

Setting `copilot.allow_builtin_mcp_servers_i_understand_the_risk` to `true` does
both halves of the opposite: it drops `--disable-builtin-mcps` from the argv *and*
suppresses the connected-server failure. Suppressing only the check would grant
you nothing.

What you are handing back: Copilot's bundled GitHub MCP server holds **your**
GitHub credential, so an agent that reaches it can open a pull request without
ever running `git push` — outside the branch-and-hand-over discipline every other
part of Ralphy enforces. The key name is deliberately long: length is the safety
feature, so it cannot be set by accident. It is persisted-only, with no per-run
flag, for the same reason.

**Effort is a request, not an instruction.** Copilot's effort vocabulary is
per-model — the catalog publishes each model's own supported list, and a level
outside it is rejected. So Ralphy clamps the requested level DOWN to the greatest
level the phase's model actually supports, never up: `xhigh` on a model offering
`low`/`medium`/`high` is sent as `high`, and on a model offering
`low`/`medium`/`high`/`max` it is *still* sent as `high` rather than escalating to
`max` (ADR-0041 D5a). A model that takes no effort argument at all never receives
the flag, however loudly it was requested; the same holds when the catalog is
unavailable or the pinned model is unknown to it — the flag is omitted and the
model's own default decides. After the phase runs, Ralphy compares the request
against the level the vendor actually recorded in its session store and logs a
warning on a divergence.

The one direction that is *not* downward is a request below the model's floor:
every effort-capable model Copilot publishes today starts at `low`, so
`none` or `minimal` is raised to that floor rather than omitted (ADR-0041 D5a's
"nothing supported at or below the request → use the lowest supported level").
If you want the model's own default, leave the key unset.

## Cursor run defaults (`cursor.*`)

| Key | Meaning |
| --- | --- |
| `cursor.allow_codebase_indexing_i_understand_the_risk` | **Danger.** Opts back into Cursor's default behavior of uploading the enclosing repository to its servers (ADR-0042 D6). Off by default: Ralphy refuses to spawn `cursor-agent` in a repository lacking `.cursorindexingignore`, and never writes that file for you. |

```powershell
ralphy config set cursor.allow_codebase_indexing_i_understand_the_risk true
```

### Skills land with no flag, env var or manifest

Unlike Copilot (`.agents/skills`), Cursor auto-discovers `SKILL.md` files
recursively under several roots, and Ralphy materializes its bundled skills
into `<repo>/.cursor/skills/` on every run — no `--plugin-dir`, no environment
variable, no `.cursor-plugin/plugin.json` manifest (ADR-0042 D12). This is the
cheapest skill delivery of any vendor Ralphy drives.

Because every run executes against an isolated, scratch `CURSOR_CONFIG_DIR`
(D17), the operator's own `~/.cursor/skills/` — Cursor's OWN user-level root,
which the vendor resolves relative to `CURSOR_CONFIG_DIR` — is **not visible**
to a Ralphy run. Only the repository-local root is read.

### The foreign harvest is real, and it is not suppressed

Cursor's skill discovery is not scoped to `.cursor/skills` alone: it also
walks `.claude/skills`, `.codex/skills` and their `~/` equivalents — with no
CLI-side allowlist. A trivial "reply OK" probe against an account with a
personal Claude Code skills library measured **78 foreign skills** injected
into a single request, at a cost of **18 212 input tokens** for that one
call (ADR-0042 D12; spike §8 Phase 4).

Isolating `HOME`/`CURSOR_CONFIG_DIR` further would suppress the harvest, but
it would also isolate the vendor credential, forcing a second login — a worse
trade for the operator than the token cost. Ralphy documents the behavior
here and does not fight it. **D17's isolation covers only Cursor's OWN root,
not the foreign ones**: unlike `~/.cursor/skills/` above, the foreign roots
(`.claude/skills`, `.codex/skills` and their `~/` equivalents) are resolved
from the repository path and the real `HOME`, never from `CURSOR_CONFIG_DIR`
— so scoping `CURSOR_CONFIG_DIR` narrows what Cursor treats as *its own*
skills, and has no effect on what it harvests from *other* vendors.

Practical consequence: a per-issue token budget tuned against another vendor
(one with no foreign-skill harvest) reads wrong for Cursor — expect materially
higher input-token floors on this vendor, independent of the task.

## Gemini run defaults (`gemini.*`)

| Key | Flag | Values | Default |
| --- | --- | --- | --- |
| `gemini.plan_model` | `--plan-model` | any id in `ralphy_agent_gemini::PINNABLE_MODELS` | none — the vendor routes |
| `gemini.exec_model` | `--exec-model` | any id in `ralphy_agent_gemini::PINNABLE_MODELS` | none — the vendor routes |

```powershell
ralphy config set gemini.plan_model gemini-2.5-pro
ralphy config set gemini.exec_model gemini-3.5-flash
```

### The router tax

Leave both unset and Ralphy omits `-m` entirely, which is not free: the CLI then
asks a **router** which engine should serve the turn, and that question is itself
a billed API call (the spike observed `gemini-3.1-flash-lite` in the
`utility_router` role beside the answering engine). Every turn therefore costs
**two requests instead of one**. Pinning a model per phase removes the routing
call — which is why `ralphy init` prints the note when it finds this CLI
installed (ADR-0043 D8).

Validation is applied at `config set` time only. A `--plan-model`/`--exec-model`
flag is passed through unfiltered on purpose: the vendor's id set is mutable by
server-side experiment flags, so a stale local list must never block an id the
CLI has just started serving. If the id really is unknown, the vendor answers
`ModelNotFoundError … { code: 404 }` and the adapter turns it into a named stop
quoting the id you asked for, not an unexplained failure.

A run left unpinned is recorded under the model key `gemini-routed`, which
carries **no price row** — a routed run reports an unpriced model rather than
being attributed to an engine it may never have used.

## Events sink keys (`events.*`)

Stored in the **global** `~/.ralphy/events.toml`, not `settings.json`. See
[the event contract](./events.md) for the payload and
[Enabling the sink](./events.md#enabling-the-sink) for a walkthrough.

| Key | Values | Default | Meaning |
| --- | --- | --- | --- |
| `events.url` | an HTTPS endpoint | none (sink off) | Where CloudEvents are POSTed. Absent → no events emitted. |
| `events.token` | a bearer token | none | Sent as `Authorization: Bearer <token>` when set. `config get` masks it; the `RALPHY_EVENTS_TOKEN` env var overrides the stored token for a single run. |

```powershell
ralphy config set events.url https://example.com/hook
ralphy config set events.token s3cret        # stored masked, echoed masked
```

## The verify gate

Before closing an issue, Ralphy re-runs a set of commands over the committed code
and only closes if they pass ([ADR-0011](./adr/0011-verify-gate-before-close.md)).
Resolution precedence, strongest first:

1. **`## Verify` in the plan** — per-issue, planner-emitted, one command per line.
2. **`verify.command`** — the per-repo fallback, used when the plan has no
   `## Verify` section.
3. **Nothing resolves** — the issue closes on the agent's self-report with a
   **loud warning**… unless `verify.require_verify_gate` is `true`, in which case
   the issue is instead parked as `ready-for-human` and left open.

`## Verify: none` on its own line in a plan is the only explicit opt-out (for an
issue with nothing machine-verifiable), and it skips the `verify.command`
fallback too.

## Inspecting the current state

```powershell
ralphy config get
```

Prints one `key = value` (or `key: not set`) line per key, including the
events-sink values for the current repo's slug with the token masked. This is the
fastest way to see exactly what a run will inherit before the built-in defaults
apply.

## See also

- [Getting started](./getting-started.md) — first steps on a fresh repo.
- [Event contract](./events.md) — the CloudEvents payload, field by field.
- [Scheduling](./scheduling.md) — running unattended on a timer.
