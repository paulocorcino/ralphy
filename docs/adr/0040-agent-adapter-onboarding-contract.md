# The adapter onboarding contract: what Ralphy asks of a new agent CLI vendor

Ralphy has onboarded four vendors (Claude, Codex, OpenCode, Kimi) and each time
the same knowledge was rediscovered by reading the previous adapter. The
per-vendor ADRs (0004, 0005, 0028) record *decisions*; none of them records the
**questions**. This ADR does: it is the vendor-neutral checklist a fifth,
sixth, or seventh vendor is probed against, and the wiring inventory an adapter
must satisfy before it can be called complete.

It is deliberately a **contract, not a framework**. ADR-0002 already settled
that there is no shared "headless runner" the vendors bend to fit — the only
thing that must match is the `Outcome` the core receives, not how it was
produced. This ADR does not walk that back. It standardizes the *interrogation*
and the *inventory*, and leaves every answer free.

Status: **accepted**. Amends nothing. First application:
[docs/research/copilot-cli-adapter-spike.md](../research/copilot-cli-adapter-spike.md).
**Amendment 1** (2026-07-20, from the Cursor spike) adds §C11 and four
sharpenings — see the end of this file.

## How to use this ADR

Three phases, in order. **Phase 2 may not begin until Phase 1's capability
matrix is filled in with observed evidence** — not documentation, not
inference. Every row cites a command that was actually run and its output.

1. **Probe** — answer the C-questions (§Capability matrix) against the real,
   installed CLI. Record raw evidence in `docs/research/<vendor>-cli-adapter-spike.md`.
2. **Decide** — write `docs/adr/00NN-<vendor>-adapter.md` as D1..Dn, one
   decision per unresolved axis, each citing the spike.
3. **Wire** — work the §Wiring inventory. It is exhaustive as of this ADR;
   an item that no longer exists is an amendment to *this* file, not a silent skip.

## Capability matrix — the questions every vendor must answer

### C1 — Invocation and the headless contract
- What is the **headless one-shot** invocation (the `-p` analog)? Is there one?
- Does the prompt go on **argv or stdin**? *(Argv has a hard OS ceiling —
  ~32 KB on Windows. Ralphy's `prompt.execute.md` is ~24 KB **before** the issue
  body is appended, so argv is a latent truncation bug. Prefer stdin whenever
  the vendor accepts it; if argv is the only channel, this is a blocking finding.)*
- Which flag grants **full autonomy** (auto-approve every tool)? Non-interactive
  mode usually *requires* it.
- Is a **PTY** required for subscription billing (the Claude particularity,
  ADR-0002), or is headless billed the same?
- What is the **working-directory** flag, and does the CLI honour the spawned
  process's cwd?

### C2 — The output stream
- What **structured output** modes exist, and is one mandatory for correctness?
  *(Kimi's rich TUI renderer crashes on a cp1252-redirected stdout — ADR-0028 D5.
  Assume the default renderer is hostile to capture until proven otherwise.)*
- Is the stream **line-delimited JSON**? Enumerate the `type` discriminators.
- Is there an explicit **terminal envelope** (a final "the run is over" record),
  or does the stream just stop? An envelope is worth a lot: it distinguishes a
  clean finish from a truncated capture.
- How is the **final assistant message** identified? *(The recurring shape:
  the last assistant record with text and **no** tool requests.)*
- Does any stream field claim to report **progress** (files changed, lines
  added)? **Verify it against a HEAD diff before trusting it** — a vendor that
  reports only its own write-tool activity will report zero for work done
  through a shell tool.

### C3 — Completion and the sentinel
- Does the vendor reliably emit an operator-chosen token as the **last line** of
  its final message? Test it explicitly — this is `DONE_SENTINEL`'s whole basis.
- Are there **semantic exit codes** beyond 0/1? *(Kimi's `75 = RETRYABLE` is the
  cleanest limit signal any vendor has offered — ADR-0028 D9.)*
- Is there a **hook mechanism** (a Claude-style Stop hook) that would give
  deterministic completion instead of text scraping?
- Which signals fill `CompletionSignals` (ADR-0023 Camada 1)? Ordering is **not**
  a vendor decision — it always delegates to `classify`.

### C4 — Models
- Is the vendor **single-model** or **multi-model**?
- Is there a **free, deterministic** way to enumerate available models?
  *(Beware: asking the agent in-band — `-p "/model --list"` — is a paid model
  round-trip whose output is generated prose, not a listing. It is
  non-deterministic across runs. Never build on it.)*
- Does the **documented** model list match the **entitled** list for the
  operator's plan? These diverge; the documented list is not a contract.
- Does the vendor **auto-route** when no model is pinned, and does the stream
  disclose the chosen model? If routing is per-turn, a single run can span
  several models — `Usage::fold_usage` (heaviest-model attribution) is then
  load-bearing, not cosmetic.
- Is there a **reasoning-effort** knob, and is it orthogonal to model choice?
- Does the vendor **reject an unknown model** deterministically, before any paid
  call? Quote the exact error — it becomes an actionable stop.
- Is model availability scoped to the operator's **plan or subscription tier**?
  If so, *the same adapter code must work at every tier* — which means model
  resolution is `Option<String>`, omitted from argv when `None`, never a
  hardcoded default. **Probe at more than one tier before believing a model
  list**, and never let a vendor's own documentation stand in for entitlement.
- **Every reachable model id must land in `PriceTable::default`** (ADR-0034) or
  every run logs "unknown model".

> **Probe technique — the deliberate-failure debug log.** A rejection is free; an
> acceptance costs a real call, so a probe loop over candidate ids is not
> viable. Instead, force a failure *late*: invoke with a deliberately invalid
> model and full debug logging to a scratch directory. Vendors fetch their model
> catalog before validating the flag, so the catalog — often with entitlement
> tiers and the rate card — lands in the log at zero cost. The same run's
> "falling back to default model X" line reveals the operator's tier. This
> generalizes to any vendor whose CLI logs its own control-plane responses, and
> it is the cheapest enumeration Ralphy has found.

### C5 — Authentication
- Which command does the operator run to log in? It must appear verbatim in the
  adapter's `<VENDOR>_AUTH_ERROR_MSG`.
- **Probe the logged-out state deliberately** and record the exact exit code and
  stderr marker. This is the only cheap way to get the ADR-0013 detector right,
  and it is unrecoverable once you log in — do it first.
- Where does the credential live (OS credential store, plaintext file, env)?
- Which **environment variables** does the vendor read for auth? Any that Ralphy
  or its sibling tooling also sets is a **cross-contamination hazard** and must
  be scrubbed or deliberately allowed (the `ANTHROPIC_API_KEY` precedent).
- Detection stays **behavioral** (exit code + stderr marker), never
  credential-file inspection — that is the settled house style.

### C6 — Usage and the session store
- Where is the session store, and what is its **topology**: flat JSONL, nested
  JSONL, or a database?
- Can Ralphy **mint the session id** before spawning (a `--session-id` analog)?
  If yes, prefer it: lookup becomes a direct key and the ADR-0008 D10
  snapshot-diff ("appeared-over-grew") is unnecessary. If no, snapshot-diff.
- Are per-call token records **cumulative** (keep-last, Codex) or **incremental**
  (sum, Kimi)? Getting this backwards silently multiplies or divides the bill —
  write the test so the wrong choice fails.
- Do records carry **model attribution** per call?
- Are cache-read and cache-creation tokens **separable**? They must never be
  folded into `input` (ADR-0008 D2).
- Is billing denominated in **tokens** or in a vendor credit unit? A credit unit
  is not a token count; if both are available, record both and be explicit about
  which one drives `Usage`.
- What does `ralphy-usage-scan` need (ADR-0033)? A pure, read-only,
  never-erroring `scan_<vendor>` that tolerates a missing store.

### C7 — Limits
- How does a quota exhaustion **surface**: exit code, structured stream record,
  or prose?
- Is there a **schedulable reset hint**? If the hint is unreliable, discard it
  and emit `Limit(None)` — ADR-0030's synthetic ~30-minute cadence then applies
  automatically. **Do not force `--stop-on-limit`**; that mechanism was removed.
- For a multi-provider vendor, match on a **limit class** (a regex over
  "rate limit | quota exceeded | too many requests | …"), never one provider's
  phrasing — OpenCode's `usage_limit_regex` is the reference.
- Is there an operator-facing **spend cap** flag, and is it a hard or soft cap?

### C8 — Skills and prompts
- How does the vendor discover **skills**: an explicit flag, a config
  environment variable, or a conventional directory? *(If it already reads a
  directory Ralphy's ecosystem populates, materialization may be free — but
  verify from the stream that the skill actually loaded.)*
- Does the vendor have a **native plan mode**, and does it fit Ralphy's
  "write `.ralphy/plan.md` yourself" contract? *(It usually does not: native
  plan modes persist to vendor-private stores and signal completion out-of-band.
  Rejecting it is the norm — say so explicitly rather than silently.)*
- Which of the **8 overlay slots** does this vendor fill (`execution-model`,
  `self-review-step`, `self-review-guidance`, `ledger-example`,
  `planning-mode-intro`, `skill-invocation`, `stages-section`, `mode-rules`)?
  An empty slot is a valid deliberate absence.
- **Every vendor gets its own `overlay.<vendor>.md`**, even if all 8 slots are
  empty — the assembly test is the anti-drift gate.

### C9 — Blast radius and the product ethos
Ralphy **never pushes and never opens PRs**. A vendor that ships capabilities
which would do so on the agent's behalf is a direct conflict, not a preference.

- Does the vendor bundle **MCP servers by default**? Which, with what scope, and
  under whose credential?
- Can the session be **exported or remotely controlled** by a third party? What
  is the default, and which flag disables it?
- Does the vendor offer **delegate-to-cloud / open-a-PR** verbs the agent could
  reach unprompted?
- Does the vendor spawn **background tasks** that can outlive the run?
- Does it read **repo-local instruction files** that would compete with Ralphy's
  charter, and can that be disabled?
- Does it **auto-update** itself mid-run?

Every "yes" needs an explicit stance in the vendor ADR: forced off, forced on,
or deliberately left to the operator.

### C10 — Cross-platform and I/O hygiene
- Console-encoding traps on Windows (cp1252 crashes; UTF-8 env vars that flip
  the CLI into TUI detection — ADR-0028 D5).
- Which env vars must be **removed** vs **set** on the child.
- Binary resolution: always `resolve_program`, never `Command::new("vendor")`.
  Probe non-PATH install locations (`~/.local/bin`, winget shims).
- `ACCEPTS_IMAGES` (ADR-0025): does the **headless** path expose an attachment
  channel? A model that advertises vision but has no headless delivery path is
  `false`.

## Wiring inventory

An adapter is not done when its crate compiles. These are the edit sites,
verified at the time of writing. Ordered by how easy they are to forget.

**Tier 1 — the crate** (`crates/ralphy-agent-<vendor>`, ~1 300 LOC, deps:
`anyhow, tracing, serde_json, include_dir, ralphy-core, ralphy-adapter-support`):

| File | Owns |
|---|---|
| `lib.rs` | `impl Agent`, `ACCEPTS_IMAGES`, the plan-prompt `include_str!`, the three budget builders (`with_max_minutes_per_issue`, `with_idle_minutes`, `with_run_deadline` — `build_agent` calls all three) |
| `command.rs` | Two command builders (run + one-shot); argv, stdio piping, env hygiene |
| `auth.rs` | `<VENDOR>_AUTH_ERROR_MSG` + `is_<vendor>_auth_error` (+ limit predicate if text-based) |
| `outcome.rs` | Stream→text parse, `CompletionSignals` fill, the degraded predicate, the single `HeadlessCall` site |
| `usage.rs` | Store locator (`home_scoped_path`), record parser, fold via `Usage::fold_usage`, `session_id` extractor |
| `tasks.rs` | The four one-shots: `diagnose_repo`, `draft_issues`, `triage_issues`, `consolidate_knowledge` |
| `skills.rs` | `include_dir!` + `materialize_assets`, if the vendor supports skills |

**Tier 2 — the prompt**: `assets/prompts/plan/overlay.<vendor>.md`, regenerate
(`RALPHY_REGEN_PROMPTS=1 cargo test -p ralphy-core --test prompt_assembly`),
register in that test's `VARIANTS`.

**Tier 3 — the registry** (hand-maintained; **three separate agent enums** exist
and they do not share a definition):

`cli.rs` `CliAgent` + `cli_name` · `init/gate.rs` `Agent` + `ALL` (**the array
length is hardcoded — bump it**) + `cli_name` + `accepts_images` +
`agent_logged_in`'s argv arm · `run/wiring.rs` `build_agent` · four one-shot
dispatch matches (`init/run.rs`, `init/issues.rs`, `triage.rs`, and
`main.rs::consolidate_with_agent`) · `main.rs::consolidate_defaults` ·
`models.rs` `agent_slug` (+ `plan_action` only if the vendor can list models) ·
`pricing.rs` `PriceTable::default` · `runstate/capture.rs` `EMIT_CALL_SHAPES`
and `MIGRATED_EMITTERS` (ADR-0039) · workspace + CLI `Cargo.toml`.

**Tier 4 — usage scan and daemon**: `usage-scan/src/<vendor>.rs` +
`<Vendor>Scan` + the `pub mod`/`pub use` · `daemon/src/usage.rs` path resolver
and `interactive_records` · the four `daemon/src/lib.rs` state-plumbing sites ·
**`daemon/src/session.rs::Agent`** — the third agent enum, plus its two matches
(`from_query`, `program_name`), `daemon/src/dispatch.rs::agent_flag`, and the
`agents` / `consoleItems` / accelerator-map trio in `daemon/assets/ui/app.js`.

Treat `session::Agent` as the canary, not `agent_flag`. This tier has *already
been missed once*: Kimi shipped a full adapter and a `daemon/src/usage.rs` path
resolver while remaining absent from the daemon enum, so the daemon could
account for its tokens but not launch it (issue #228, fixed). Nothing complained,
because `agent_flag` is exhaustive over the daemon's own enum — a missing variant
there compiles cleanly and always will. The failure surfaces at runtime, three
layers deep: `Agent::from_query` returns `None` → `ArgvError::BadParam("agent")`
→ the daemon spawns nothing. Add the variant first and let the compiler walk you
through the rest.

**Tier 5 — the tests that will trip you**: `prompt_assembly` ·
`capture.rs::no_vocabulary_literal_outside_emit` · the `pricing.rs` model-id
assertion · `gate.rs`'s `accepts_images` assertion · `cli.rs`'s `--agent` round
trip · `daemon/src/lib.rs`'s `/api/usage` coverage. Adapter tests are inline
`#[cfg(test)] mod tests`, not a `tests/` directory. A subprocess helper bin is
normally **not** needed — `adapter-support`'s `headless_test_child` already
covers the process plumbing.

## Consequences

- The **capability matrix is the deliverable of a spike**, and a spike with an
  unanswered C-question is not finished. "The docs say X" is not an answer to a
  C-question; a command and its output is.
- **The logged-out probe is destructive of its own evidence.** C5 must be run
  before the operator authenticates, or that signature costs a logout to recover.
- **Three agent enums and five tiers** is the real cost of a vendor, and most of
  it is outside the adapter crate. Anyone estimating "just write the adapter" is
  estimating Tier 1 only — roughly half the work.
- This ADR is expected to **drift**, because the wiring inventory tracks live
  code. Drift is repaired by amending this file, which is cheaper than the
  current alternative of re-reading four adapters.
- Nothing here constrains an adapter's *answers*. A vendor free to be nothing
  like the other four remains a first-class citizen (ADR-0002); this contract
  only insists the differences were **found on purpose** rather than discovered
  in production.

---

## Amendment 1 — 2026-07-20, from the Cursor spike

Source: [docs/research/cursor-cli-adapter-spike.md](../research/cursor-cli-adapter-spike.md).
Cursor answered every C-question, and in doing so exposed one axis this contract
did not ask about at all, plus four places where an existing question let a
wrong answer through. Nothing below invalidates a prior spike; Copilot and Kimi
simply happen to answer C11 with "no".

### C11 — Persistent state the vendor owns and Ralphy can corrupt

The contract assumed a run is a pure function of argv, env and cwd. It is not.
Cursor keeps an operator config (`~/.cursor/cli-config.json`) that is **both an
input and an output** of a run: `--model` writes four keys into it, and a run
that *failed* keeps the write. Every subsequent invocation that passed no
`--model` then inherited a model the account could not use and failed too —
including invocations from a different tool sharing the same config.

- Does the vendor keep a **config file the CLI itself writes**? Where, and which
  flags mutate it?
- **Does a failed run roll the mutation back?** Probe this deliberately: run a
  flag that is rejected, then run again without it.
- Which settings in that file **override or veto argv**? (Cursor's `--force` is
  *"unless explicitly denied"* by a `permissions.deny` list the operator owns.)
- Is there a **config-dir env var** (`CURSOR_CONFIG_DIR`, `XDG_CONFIG_HOME`)
  that would let Ralphy run against isolated state instead of the operator's?
- **Does the vendor push content onto the operator's disk mid-run?** Cursor
  downloaded 17 vendor-authored skills — including PR-opening guidance and two
  that mutate the operator's own configuration — on first authenticated run.

The rule that follows: **an adapter must state every argv flag explicitly,
including the ones whose value is "the default"**, because on a vendor with
write-back, omitting a flag does not mean "default" — it means "whatever the
last invocation left behind".

### Sharpenings to existing questions

**C9 — add: does a run transmit the repository off the machine?**
Not "does the vendor have a cloud feature", but: does an ordinary headless run
send source code to the vendor's servers, and can that be turned off? Cursor's
first run uploaded a merkle tree of the **parent** repository — 476 sync lines —
from a task forbidden to read files, with the vendor's own privacy flags on.
Scoping cwd does not scope the upload. **Verify the opt-out by controlled A/B on
two fresh repositories**, because a second run against an already-known repo is
silent whether or not the opt-out works. And check the opt-out's collateral: one
of Cursor's two ignore files also disables the agent's edit tool, which the
agent then routed around via its shell.

**C8 — add: whose skills does it read?**
Ask which directories the vendor scans, then **plant a marker `SKILL.md` in each
candidate root and have the agent list what it sees** — documentation is not
evidence here. Cursor deliberately reads `~/.claude/skills`, `.claude/skills`,
`~/.codex/skills` and `.codex/skills` for "backward compatibility", and injected
78 skills — the operator's entire unrelated library — into a single request.
This cuts both ways and both are findings: free materialization for Ralphy, and
cross-vendor leakage with no CLI-side allowlist.

**C3 — add: a documented hook is not a working hook.**
Cursor documents a `stop` event ("called when the agent loop ends") that would
have given deterministic completion. Registered alongside two other events in a
project `hooks.json` and exercised by a real run, **only
`beforeShellExecution` fired**. If a hook mechanism is the reason an adapter
plans to skip `DONE_SENTINEL`, that hook must be observed firing **in the
headless path**, not read about.

**C10 — add: one vendor, several binary names, none on `PATH`.**
Cursor installs as `agent` *and* `cursor-agent` (`.cmd` + `.ps1` on Windows,
plus a `versions/` tree), off `PATH` on both platforms, and its own CI docs
name a third install location. `resolve_program` resolves through `PATH`; a
vendor that never lands there needs an explicit probe list, and the adapter must
try **every** name the vendor ships.

### One consequence for the wiring inventory

C6 may legitimately answer **"there is no local usage store"**. Cursor reports
tokens only in the live stream envelope and persists none of it, so
`ralphy-usage-scan` (ADR-0033) cannot see interactive sessions for such a
vendor. Tier 4's `usage-scan/src/<vendor>.rs` is still written — it enumerates
sessions and reports tokens as unavailable. **Stating the gap is the deliverable;
inventing a number is the failure.**
