# On a usage limit the adapter rotates to the next configured account before it sleeps; the core still sees one `Limit`

Status: **proposed** (2026-09-15) — decided, not yet implemented. Third in
the track opened by [ADR-0058](./0058-checkout-per-run.md); independent of
it and of [ADR-0059](./0059-agent-state-by-hooks.md) in code, related by
convention (one new `emit::` event).

_Extends [ADR-0003](./0003-usage-limit-handling.md) (wait for the reset,
resume the same issue) and [ADR-0030](./0030-synthetic-reset-for-unschedulable-limits.md)
(synthetic reset) without changing either: rotation happens **before** the
wait those ADRs describe. Amends [ADR-0002](./0002-core-agnostic-adapter-boundary.md)
§adapter settings (one more adapter-owned key), [ADR-0008](./0008-token-usage-tracking.md)
D7 (the actor is still the git identity; the account is a new label),
[ADR-0033](./0033-interactive-usage-stateless-scan.md) §5/§6 (an additive
ledger field and extra scan roots), [ADR-0039](./0039-event-vocabulary-owned-by-core-emit.md)
(one event), and the [ADR-0040](./0040-agent-adapter-onboarding-contract.md)
C5/C11 checklist rows._

## The time a limit costs today

On `Outcome::Limit` the runner does what ADR-0003 D1 says: it waits for the
vendor's reset, plus five minutes, and retries the same issue from
`plan.md`. When the vendor reports no reset time — and only Claude and Codex
ever do; OpenCode parses one and drops it, Copilot, Cursor, Gemini and Kimi
never see one — ADR-0030 substitutes a synthetic 25 + 5 minutes. A run that
hits a five-hour window at 14:00 sleeps until the window turns; a run on a
vendor with no hint sleeps thirty minutes at a time until it does.

That is the right behaviour for one account. Operators who run Ralphy
unattended increasingly hold more than one — a second subscription, a
team seat next to a personal one — precisely so that a limit on one does not
idle the machine. Nothing in Ralphy can use the second account: every
adapter reads the vendor's default home (`~/.claude`, `$CODEX_HOME`,
`$KIMI_CODE_HOME`) and never points anywhere else. The Claude adapter goes
further and hardcodes `~/.claude` in three places regardless of the
operator's own `CLAUDE_CONFIG_DIR` (`crates/ralphy-agent-claude/src/usage.rs:22,274`,
`interactive.rs:495`).

Two adapters already own a per-run home: Cursor seeds a scratch
`CURSOR_CONFIG_DIR` from the operator's (`crates/ralphy-agent-cursor/src/command.rs:53-70`),
Gemini sets `GEMINI_CLI_HOME` to `.ralphy/gemini-home` unconditionally
(`crates/ralphy-agent-gemini/src/command.rs:179-196`). The mechanism to
point a vendor at a directory Ralphy chose is proven; what is missing is the
list of directories and the rule for walking it.

## Decision

### 1. An account is a directory the operator prepared; Ralphy never writes credentials

An **account** is a vendor home directory — the thing the vendor's own env
var names — that the operator created by running the vendor's login under
that variable (`CLAUDE_CONFIG_DIR=~/.claude-b claude login`, and the
equivalent for each vendor). Ralphy reads it, points the child at it, and
scans it for usage. It **never** creates one, copies credentials between
two, refreshes a token, or writes into the operator's default home to make
an account "active". Rotation is `env(<VENDOR_HOME_VAR>, <dir>)` at spawn and
nothing else.

This is ADR-0040 C11 applied: the vendor's persistent state is the vendor's;
an unattended process that rewrites `~/.claude/.credentials.json` is one bug
away from locking the operator out of their own CLI. Isolation costs the
operator one login per account and costs Ralphy no trust.

### 2. The list is an adapter-owned setting; the core never sees it

Each adapter that has a home variable gains a settings key in its own
section of `settings.json`:

```json
{ "claude": { "accounts": ["~/.claude", "~/.claude-b"] },
  "codex":  { "accounts": ["~/.codex", "~/.codex-work"] } }
```

`ClaudeSettings`, `CodexSettings`, `KimiSettings` carry it (adapter crates,
per the ADR-0002 amendment: "`Settings` keeps only agent-agnostic keys plus
a generic per-agent section blob"); `run/wiring.rs` reads it and passes it
to the adapter builder as it does every other vendor value. `ralphy config
set claude.accounts` accepts a comma-separated list. Order is rotation
order. An absent or one-element list is today's behaviour exactly; the
first element is what the adapter uses when nothing has been limited.

Paths may be `~`-relative and are resolved by the adapter. A path that does
not exist is a preflight error (§6), not a runtime surprise.

### 3. Rotation happens inside `plan()` / `execute()`; the core still sees one `Limit`

When the adapter's own limit detection fires — the same detection that
today yields `Outcome::Limit` or `PlanLimit` (ADR-0023 ladder, per-vendor
predicates) — the adapter, before returning:

1. records the limit against the current account (in memory, for this run);
2. advances to the next account in the list that is not recorded as limited;
3. if one exists, re-spawns the phase under it — resuming exactly as
   ADR-0003 D2 says, "git + `plan.md`, never `claude --resume`": a plan
   phase re-plans, an execute phase re-executes the same `plan.md`;
4. if none exists, returns `Outcome::Limit` / `PlanLimit` with the
   **earliest** reset hint across the limited accounts, and clears its
   record so the next attempt after the wait starts from the first account.

The core, `RunClock`, the two-consecutive-no-commit cap
(`phases.rs:549`), `MAX_PLAN_LIMIT_RESUMES`, `--stop-on-limit` — none of
them change. `Outcome::Limit` acquires a slightly stronger meaning, "every
configured account is limited", which is what "limited" already meant with
one account.

The rotation is bounded: each account is tried at most once per phase
attempt, so a list of N accounts costs at most N spawns before the ADR-0003
wait. A limit on account 2 while account 1 is already limited does not go
back to 1 within the same attempt.

### 4. One event: `emit::account_rotated`

Rotation inside the adapter is invisible to the run snapshot and the
Telegram card unless it says so. Per ADR-0039 §1:

```rust
pub fn account_rotated(from: &str, to: &str, reason: &str)
```

decoded into `RunEvent::AccountRotated`, CloudEvent
`dev.ralphy.run.account_rotated` (data `{ from, to, reason }`), a line in the
Telegram card, and an additive `account: Option<String>` on the snapshot's
`PhaseBlock` naming the account in use. `from`/`to` are **labels** — the
directory's basename by default, or the operator's own label if the entry
is written as `{ "path": "...", "label": "work" }` — never full paths, since
the event leaves the machine (ADR-0019).

### 5. The ledger records the account as a label; the actor stays the git identity

`LedgerRecord`, `Plan` and `Execution` gain `account: Option<String>` with
`skip_serializing_if`, the same additive shape ADR-0033 §5 used for
`session_id`. ADR-0008 D7's actor is unchanged — the human is still
`git config user.email`; the account is *which subscription paid*, a
different axis. `ralphy usage --by account` groups on it. The label is the
same one §4 emits.

Usage-scan (ADR-0033) gains the configured account roots as **additional
scan bases** for that vendor: a phase served by `~/.claude-b` writes its
transcript under `~/.claude-b/projects/…`, and the Spend view must find it.
The scan stays stateless (§2); it just reads more directories.

### 6. Preflight probes every configured account once per run

`preflight_agents` (`crates/ralphy-cli/src/run/wiring.rs:393-399`) checks
only that the binary exists. With a list, it additionally checks that each
account directory exists and — for vendors whose adapter has a cheap,
non-destructive auth probe — that it is logged in, so a second account that
was never logged in fails the run at start with a named path, not
mid-issue after a rotation. ADR-0040 C5's warning stands: a probe that
triggers a login flow is not a probe; where the vendor has no safe probe,
existence is the check.

### 7. Vendors in scope, and why the others are not

| Vendor | Home variable | In this ADR | Reason |
|---|---|---|---|
| Claude | `CLAUDE_CONFIG_DIR` | **yes** | the vendor honours it for credentials, settings and `projects/`; the three hardcoded `~/.claude` paths are fixed as a prerequisite (§8) |
| Codex | `CODEX_HOME` | **yes** | already read by the adapter; `auth.json` and `sessions/` live under it |
| Kimi | `KIMI_CODE_HOME` | **yes** | already read for usage; login state lives under it |
| Cursor | `CURSOR_CONFIG_DIR` | no | the adapter already seeds a per-run scratch dir from the operator's; an account list would mean seeding from a different source — a later extension of that mechanism, not this ADR |
| Gemini | `GEMINI_CLI_HOME` | no | the adapter owns the home unconditionally and scrubs auth env by `selectedType`; accounts would have to be expressed as auth material, which §1 forbids Ralphy from handling |
| Copilot | `COPILOT_HOME` | no | credentials live in the OS store, not the directory; the variable does not select an account |
| OpenCode | — | no | no home variable; auth is per provider inside one config |

### 8. Prerequisite: the Claude adapter resolves its home through one function

Independently of accounts, `crates/ralphy-agent-claude` gains
`config_dir() -> PathBuf` (`$CLAUDE_CONFIG_DIR` else `~/.claude`) and the
three hardcoded sites use it: the transcript directory for usage and for
`transcript_limit` (`usage.rs`, `auth.rs:81`), and the workspace-trust key
in `~/.claude.json` (`interactive.rs:491-506`). Today an operator who sets
`CLAUDE_CONFIG_DIR` themselves gets wrong usage and a possible missed limit;
with rotation the wrong store would be read on every swap. Usage-scan's
Claude root (`crates/ralphy-usage-scan/src/claude.rs`) takes the same
resolution. This lands first, as its own `fix`.

### 9. Opt-in, and the operator's responsibility

Rotation is off until an operator writes an `accounts` list. Ralphy's docs
say what the mechanism does — it points the vendor CLI at a directory the
operator logged into — and say nothing about whether a given operator may
hold several accounts of a vendor: that is between the operator and the
vendor's terms, as every other capability Ralphy exposes opt-in
(cf. the security posture: strongest available, opt-in, never withheld).

## Considered options — rejected

- **Manual switch only (a config key the operator flips between runs).**
  Solves nothing for the unattended case, which is the case.
- **Materialise the chosen account's credentials into the vendor's default
  home, snapshot and restore the original.** Works for an interactive tool
  whose user can watch the swap; from an unattended process it is a race
  with the vendor's own token refresh and a corruption path into the
  operator's primary login. ADR-0040 C11 exists to reject exactly this.
- **Rotate in the core: `Outcome::Limit { retry_now: bool }` or a
  vendor-aware `RunClock`.** Teaches the core that accounts exist and that
  a vendor has a home directory — an ADR-0002 leak for no gain over doing it
  where the spawn already is.
- **Poll the vendor's quota endpoint to rotate *before* a limit.** A second
  network dependency, a token handled by Ralphy, and an endpoint that
  rate-limits its own pollers. Ralphy reacts to the limit the vendor reports
  in-band; proactive quota awareness is a different feature.
- **One shared "accounts" key in the agent-agnostic `Settings`.** The list
  is vendor-shaped (which variable, which probe); ADR-0002 puts it in the
  adapter section.
- **Full paths in the event.** They leave the machine (ADR-0019 sink);
  labels suffice.

## Consequences

- A run with two Claude accounts that hits a five-hour window continues on
  the second within one respawn instead of sleeping until the window turns;
  the ADR-0003 wait becomes the *last* resort, not the first.
- `Outcome::Limit` and every match on it are untouched; adapters that never
  configure accounts behave byte-for-byte as before.
- Ledger lines and the Spend view can show which subscription paid for a
  phase; `ralphy usage --by account` exists.
- One `emit::` function, one `RunEvent`, one CloudEvent type, one snapshot
  field, three adapter settings keys, one preflight extension, one usage-scan
  root list.
- ADR-0002, ADR-0008 D7, ADR-0033 §5/§6, ADR-0039, ADR-0040 C5/C11 carry
  one-line amendments pointing here; `docs/usage-and-cost.md` and
  `docs/run-options.md` document `accounts`.
- Changelog: `feature` — "a run continues on your next configured account
  when one hits its usage limit, and the ledger says which account paid".

## Implementation notes (not decisions)

(1) §8 Claude `config_dir()` fix + usage-scan root — its own PR; (2)
`emit::account_rotated` + `RunEvent` + snapshot field + CloudEvent + ledger
`account`; (3) Claude adapter: `accounts` setting, rotation in plan/execute,
preflight; (4) Codex; (5) Kimi; (6) `ralphy usage --by account` and
usage-scan extra roots; (7) docs and amendments. Steps (3)–(5) are one issue
each and independent.
