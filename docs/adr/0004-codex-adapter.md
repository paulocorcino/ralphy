# The Codex adapter: a per-run peer of the Claude adapter, native to `codex exec`

Ralphy gains a second agent CLI vendor — OpenAI's `codex` — as a new isolated
crate `ralphy-agent-codex` that implements the same PTY-free `Agent` trait
(docs/adr/0002). The adapter is selected **per run** by a `--agent claude|codex`
flag; the core keeps taking a single `&dyn Agent` and never learns which vendor
it holds. The only surface shared between the two adapters is the core's `Agent`
trait and `Outcome` enum — there is deliberately **no** shared "headless runner"
that both bend to fit (the boundary invariant, whose home is ADR-0002). Each
adapter is built to its vendor's best-fit mechanism,
even where that makes the two internally divergent, because the only thing that
must match is the `Outcome` the core receives, not how it was produced.

This is grounded in a working prior-art runner the operator already built against
`codex-cli 0.138.0` (`run_backlog_codex.py`), which proved the invocation and the
detectable completion signals below.

## D1 — Selection is per run, via `--agent`; the core is untouched

`main.rs` matches `--agent` and boxes the right adapter
(`Box<dyn Agent>`); `run_queue(&cfg, &queue, agent.as_ref(), …)` is unchanged.
We rejected per-issue routing (a label like `agent:codex` dispatched inside the
core) and a global env/config switch: per-run is the smallest surface, keeps the
`--agent` choice out of the core, and matches ADR-0002's stance that an adapter is
the isolated unit. Mixing vendors within one run buys nothing the operator asked
for and would force a router into the core.

## D2 — Completion is detected by Codex-native signals, not a Stop hook

The Claude adapter detects completion from a Stop-hook flag file over a live PTY
session. Codex has no such hook and does not need an interactive session, so the
Codex adapter runs headless:

```
codex exec -C <root> -m <model> -c model_reasoning_effort="<effort>" \
  -s danger-full-access -a never -o .ralphy/codex-last.txt -
```

It maps Codex's own signals onto the same core `Outcome`:

- exit 0 + `RALPHY_DONE_EXIT` in the `-o` final message → `Done`;
  `RALPHY_BLOCKED_EXIT <reason>` → `Blocked`.
- no new commit (`HEAD` unchanged) across the streak, or a non-zero exit → `Stuck`.
- a usage-limit match (D6) → `Limit`.
- the orchestrator's per-issue wall timeout → `Timeout`.

The exit-code + `HEAD`-diff progress check is the same idea the Claude headless
loop already uses (`headless_step`) and the prior-art script proved; the `-o`
sentinel read is the minimal addition that keeps the execution charter identical
across vendors. We did **not** reuse Claude's PTY, Stop hook, flag file, or
workspace-trust shim — none apply to `codex exec`, and importing them would be the
compatibility-shaped code this design avoids.

## D3 — Complexity routing is reasoning effort, not a model swap

_Superseded by the Amendment (2026-07-10) below: the Codex 5.6 family inverted
this decision's premise, and the tier now routes to a model._

Claude routes complexity by swapping models (`sonnet`/`opus`). Codex scales by a
**reasoning-effort** knob on one coding model, so the Codex adapter expresses the
same routing as effort: a fixed, operator-parametrizable model (defaulting to the
most recent), with `model_reasoning_effort` set via `-c`. Planning always runs at
`high`; execution takes the plan's neutral complexity tier (low/medium/high). The
plan's `## Execution model` line therefore emits a vendor-neutral tier for Codex
rather than a Claude model name, translated to effort at a single point inside the
adapter — the mirror of ADR-0002's single tier↔model point in the Claude adapter.
Forcing a two-model swap onto Codex to imitate `sonnet`/`opus` was rejected as
imitation of the wrong vendor's shape.

## D4 — Reuse the skill content; re-target the delivery, not the files

The `reviewer` and `staged-plan` skills are already directories of `SKILL.md` +
`scripts/`/`references/`/`templates/`, which is almost exactly Codex's own skill
layout (`.agents/skills/<name>/SKILL.md`, frontmatter `name` + `description`).
The Codex adapter therefore materializes the **same skill content** into
`.agents/skills/` (auto-discovered by `codex exec`), instead of the Claude plugin
form (`.claude-plugin/plugin.json` under `.ralphy/plugin/`, passed via
`--plugin-dir`). Two prompt spots are Claude-isms and get Codex variants: the
`## Execution model` tier line (D3), and the reviewer self-review step, whose
"spawn the reviewer skill as an independent subagent" assumes Claude's Task tool
and must be rephrased to Codex-native subagent dispatch. All existing Claude
assets stay untouched.

Because Codex has no setting to point at a private skills directory — it only
scans the conventional `.agents/skills` hierarchy, and `[[skills.config]]` merely
toggles a skill on/off — that directory is a **user-owned, shared** location we must
not wipe. So the adapter splits storage from exposure: the real skill content is
materialized into ralphy's own `.ralphy/skills` store (cleared-and-replaced
wholesale, like the OpenCode path, and kept out of git by `.ralphy/.gitignore`), and
only **per-skill symlinks** are placed into `.agents/skills/<name>` — additively,
replacing just the `reviewer`/`staged-plan` entries ralphy owns and leaving any
sibling user skills intact. On Windows, where a symlink needs Developer Mode/admin,
the link falls back to a recursive copy. A **merged** `.agents/skills/.gitignore`
adds a `/<name>` line per ralphy skill without overwriting the user's own entries.
`.agents/skills` is preferred-and-reused when it exists, else created; `.codex` and
`.claude` are not used because `codex exec` does not discover skills there.

## D5 — Subscription auth is the operator's; security is the isolated branch

Codex bills against the ChatGPT subscription when signed in via `codex login` and
switches to API billing if `OPENAI_API_KEY` is present. The operator owns the
login (CLI-only, no API), so the adapter manages no provider key — unlike the
Claude path, which clears `ANTHROPIC_API_KEY` in `main.rs`. The execution sandbox
posture follows the prior art: `-s danger-full-access -a never` (full autonomy,
no sandbox), with safety resting on Ralphy's existing net — every issue commits
onto an isolated run branch a human merges by hand, plus the reviewer self-review.
The Claude `PreToolUse` guard is not ported onto the Codex path.

## D6 — A usage limit auto-resumes when Codex names a reset time; otherwise stops

**Superseded.** This decision originally stopped-and-reported on every Codex limit
(`effective_stop_on_limit` forced `true` for Codex), on the premise that Codex's
reset hint was never trustworthy. That premise was wrong: when Codex *does* emit a
`try again at <datetime>`, the datetime is an **absolute RFC3339 instant**
(`2026-06-09T18:00:00Z`) — its own date and zone, no next-occurrence guess. That is
*more* trustworthy than Claude's relative `"resets 3pm"`, not less. So Codex now
follows the same default as Claude (ADR-0003 D1): auto-resume, with `--stop-on-limit`
as the opt-out.

The split is the reset hint, not the vendor. The adapter matches the limit text
(`you've hit your usage limit`, `usage limit`, `rate limit reached`) to
`Outcome::Limit`, and extracts the absolute `try again at <datetime>` only when one
is present. The core's `next_reset` parses that RFC3339 instant directly and waits
for it. When Codex emits only "wait for limits to reset (every 5h and every week)"
with no near-term time, the hint is `None` → `Outcome::Limit(None)` → ADR-0003's "no
parseable reset → stop" fallback still fires. Blocking for hours on a *guessed* wake
time — the original concern — never happens, because the only thing waited on is an
explicit absolute instant.

## Consequences

- The core, `ralphy-agent-claude`, the existing prompts/plugin, `hook.rs`,
  `guard.rs`, and the `ANTHROPIC_API_KEY` clearing all stay untouched; the only
  core-side change is the `--agent` match in `main.rs`. No-regression for Claude
  is structural, not tested-in.
- `plan::count_open_steps` is vendor-neutral and reused; the Codex adapter reads
  the neutral tier out of `Plan.recommended_model` and maps it to effort, so the
  core `Plan` shape is unchanged.
- Two items are deferred until observed against a live Codex run, neither
  blocking: the exact shape of a `try again at <datetime>` reset (firm up the
  parser then; until then `Limit(None)` + stop), and whether the reviewer
  subagent is best dispatched as `$reviewer` or a `.codex/agents/reviewer.toml`.
- A defensive option remains open: `.env_remove("OPENAI_API_KEY")` on the Codex
  child `Command` to prevent an inherited key from silently switching the run to
  API billing.

## Amendment (2026-07-10): the Codex 5.6 family inverts D3 — the tier routes to a model

D3's premise — "Codex scales by a reasoning-effort knob on **one** coding model" —
no longer describes the vendor. Codex 5.6 ships a three-model family positioned by
weight, the same shape ADR-0002 codified for Claude: `gpt-5.6-sol` (flagship, for
complex/high-value work), `gpt-5.6-terra` (balanced everyday model), `gpt-5.6-luna`
(fast/affordable, for clear and repetitive work). The old fallback `gpt-5-codex` is
gone from the catalog entirely (even `gpt-5.3-codex` is deprecated for ChatGPT-auth
accounts), so `DEFAULT_CODEX_MODEL = "gpt-5-codex"` became a known-invalid
configuration. Routing complexity by model swap is now the vendor's **own** shape;
D3's rejection of it as "imitation of the wrong vendor's shape" is inverted, not
merely renamed.

**Decision.** The plan's neutral `## Execution model: low|medium|high` tier now
routes to a **model**, and `model_reasoning_effort` is held at the vendor default
(`medium`) instead of being derived from the tier:

| role                   | Claude (ADR-0002) | Codex                     |
| ---------------------- | ----------------- | ------------------------- |
| planning               | opus              | `gpt-5.6-sol`, effort medium |
| execute, tier `high`   | opus              | `gpt-5.6-sol`             |
| execute, tier `medium` | sonnet            | `gpt-5.6-terra`           |
| execute, tier `low`    | sonnet            | `gpt-5.6-luna`            |

- **One axis, not two.** The tier chooses the model; effort stays a single global
  operator override (opt-up to `high`/`xhigh`, the latter model-dependent), never
  part of the tier mapping. A free tier→(model, effort) matrix was rejected as
  configuration surface without evidence. This also supersedes D3's "planning
  always runs at `high`": planning runs on Sol at default effort, aligning with
  the vendor's own "start at the default effort, raise only when needed" guidance.
- **Luna on tier `low` is the vendor's positioning**, and matches the tier's own
  definition ("mechanical, localized, well-understood"). Routing `low` to Terra
  was considered as the conservative alternative and stays available as the
  fallback if live runs show Luna under-delivering.
- **Resolution order is unchanged**: `--exec-model` override → the user's
  `config.toml` `model` → the routing table above. Where no tier exists (the
  one-shot `init` sessions, ADR-0012/0025), the adapter instead **omits `-m`**
  when neither override nor config names a model, delegating to Codex's own
  recommended default.
- **Pinned `gpt-5.6-*` names re-create the obsolescence this amendment fixes.**
  The mitigation is structural, not nominal: the family lives in a single
  constants table in the adapter's command module, so a generation change is a
  three-line diff plus a new amendment — never a hunt across the repo. The
  ChatGPT-subscription cost of heavier routes shows up as faster usage-limit
  burn (`Outcome::Limit`), not billing, so mis-calibration is observable in runs.
- `## Execution model:` and `Plan.recommended_model` keep their names: the tier
  now selects a model in fact, so both are honest again and no public-API rename
  is needed.
- The deferred Consequences item on `$reviewer` vs `.codex/agents/reviewer.toml`
  is updated: custom agents in `.codex/agents/*.toml` are now officially
  supported, but the choice **remains deferred** pending a measured benefit over
  the default subagent + auto-discovered skill. (`.agents/skills` remains the
  skills surface — D4 is unaffected.)

**Live validation (2026-07-10, codex-cli 0.144.1).** A headless spike ran each
route through the exact invocation this ADR fixes (`codex exec -C <neutral dir>
--skip-git-repo-check -m <model> -s read-only -`, prompt on stdin):

- `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` all complete with exit 0; the
  banner (`model: …`) confirms the requested model is what runs.
- `gpt-5-codex` fails hard: HTTP 400 `"The 'gpt-5-codex' model is not supported
  when using Codex with a ChatGPT account"`, preceded by a "model metadata not
  found" warning — the old `DEFAULT_CODEX_MODEL` is confirmed dead, not merely
  deprecated.
- With `-m` omitted, the CLI resolves the user's `config.toml` `model` first;
  with no config at all its built-in default is `gpt-5.6-sol` (resolved
  client-side, visible in the banner). Omitting `-m` on the init one-shots is
  therefore safe.
- `-c model_reasoning_effort="medium"` takes effect and **overrides** the
  user's `config.toml` effort (observed against a config pinning `low`).
  Pinning `medium` per invocation is thus a deliberate override of the
  operator's interactive default, not a complement to it.
- The invocation surface is unchanged in 0.144.1: `exec`, `-C`, `-m`, `-c`,
  `-s`, `-o`, `-i`, stdin `-`, `--skip-git-repo-check` all behave as D2 fixed.
- Two observational cautions: the model's *self-reported* slug is unreliable
  (Sol answered "gpt-5.3-codex"; Terra/Luna answered "gpt-5") — the banner and
  the session rollout are the only truth about which model ran (the rollout is
  what ADR-0008 usage tracking already reads, so `usage` stays faithful). And
  `config.toml` now has a native `plan_mode_reasoning_effort` key; it governs
  Codex's own interactive plan mode, not ralphy's plan charter (which is an
  ordinary `exec` run), so it does not interact with this routing.

## Amendment (2026-07-23): operator `--plan-effort`/`--exec-effort` set `model_reasoning_effort`

The 2026-07-10 amendment held `model_reasoning_effort` at the vendor default
(`medium`) and treated effort as frozen relative to the tier→model routing.
That freeze is lifted for the operator's Effort flags (#286 / ADR-0044 D7).

**Decision.** `--plan-effort` / `--exec-effort` now set `model_reasoning_effort`
on plan and execute `codex exec` invocations. When unset, the default remains
`medium`. Effort stays orthogonal to tier→model: the tier still picks Sol /
Terra / Luna; effort only sets how hard the chosen model thinks. Init/triage
one-shots keep `DEFAULT_CODEX_EFFORT`. Amends D3's frozen-effort clause and
the 2026-07-10 `held at the vendor default` wording for run plan/execute.

## Amendment (2026-07-24): the execute tier is one cost/power ladder — model *and* default effort — with a new `xhigh` rung

The 2026-07-10 amendment routed the tier to a model and froze effort at the
vendor default; the 2026-07-23 amendment let the operator's `--exec-effort`
move effort but kept it **orthogonal** to the tier. That orthogonality left the
planner unable to ask for the thing operators actually wanted: the flagship
*thinking harder*. Sol at tier `high` ran at `medium` effort, and the only way
to reach Sol at `high` effort was an operator flag applied uniformly to every
issue in the run — not a per-issue judgment the plan could make from the work in
front of it.

**Decision.** For the **execute** phase, the neutral complexity tier now selects
a single point on a `(model, effort)` cost/power ladder, and a fourth rung
`xhigh` is added so the planner can reach Sol at `high` effort per issue:

| tier (neutral) | model           | default effort |
| -------------- | --------------- | -------------- |
| `low`          | `gpt-5.6-luna`  | low            |
| `medium`       | `gpt-5.6-terra` | medium         |
| `high`         | `gpt-5.6-sol`   | medium         |
| `xhigh`        | `gpt-5.6-sol`   | high           |

- **Luna stays on `low`.** Dropping Luna for Terra:low was considered and
  rejected: Terra costs 2.5× Luna per token (the seeded floor: Sol 41.75, Terra
  20.875, Luna 8.35), and `low` is *defined* as mechanical, localized,
  well-understood work — the territory where the cheap model has the least
  downside. No measured Luna failure rate justifies the 2.5× on the most common
  trivial-task tier. The conservative Terra:low fallback named in the 2026-07-10
  amendment remains available if live runs show Luna under-delivering.
- **The ladder is monotonic in both axes** — each rung is ≥ the previous in
  model weight and effort — so it reads as one "how much power does this issue
  deserve" dial, not a free tier×effort matrix (still rejected as unearned
  configuration surface). `high` → Sol:medium and `xhigh` → Sol:high are the two
  flagship rungs the original request asked for.
- **Effort precedence is unchanged in spirit, refined in the default.** The
  order is `--exec-effort` override → the **tier-derived** effort above (was:
  the flat `medium`). An explicit operator flag still wins on every issue — the
  operator is never denied (the opt-in posture). Only the *unset* default moved
  from flat `medium` to per-tier. `--exec-model` still short-circuits the model
  column exactly as before (override → `config.toml` model → the table).
- **Plan phase is untouched.** Planning runs before any tier exists, so it keeps
  running on Sol at `--plan-effort` (default `medium`, `DEFAULT_CODEX_EFFORT`).
  Init/triage one-shots likewise keep `DEFAULT_CODEX_EFFORT`. This amendment
  governs the **execute** routing only.
- **`xhigh` is the neutral-lexicon rung (ADR-0044), not a Codex value.** Codex's
  `model_reasoning_effort` accepts `minimal|low|medium|high`; the `xhigh` tier
  therefore maps to the argv effort `high` at the single routing point — the
  neutral word names the *rung*, the concrete word `high` is what reaches
  `codex exec`. The plan charter's `## Execution model` line gains `xhigh` as a
  fourth accepted value (codex overlay only; other vendors are unchanged).
- **Direct model selection is unaffected.** `--exec-model gpt-5.6-luna` (or any
  id) bypasses the ladder entirely, so an operator invoking a model by hand gets
  exactly it — the ladder is the *auto-routing* default, not a cage.

This supersedes the 2026-07-23 amendment's "effort stays orthogonal to
tier→model" for the execute phase; that orthogonality still holds for the plan
phase and the init/triage one-shots.
