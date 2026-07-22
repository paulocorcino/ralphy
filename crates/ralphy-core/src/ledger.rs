//! The central, append-only token-usage ledger (ADR-0008 D6). One JSON object
//! per line, one line per completed phase, in `~/.ralphy/usage/<project-id>.jsonl`
//! — outside the per-run scratch so accumulation survives the run branch it never
//! pushes. The unit of truth is **tokens**; no `cost`/`usd` is ever written (D2),
//! it is derived at read-time from a price table (D8).
//!
//! Everything here is best-effort by contract: a write or parse failure logs and
//! is swallowed by the caller so token measurement never gates or breaks the
//! orchestration it observes (D9).

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use tracing::warn;

use crate::Usage;

/// One ledger line (ADR-0008 D6). Serialized as a flat JSON object whose `tokens`
/// member carries only the four numeric token fields — the top-level `model` is
/// the model key (D8), so `Usage::model` is deliberately omitted from `tokens`.
#[derive(Debug, Clone, Serialize)]
pub struct LedgerRecord {
    /// `owner/repo` git-remote slug, or a path-hash fallback (D7).
    pub project: String,
    /// `git config user.email` — the actor key (D7).
    pub actor_email: String,
    /// `git config user.name` — the actor display name (D7).
    pub actor_name: String,
    /// The orchestrator build, `env!("CARGO_PKG_VERSION")` (D6).
    pub ralphy_version: String,
    /// The issue this phase served, or `0` for a run-level phase not tied to any
    /// single issue (the end-of-run `consolidate` pass — issue #269).
    pub issue: u64,
    /// `plan` | `execute` | `protocol-repair` | `repair` (the runner's per-issue
    /// phases) | `consolidate` (the run-level knowledge-consolidation pass).
    pub phase: String,
    /// The adapter's self-reported vendor label ([`crate::Agent::name`]),
    /// opaque to the core.
    pub agent: String,
    /// The model the price table resolves on (D8), or `unknown`.
    pub model: String,
    /// The vendor session identity for the run-vs-scan dedup key (ADR-0033 §5).
    /// Absent on old lines and resumed-plan phases; skipped when `None` so the
    /// on-disk shape stays additive and append-only safe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The terminal status of this phase (D6).
    pub outcome: String,
    /// The four-way token split, written WITHOUT `Usage::model`.
    #[serde(serialize_with = "serialize_tokens")]
    pub tokens: Usage,
    /// RFC3339 UTC timestamp.
    pub ts: String,
}

/// Serialize a [`Usage`] as a `tokens` object carrying only the four numeric
/// fields — never `model`, which is the record's top-level `model` field (D6).
fn serialize_tokens<S>(usage: &Usage, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut state = serializer.serialize_struct("Tokens", 4)?;
    state.serialize_field("input", &usage.input)?;
    state.serialize_field("output", &usage.output)?;
    state.serialize_field("cache_read", &usage.cache_read)?;
    state.serialize_field("cache_creation", &usage.cache_creation)?;
    state.end()
}

/// Where phase records land, behind a trait so the queue loop unit-tests
/// against an in-memory sink. Callers treat a failure as best-effort — warn,
/// never stop the run (D9).
pub trait LedgerSink {
    fn append(&self, rec: &LedgerRecord) -> Result<()>;
}

/// The production sink: the project's JSONL file under the usage root.
pub struct FileLedger;

impl LedgerSink for FileLedger {
    fn append(&self, rec: &LedgerRecord) -> Result<()> {
        append(rec)
    }
}

/// Serialize one record to a single JSON line (no trailing newline). Pure, so the
/// field set and the no-`cost`/`usd` invariant unit-test without the filesystem.
pub fn record_line(rec: &LedgerRecord) -> Result<String> {
    Ok(serde_json::to_string(rec)?)
}

/// Sum the four token fields across every parseable JSONL line's `tokens` object.
/// Tolerant of malformed lines and missing fields — a bad line is skipped, not
/// fatal (the ledger is append-only and best-effort).
pub fn sum_tokens(jsonl: &str) -> Usage {
    let mut total = Usage::default();
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(tokens) = value.get("tokens") else {
            continue;
        };
        let field = |k: &str| tokens.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        total.input += field("input");
        total.output += field("output");
        total.cache_read += field("cache_read");
        total.cache_creation += field("cache_creation");
    }
    total
}

/// A read model over one ledger line (ADR-0008 D8/D11). Built tolerantly from a
/// `serde_json::Value` — never `#[derive(Deserialize)]` on [`LedgerRecord`], whose
/// `tokens` serializes through a custom hook — so the reader mirrors [`sum_tokens`]'s
/// "skip a malformed line" stance instead of fighting that write-side asymmetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRow {
    pub project: String,
    pub actor_email: String,
    pub actor_name: String,
    pub ralphy_version: String,
    pub issue: u64,
    pub phase: String,
    pub agent: String,
    pub model: String,
    /// The vendor session identity (ADR-0033 §5); `None` on old lines that
    /// predate the field.
    pub session_id: Option<String>,
    pub outcome: String,
    /// The four numeric token fields (never carries `model` — the row's `model` is
    /// the top-level field).
    pub tokens: Usage,
    pub ts: String,
}

/// Parse every well-formed line of a JSONL ledger string into a [`UsageRow`].
/// Tolerant: a line that does not parse as a JSON object is skipped, mirroring
/// [`sum_tokens`]. Missing string fields default to empty, missing numbers to `0`.
pub fn read_rows(jsonl: &str) -> Vec<UsageRow> {
    let mut rows = Vec::new();
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if !value.is_object() {
            continue;
        }
        let s = |k: &str| {
            value
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let n = |k: &str| value.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let tok = |k: &str| {
            value
                .get("tokens")
                .and_then(|t| t.get(k))
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        };
        rows.push(UsageRow {
            project: s("project"),
            actor_email: s("actor_email"),
            actor_name: s("actor_name"),
            ralphy_version: s("ralphy_version"),
            issue: n("issue"),
            phase: s("phase"),
            agent: s("agent"),
            model: s("model"),
            session_id: value
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            outcome: s("outcome"),
            tokens: Usage {
                input: tok("input"),
                output: tok("output"),
                cache_read: tok("cache_read"),
                cache_creation: tok("cache_creation"),
                model: None,
            },
            ts: s("ts"),
        });
    }
    rows
}

/// Read a project's whole ledger file into [`UsageRow`]s. An empty vec on a
/// missing file (nothing recorded yet) or an unresolved ledger root.
pub fn read_project_rows(slug: &str) -> Vec<UsageRow> {
    let Some(path) = ledger_path(slug) else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    read_rows(&content)
}

/// The ledger root: `$RALPHY_USAGE_DIR` when set (tests point it at a temp dir),
/// else `<home>/.ralphy/usage`. `None` when no home directory can be resolved.
fn usage_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_USAGE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".ralphy").join("usage"))
}

/// The ledger file for `slug`: `<root>/<sanitized>.jsonl`, where the `owner/repo`
/// slug's `/` is sanitized to `-` for the filename (the in-line `project` field
/// keeps the `owner/repo` form — D6).
fn ledger_path(slug: &str) -> Option<PathBuf> {
    let sanitized = slug.replace('/', "-");
    Some(usage_root()?.join(format!("{sanitized}.jsonl")))
}

/// Append one record as a JSON line to its project's ledger file, creating the
/// directory as needed. Keyed by `rec.project`.
pub fn append(rec: &LedgerRecord) -> Result<()> {
    use std::io::Write;
    let path = ledger_path(&rec.project).ok_or_else(|| anyhow!("no usage-ledger root resolved"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = record_line(rec)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Append a **run-level** phase line — a phase not tied to a single issue, namely
/// the end-of-run knowledge consolidation (ADR-0008; issue #269). `issue` is `0`,
/// the run-level sentinel: real issue numbers start at `1`, so a `0` line is a
/// run-scoped overhead cost the project total counts and a per-issue query skips.
///
/// Built and appended here (not in the runner's `RunLedger`) because consolidation
/// fires from the cli after the queue drains, outside any `IssueCtx`. Best-effort
/// like every ledger write: a `0`-token usage writes nothing (an empty overhead
/// line would only add noise), and a write failure warns rather than stops (D9).
pub fn append_run_phase(
    project: &str,
    actor_email: &str,
    actor_name: &str,
    agent: &str,
    phase: &str,
    usage: &Usage,
) {
    if usage.total() == 0 {
        return;
    }
    let rec = LedgerRecord {
        project: project.to_string(),
        actor_email: actor_email.to_string(),
        actor_name: actor_name.to_string(),
        ralphy_version: env!("CARGO_PKG_VERSION").into(),
        issue: 0,
        phase: phase.to_string(),
        agent: agent.to_string(),
        model: usage.model.clone().unwrap_or_else(|| "unknown".into()),
        session_id: None,
        outcome: "ok".into(),
        tokens: usage.clone(),
        ts: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = append(&rec) {
        warn!(phase, error = %e, "writing run-level {} usage ledger line failed", phase);
    }
}

/// The project's cumulative token totals, summed over its whole ledger file. A
/// missing file (nothing recorded yet) reads as `Usage::default()`.
pub fn project_total(slug: &str) -> Usage {
    let Some(path) = ledger_path(slug) else {
        return Usage::default();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Usage::default();
    };
    sum_tokens(&content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `RALPHY_USAGE_DIR` is process-global, so the tests that point it at a temp
    /// dir must not run concurrently — one removing it mid-way would send another's
    /// read to the real home. Serialize them behind this lock.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn sample_record() -> LedgerRecord {
        LedgerRecord {
            project: "owner/repo".into(),
            actor_email: "dev@example.com".into(),
            actor_name: "Dev Name".into(),
            ralphy_version: "0.1.0-rc5".into(),
            issue: 42,
            phase: "execute".into(),
            agent: "agent-a".into(),
            model: "model-a".into(),
            session_id: Some("sess-x".into()),
            outcome: "done".into(),
            tokens: Usage {
                input: 100,
                output: 9,
                cache_read: 1710,
                cache_creation: 94,
                model: Some("model-a".into()),
            },
            ts: "2026-06-15T12:34:56+00:00".into(),
        }
    }

    #[test]
    fn record_line_has_all_fields_and_no_cost_or_usd() {
        let line = record_line(&sample_record()).expect("serialize");
        for key in [
            "project",
            "actor_email",
            "actor_name",
            "ralphy_version",
            "issue",
            "phase",
            "agent",
            "model",
            "session_id",
            "outcome",
            "tokens",
            "ts",
        ] {
            assert!(
                line.contains(&format!("\"{key}\"")),
                "record must carry the `{key}` key: {line}"
            );
        }
        // The four token sub-fields are present...
        for key in ["input", "output", "cache_read", "cache_creation"] {
            assert!(line.contains(&format!("\"{key}\"")), "tokens.{key}: {line}");
        }
        // ...but never the model inside `tokens` (it is the top-level field), and
        // never a derived cost — USD is a read-time projection, never stored (D2).
        assert!(
            !line.contains("cost") && !line.contains("usd"),
            "no cost/usd may be written to the ledger: {line}"
        );
    }

    #[test]
    fn sum_tokens_adds_four_fields_across_lines() {
        let jsonl = "\
{\"phase\":\"plan\",\"tokens\":{\"input\":10,\"output\":1,\"cache_read\":100,\"cache_creation\":5}}
{\"phase\":\"execute\",\"tokens\":{\"input\":20,\"output\":2,\"cache_read\":200,\"cache_creation\":7}}
";
        let total = sum_tokens(jsonl);
        assert_eq!(total.input, 30);
        assert_eq!(total.output, 3);
        assert_eq!(total.cache_read, 300);
        assert_eq!(total.cache_creation, 12);
    }

    #[test]
    fn read_rows_parses_good_lines_and_skips_malformed() {
        // Two well-formed lines and one malformed (unparseable) middle line.
        let jsonl = "\
{\"project\":\"owner/repo\",\"actor_email\":\"a@x.io\",\"actor_name\":\"A\",\"ralphy_version\":\"rc5\",\"issue\":42,\"phase\":\"plan\",\"agent\":\"agent-a\",\"model\":\"model-a\",\"outcome\":\"ok\",\"tokens\":{\"input\":10,\"output\":1,\"cache_read\":100,\"cache_creation\":5},\"ts\":\"2026-06-15T12:00:00+00:00\"}
{ this is not valid json
{\"project\":\"owner/repo\",\"actor_email\":\"b@x.io\",\"actor_name\":\"B\",\"ralphy_version\":\"rc5\",\"issue\":42,\"phase\":\"execute\",\"agent\":\"agent-b\",\"model\":\"model-b\",\"outcome\":\"done\",\"tokens\":{\"input\":20,\"output\":2,\"cache_read\":200,\"cache_creation\":7},\"ts\":\"2026-06-15T12:05:00+00:00\"}
";
        let rows = read_rows(jsonl);
        assert_eq!(rows.len(), 2, "malformed middle line is skipped");

        assert_eq!(rows[0].model, "model-a");
        assert_eq!(rows[0].phase, "plan");
        assert_eq!(rows[0].actor_email, "a@x.io");
        assert_eq!(rows[0].issue, 42);
        assert_eq!(rows[0].tokens.input, 10);
        assert_eq!(rows[0].tokens.cache_read, 100);
        assert_eq!(rows[0].ts, "2026-06-15T12:00:00+00:00");

        assert_eq!(rows[1].model, "model-b");
        assert_eq!(rows[1].phase, "execute");
        assert_eq!(rows[1].agent, "agent-b");
        assert_eq!(rows[1].tokens.output, 2);
    }

    #[test]
    fn read_rows_parses_mixed_session_id() {
        // One OLD line lacking `session_id`, one NEW line carrying it — both must
        // parse into a `UsageRow`, neither skipped (additive, append-only safe).
        let jsonl = "\
{\"project\":\"owner/repo\",\"issue\":42,\"phase\":\"plan\",\"agent\":\"a\",\"model\":\"m\",\"outcome\":\"ok\",\"tokens\":{\"input\":1,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"t\"}
{\"project\":\"owner/repo\",\"issue\":42,\"phase\":\"execute\",\"agent\":\"a\",\"model\":\"m\",\"session_id\":\"sess-b\",\"outcome\":\"done\",\"tokens\":{\"input\":2,\"output\":0,\"cache_read\":0,\"cache_creation\":0},\"ts\":\"t\"}
";
        let rows = read_rows(jsonl);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].session_id.is_none(), "old line has no session_id");
        assert_eq!(rows[1].session_id.as_deref(), Some("sess-b"));
    }

    #[test]
    fn append_then_project_total_round_trips() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Point the ledger root at a unique temp dir so production is untouched.
        let dir = std::env::temp_dir().join(format!(
            "ralphy-ledger-{}-{:x}",
            std::process::id(),
            sample_record().issue
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("RALPHY_USAGE_DIR", &dir);

        let mut first = sample_record();
        first.phase = "plan".into();
        first.tokens = Usage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_creation: 4,
            model: None,
        };
        let mut second = sample_record();
        second.tokens = Usage {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_creation: 40,
            model: None,
        };
        append(&first).expect("append first");
        append(&second).expect("append second");

        let total = project_total(&sample_record().project);
        assert_eq!(total.input, 11);
        assert_eq!(total.output, 22);
        assert_eq!(total.cache_read, 33);
        assert_eq!(total.cache_creation, 44);

        std::env::remove_var("RALPHY_USAGE_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Issue #269: the run-level `consolidate` line lands with `issue = 0`, counts
    /// toward the project total, and is skipped by a per-issue read — while a
    /// zero-token pass writes nothing at all.
    #[test]
    fn append_run_phase_records_a_run_level_consolidate_line() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir =
            std::env::temp_dir().join(format!("ralphy-ledger-runphase-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("RALPHY_USAGE_DIR", &dir);

        let usage = Usage {
            input: 33_398,
            output: 5_444,
            cache_read: 337_152,
            cache_creation: 0,
            model: Some("composer-2.5".into()),
        };
        // A zero-token pass is a no-op: no line, no file.
        append_run_phase(
            "owner/repo",
            "dev@example.com",
            "Dev Name",
            "cursor",
            "consolidate",
            &Usage::default(),
        );
        assert_eq!(
            project_total("owner/repo").total(),
            0,
            "a zero-token consolidation must write nothing"
        );

        append_run_phase(
            "owner/repo",
            "dev@example.com",
            "Dev Name",
            "cursor",
            "consolidate",
            &usage,
        );

        // It counts toward the project total.
        assert_eq!(project_total("owner/repo").total(), usage.total());

        // The line is a run-level `consolidate` phase at issue 0, agent/model intact.
        let rows = read_project_rows("owner/repo");
        assert_eq!(rows.len(), 1, "exactly the one non-zero line was written");
        let row = &rows[0];
        assert_eq!(row.issue, 0, "run-level sentinel");
        assert_eq!(row.phase, "consolidate");
        assert_eq!(row.agent, "cursor");
        assert_eq!(row.model, "composer-2.5");
        assert_eq!(row.outcome, "ok");
        assert_eq!(row.tokens.cache_read, 337_152);

        std::env::remove_var("RALPHY_USAGE_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
