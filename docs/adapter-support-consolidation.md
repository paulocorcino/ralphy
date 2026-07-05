# Adapter-support consolidation (design, HITL — issue #106)

Design of what should be promoted from the three agent adapters
(`ralphy-agent-{claude,codex,opencode}`) into the shared, vendor-neutral
`ralphy-adapter-support` crate **before** each adapter is split
([ADR-0022](./adr/0022-file-split-conventions.md)), so the three splits consume
the same helpers and do not diverge. Reuse is the goal; speculative abstraction
over legitimate per-vendor difference is explicitly out of scope.

Boundary this must respect: [ADR-0002](./adr/0002-core-agnostic-adapter-boundary.md)
and the `adapter-support` charter (crate-level doc in
[`lib.rs`](../crates/ralphy-adapter-support/src/lib.rs)). `adapter-support` owns
**mechanical plumbing only**, speaks a **`std`-only public surface** (`Command`,
`Duration`, `Instant`, `String`, `PathBuf` — no `portable-pty`, no vendor names),
and produces **no `Outcome`**. Every completion-protocol / classification decision
stays in the adapter. This design does not reopen that seam.

## What the size asymmetry actually is (answering the premise)

The prod-line gap — Claude `lib.rs` (~2840) vs OpenCode (~1597) vs Codex (~1432) —
is **mostly legitimate, not duplication.** Claude's excess is:

- the **interactive PTY execute path** (`execute_outcome`, `drive_session`,
  `LoginTuiWatch`, first-run gate priming) — Claude's primary billing path per
  ADR-0002; Codex/OpenCode run headless and have no equivalent;
- **Claude Code settings/hooks JSON** generation (Stop / PreToolUse / PostToolUse);
- the **multi-call `claude -p` loop** (`execute_headless` up to `max_exec_calls`) —
  Codex/OpenCode are single-shot `exec`;
- **richer transcript token accounting** (camelCase/snake_case `usage` split,
  `message.id` dedup).

None of that generalizes. So consolidation is **targeted and small**, not a
rebalancing of Claude into the shared crate. The three adapters are already
well-refactored against `adapter-support` (`run_headless`, `run_json_session`,
`materialize_assets`, `auth_error`/`detect_limit`/`scan_json_lines`,
`session_files_appeared`/`list_session_files`, `resolve_program`, `home_dir`,
`DONE_SENTINEL`/`PLAN_CHARTER`, `done_sentinel`/`blocked_reason` are all shared).

## Duplication map (verified, with evidence)

| # | Duplicated logic | Claude | Codex | OpenCode | Verdict |
|---|---|---|---|---|---|
| D1 | Issue budget/deadline: `with_max_minutes_per_issue` + `with_run_deadline` + `issue_deadline` clamp to `UNBOUNDED_ISSUE_HORIZON` | `lib.rs:240–260` | `lib.rs:227–256` | `lib.rs:324–353` | **Promote** — identical clock arithmetic, zero vendor content |
| D2 | Headless run wrapper: `run_headless` → combine stdout+stderr → write `<name>.log` → `exited_cleanly = exit.map(success).unwrap_or(false)` | `run_headless_call` `lib.rs:345–397` | `run_codex` `lib.rs:664–682` | `run_opencode` `lib.rs:610–630` | **Promote** — same post-`run_headless` shell |
| D3 | Non-JSON one-shot session (headless + write log + auth/timeout bail, no artifact) | `consolidate_knowledge` `lib.rs:483–536` | — | — | **Promote** — a `run_json_session` sibling; folds the one entrypoint still hand-rolling argv |
| D4 | `PROMPT_EXECUTE = include_str!(".../prompt.execute.md")` (byte-identical asset, 3 copies) | `lib.rs:43` | `lib.rs:203` | `lib.rs:100` | **Promote** — one shared const beside `PLAN_CHARTER` |
| D5 | Home-scoped store/config locator: `$XXX_HOME` override else `~/.dir/...` | `transcript_dir`/`dirs_home` | `codex_config_path` `:275`, `codex_sessions_dir` `:748` | `opencode_db_path` `:687` | **Promote (thin)** — shared "override-else-home-join" core |
| D6 | Snapshot-diff usage **fold**: snapshot dir → run → `session_files_appeared` → parse each → sum `Usage` | `fold_exec_usage` `:1687` | `fold_rollout_usage` `:760` | — (uses SQLite) | **Keep** — only 2 consumers, and the fold sums into core's `Usage`; the file-diff half is already shared |
| D7 | Outcome classifier + headless loop state machine | `classify_exec_call`/`headless_step`/`HeadlessReason` `:1323–1378` | `classify_codex_outcome` `:626` | `classify_opencode_outcome` `:579` | **Keep** — policy genuinely differs (flag-file+transcript vs exit-code vs JSON `saw_error`); shared predicates already extracted |
| D8 | Token/usage parsing bodies | `parse_transcript_usage` etc. | `parse_codex_rollout_usage` | `read_opencode_session_usage` (rusqlite) | **Keep** — different stores/schemas (flat jsonl / nested rollout / SQLite) |
| D9 | Command construction (argv/flags/env-scrub) | `run_headless_call` argv | `build_codex_command` | `build_opencode_command` | **Keep** — different CLIs; the reusable atoms (`resolve_program`, stdio-pipe, `run_headless`) are already shared |
| D10 | Auth/limit predicates + messages | `is_claude_auth_error`, `is_limit_text` | `is_codex_auth_error`, `is_codex_limit_text` | `is_opencode_auth_error`, `parse_opencode_limit` | **Keep** — phrases/JSON shapes are vendor-specific; scaffolds (`auth_error`, `detect_limit`, `scan_json_lines`) already shared |
| D11 | PTY byte mechanics: `strip_pty_escapes` `:1156`, `find_subslice` `:1810`, `scan_dsr_request` `:1820` | Claude only | — | — | **Not adapter-support** — single-consumer today; if ever promoted, home is `ralphy-pty` (std-only surface forbids it here). Defer |
| D12 | FS symlink/copy set: `link_or_copy_dir`, `symlink_dir`, `remove_path`, `copy_dir_all`, `ensure_gitignore_entries` | — | `lib.rs:94–184` | — | **Keep (for now)** — Codex-only (`.agents/skills` linking); promote only when a 2nd consumer appears |

## Shared helper surface to add to `adapter-support`

These are the helpers the three split issues will consume. All `std`-typed, no
`Outcome`, no vendor vocabulary.

```rust
// budget.rs — D1
/// Per-issue deadline: `now + max_minutes`, clamped to `run_deadline`. A
/// `max_minutes` of 0 disables the per-issue cap (falls back to run_deadline).
/// `unbounded` is the far-future horizon the caller sources from core
/// (`ralphy_core::UNBOUNDED_ISSUE_HORIZON`) so this crate stays core-free.
pub fn issue_deadline(
    now: Instant,
    max_minutes_per_issue: u64,
    run_deadline: Option<Instant>,
    unbounded: Instant,
) -> Instant;

// headless (extend lib.rs) — D2
pub struct HeadlessRun {
    pub stdout: String,       // stdout alone (OpenCode parses the JSON stream from it)
    pub log: String,          // stdout + stderr, as persisted
    pub exited_cleanly: bool, // exit.map(|s| s.success()).unwrap_or(false)
    pub timed_out: bool,
}
/// `run_headless` + combine stdout/stderr into `log`, persist it at `log_path`,
/// and recover `exited_cleanly`. The adapter keeps its own `classify_*`.
pub fn run_headless_logged(
    cmd: Command, prompt: &str, timeout: Duration, log_path: &Path,
) -> Result<HeadlessRun>;

// json_session (extend) — D3
/// Sibling of `run_json_session` for one-shots that produce **no JSON artifact**
/// (Claude's `consolidate_knowledge`): spawn, persist log, bail on auth/timeout,
/// return the combined log. Same `JsonSession` inputs minus `out_path`.
pub fn run_text_session(
    session: TextSession<'_>, auth_error: impl Fn(&str) -> bool,
) -> Result<String>;

// paths (extend) — D5
/// `override_env.map(PathBuf::from)` else `home_dir().map(|h| h.join(default_rel))`.
pub fn home_scoped_path(override_env: Option<OsString>, default_rel: &Path) -> Option<PathBuf>;

// lib.rs — D4
/// The shared execution charter asset, embedded once (like `PLAN_CHARTER`).
pub const PROMPT_EXECUTE: &str = include_str!(".../prompt.execute.md");
```

**Not promoted, on purpose** (anti-over-abstraction — the issue's fourth
acceptance criterion): D6–D12. D7/D8/D10 are the load-bearing case: the *pattern*
(classify → detect limit → detect done/blocked; correlate session → sum usage)
looks shared, but the *policy and data shapes* are legitimately per-vendor, and
the mechanical scaffolds underneath them (`detect_limit`, `blocked_reason`,
`done_sentinel`, `scan_json_lines`, `session_files_appeared`) are **already**
extracted. Unifying the bodies would force three different vendors to bend to one
shape — exactly the ADR-0002 failure mode. D11 is the boundary trap: PTY helpers
are generic but must never land in `adapter-support` (std-only, no PTY); their
home is `ralphy-pty`, and they have one consumer today, so defer.

## Sequencing for the split issues

1. **Land D1–D5 in `adapter-support` first** (this pre-split step): add the
   helpers with tests migrated from the adapters (ADR-0022 — tests move with the
   code), each adapter rewired to call them. Public crate API of the adapters is
   unchanged (internal-only consolidation), so no downstream churn.
2. **Then** the three `refactor(adapters): split …` issues (claude/codex/opencode)
   run against a shared floor: each split is now moving genuinely
   vendor-specific code into `foo.rs` + `foo/` modules, not re-copying plumbing.

Rough payoff: D1 removes ~3×20 lines, D2 ~3×15, D4 ~3 consts→1, D3 folds one
hand-rolled scaffold, D5 ~4 locators→1. Small but it prevents the three splits
from crystallizing three copies of the same clock/log/charter code.

## Follow-up (not done here)

The three split issues (claude/codex/opencode) plus a "land D1–D5" issue are the
natural next tickets. Not created here — creating GitHub issues is outward-facing
and out of scope for this HITL design (per repo policy: no push/PR/issue without
an explicit ask).
