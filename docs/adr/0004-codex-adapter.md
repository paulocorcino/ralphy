# The Codex adapter: a per-run peer of the Claude adapter, native to `codex exec`

Ralphy gains a second agent CLI vendor — OpenAI's `codex` — as a new isolated
crate `ralphy-agent-codex` that implements the same PTY-free `Agent` trait
(docs/adr/0002). The adapter is selected **per run** by a `--agent claude|codex`
flag; the core keeps taking a single `&dyn Agent` and never learns which vendor
it holds. The only surface shared between the two adapters is the core's `Agent`
trait and `Outcome` enum — there is deliberately **no** shared "headless runner"
that both bend to fit. Each adapter is built to its vendor's best-fit mechanism,
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
