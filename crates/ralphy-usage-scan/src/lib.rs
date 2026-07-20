//! Stateless usage scan (ADR-0033 §2/§6/§7): parse the on-disk vendor session
//! stores into normalized interactive token-usage records, aggregated per
//! session × model. Pure and sync — no tokio, no state files, no writes; the
//! daemon calls it on request and serializes the result.
//!
//! This slice ships the **Claude** ([`claude`]), **Codex** ([`codex`]),
//! **OpenCode** ([`opencode`]), **Kimi** ([`kimi`]), and **Copilot**
//! ([`copilot`]) modules. The
//! one-module-per-vendor shape (§7) leaves room for more to follow. The [`kimi`]
//! module carries a tokscale-derived (`junhoyeo/tokscale`, MIT) parser — that
//! attribution lives in `kimi.rs`, not here; this file owns only the shared
//! record contract.

use std::collections::HashSet;
use std::path::Path;

pub mod claude;
pub mod codex;
pub mod copilot;
pub mod kimi;
pub mod opencode;

pub use claude::scan_claude;
pub use codex::scan_codex;
pub use copilot::{scan_copilot, session_tokens};
pub use kimi::scan_kimi;
pub use opencode::scan_opencode;

/// The four Messages-API token counts an interactive record carries (ADR-0033 §3
/// record shape). Snake_case field names mirror the ledger's `tokens` block so a
/// UI can render run and interactive records the same way.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

/// One normalized interactive-usage record: a single vendor session's spend on a
/// single model (ADR-0033 §3). `project`/`actor_email` are `None` when the
/// session's workspace matched no registered repo (reported, never dropped, §6).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct InteractiveRecord {
    pub agent: String,
    pub model: String,
    pub session_id: String,
    pub project: Option<String>,
    pub actor_email: Option<String>,
    pub tokens: Tokens,
    pub first_ts: String,
    pub last_ts: String,
}

/// A repo the daemon knows about, as the scan needs it: the `owner/repo` slug it
/// reports as `project`, and the filesystem path it dashed-cwd-encodes to match a
/// transcript workspace directory (ADR-0008 D10).
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredRepo {
    pub slug: String,
    pub path: String,
}

/// Everything the Claude scan reads — the deep-module seam of ADR-0033 §7: the
/// store root, the run-owned session ids to exclude, the repo registry for
/// project/actor attribution, and an optional `since` lower bound on `last_ts`.
pub struct ClaudeScan<'a> {
    pub projects_dir: &'a Path,
    pub run_session_ids: &'a HashSet<String>,
    pub repos: &'a [RegisteredRepo],
    pub since: Option<&'a str>,
}

/// Everything the Codex scan reads, mirroring [`ClaudeScan`]: `codex_dir` is the
/// `.codex` base (the scan walks its `sessions/` and `archived_sessions/`
/// subtrees), plus the run-owned ids to exclude, the repo registry for
/// attribution, and an optional `since` lower bound on `last_ts` (ADR-0033 §2).
pub struct CodexScan<'a> {
    pub codex_dir: &'a Path,
    pub run_session_ids: &'a HashSet<String>,
    pub repos: &'a [RegisteredRepo],
    pub since: Option<&'a str>,
}

/// Everything the OpenCode scan reads, mirroring [`CodexScan`]: `db_path` is the
/// `opencode.db` SQLite store (the scan opens it read-only and reads its
/// `message`/`session` tables), plus the run-owned ids to exclude, the repo
/// registry for attribution, and an optional `since` lower bound on `last_ts`
/// (ADR-0033 §2).
pub struct OpenCodeScan<'a> {
    pub db_path: &'a Path,
    pub run_session_ids: &'a HashSet<String>,
    pub repos: &'a [RegisteredRepo],
    pub since: Option<&'a str>,
}

/// Everything the Copilot scan reads, mirroring [`OpenCodeScan`]: `db_path` is the
/// `session-store.db` SQLite store (the scan COPIES it plus its `-wal`/`-shm`
/// sidecars before reading its `assistant_usage_events`/`sessions` tables — see
/// `copilot.rs`), plus the run-owned ids to exclude, the repo registry for
/// attribution, and an optional `since` lower bound on `last_ts` (ADR-0033 §2).
pub struct CopilotScan<'a> {
    pub db_path: &'a Path,
    pub run_session_ids: &'a HashSet<String>,
    pub repos: &'a [RegisteredRepo],
    pub since: Option<&'a str>,
}

/// Everything the Kimi scan reads, mirroring [`OpenCodeScan`] but with TWO store
/// roots (ADR-0033 §2): `kimi_dir` is the `.kimi` base (legacy `kimi-cli`
/// `StatusUpdate` wire files) and `kimi_code_dir` is the `.kimi-code` base
/// (`usage.record` wire files). Per-root dispatch avoids content-sniffing — the
/// format is decided by which root a `wire.jsonl` lives under. Plus the run-owned
/// ids to exclude, the repo registry for attribution, and an optional `since`
/// lower bound on `last_ts`.
pub struct KimiScan<'a> {
    pub kimi_dir: &'a Path,
    pub kimi_code_dir: &'a Path,
    pub run_session_ids: &'a HashSet<String>,
    pub repos: &'a [RegisteredRepo],
    pub since: Option<&'a str>,
}
