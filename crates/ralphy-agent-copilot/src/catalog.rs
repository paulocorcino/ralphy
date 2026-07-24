//! The model catalog, learned for FREE from one `copilot` subprocess.
//!
//! Copilot fetches its whole model list from CAPI at startup and logs it, so a
//! probe that starts the CLI and then fails model *selection* answers three
//! questions in one process and costs zero model calls: the operator is logged in
//! (the fetch needs a session), the account is entitled to pin a model, and the
//! per-model rate card / effort support table is on disk.
//!
//! The probe is judged by the PRESENCE of the CAPI log line, never by the exit
//! status: the failed-model probe has been observed exiting both `0` and `1` on
//! the same host/CLI version (ADR-0041; see the issue #231 evidence doc), while
//! the log line is the actual evidence the fetch succeeded.
//!
//! Nothing about the catalog is hardcoded here — the vendor's list is the only
//! source, and a `no_hardcoded_model_table` test keeps it that way.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use ralphy_adapter_support::{resolve_program, run_headless};
use serde_json::Value;

use crate::command::mint_session_id;

/// The marker the vendor logs immediately before the raw CAPI payload. Everything
/// after it on the same line is a JSON object.
const CATALOG_MARKER: &str = "fetched models from CAPI /models ";

/// The marker the vendor logs for the model it actually settled on.
const DEFAULT_MODEL_MARKER: &str = "Using default model: ";

/// The binary, named once. Matches `Agent::cli_name()` on the CLI side.
const COPILOT_BIN: &str = "copilot";

/// The invalid `--model` value that makes selection fail after the catalog fetch.
/// Verified free: the CLI exits before any model call.
const UNSELECTABLE_SENTINEL: &str = "zzz-not-real";

/// Surfaced when the probe ran but logged no CAPI model list — a logged-out
/// operator, an account that cannot pin a model, or an inherited token that made
/// the CLI refuse to start. Actionable, never a panic.
pub const COPILOT_CATALOG_ERROR_MSG: &str = "Copilot model catalog unavailable (the preflight probe logged no CAPI model list) — run `copilot login`, then confirm the account can pin a model with `copilot --model <id>`";

/// Surfaced when the probe fetched the catalog but the vendor never rejected the
/// deliberately invalid `--model`: the probe may have run a BILLED turn, so the
/// result is refused rather than trusted.
pub const COPILOT_PROBE_BILLED_MSG: &str = "Copilot preflight probe did not reject its invalid --model: the probe may have spent a model call — treat the catalog as unavailable and report it upstream";

/// The rate card of one model, in nano-AIU per 1M tokens as the vendor reports it.
/// NOT a cost model: Copilot bills in AI credits with an independent per-model
/// request multiplier, so these numbers are exposed, never spent (ADR-0041 D6).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CopilotPrices {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Absent from the payload for models the vendor does not cap here.
    pub max_prompt_tokens: Option<u64>,
}

/// One catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotModel {
    pub id: String,
    /// `model_picker_enabled`: the operator can pin this id with `--model`.
    pub selectable: bool,
    /// The plan tiers this model is limited to. EMPTY means every tier — the
    /// vendor omits the key entirely rather than listing all of them.
    pub restricted_to: Vec<String>,
    /// `capabilities.supports.reasoning_effort`, or `None` when the model takes no
    /// effort argument (the key is ABSENT, not null, for those).
    pub reasoning_effort: Option<Vec<String>>,
    pub prices: CopilotPrices,
    /// The entry carries a second, long-context rate card.
    pub long_context: bool,
}

/// The catalog as one probe observed it. Account-scoped: `default_model` and every
/// `restricted_to` reflect the probing operator's plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotCatalog {
    pub models: Vec<CopilotModel>,
    pub default_model: Option<String>,
    /// The session id the probe itself minted — the key to prove it wrote no usage.
    pub probe_session_id: String,
}

impl CopilotCatalog {
    pub fn get(&self, id: &str) -> Option<&CopilotModel> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn selectable(&self) -> impl Iterator<Item = &CopilotModel> {
        self.models.iter().filter(|m| m.selectable)
    }
}

/// Read the raw string array under `models` — the vendor nests the payload as a
/// JSON *string*, not an array.
fn models_array(obj: &Value) -> Result<Vec<Value>> {
    let raw = obj
        .get("models")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("the CAPI payload has no `models` string"))?;
    let parsed: Value = serde_json::from_str(raw).context("parsing the nested `models` array")?;
    match parsed {
        Value::Array(v) => Ok(v),
        _ => Err(anyhow!("`models` is not a JSON array")),
    }
}

fn string_list(v: Option<&Value>) -> Option<Vec<String>> {
    let arr = v?.as_array()?;
    Some(
        arr.iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    )
}

fn prices_of(default_card: Option<&Value>) -> CopilotPrices {
    let get = |k: &str| {
        default_card
            .and_then(|c| c.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    CopilotPrices {
        input: get("input_price"),
        output: get("output_price"),
        cache_read: get("cache_read_price"),
        cache_write: get("cache_write_price"),
        max_prompt_tokens: default_card
            .and_then(|c| c.get("max_prompt_tokens"))
            .and_then(Value::as_u64),
    }
}

fn model_of(entry: &Value) -> Result<CopilotModel> {
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("a catalog entry has no `id`"))?
        .to_owned();
    let billing = entry.get("billing");
    let token_prices = billing.and_then(|b| b.get("token_prices"));
    Ok(CopilotModel {
        selectable: entry
            .get("model_picker_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        // ABSENT `restricted_to` = available on every tier.
        restricted_to: string_list(billing.and_then(|b| b.get("restricted_to")))
            .unwrap_or_default(),
        reasoning_effort: string_list(
            entry
                .get("capabilities")
                .and_then(|c| c.get("supports"))
                .and_then(|s| s.get("reasoning_effort")),
        ),
        prices: prices_of(token_prices.and_then(|t| t.get("default"))),
        long_context: token_prices
            .and_then(|t| t.get("long_context"))
            .is_some_and(|v| !v.is_null()),
        id,
    })
}

/// Parse a `copilot --log-level all` PROBE log into the catalog.
/// `probe_session_id` is the id the caller minted for the probe, carried through
/// so a caller can prove the probe billed nothing.
///
/// The freeness of the probe rests on model SELECTION failing after the catalog
/// fetch, so this refuses a log that shows no rejection of the sentinel id: a
/// vendor that ever fell back to a working model would otherwise turn a login
/// check into a billed, `--allow-all-tools` turn, silently.
pub fn parse_catalog(log: &str, probe_session_id: &str) -> Result<CopilotCatalog> {
    let payload = log
        .lines()
        .find_map(|l| l.split_once(CATALOG_MARKER).map(|(_, rest)| rest))
        .ok_or_else(|| anyhow!("{COPILOT_CATALOG_ERROR_MSG}"))?;
    let obj: Value = serde_json::from_str(payload.trim()).context("parsing the CAPI payload")?;
    // The payload's own `count` is ignored: the array is the fact.
    let models = models_array(&obj)?
        .iter()
        .map(model_of)
        .collect::<Result<Vec<_>>>()?;
    let default_model = log.lines().find_map(|l| {
        l.split_once(DEFAULT_MODEL_MARKER)
            .map(|(_, rest)| rest.trim().to_owned())
            .filter(|s| !s.is_empty())
    });
    if !log
        .lines()
        .any(|l| l.contains(UNSELECTABLE_SENTINEL) && l.contains("not available"))
    {
        return Err(anyhow!("{COPILOT_PROBE_BILLED_MSG}"));
    }
    Ok(CopilotCatalog {
        models,
        default_model,
        probe_session_id: probe_session_id.to_owned(),
    })
}

/// Run the free probe and return the catalog.
///
/// One subprocess answers auth + entitlement + catalog: the CLI starts (proving
/// the OAuth session), fetches the list from CAPI, then fails to select the
/// deliberately invalid model and exits without ever calling one. The five
/// blast-radius flags and the three `env_remove`s mirror the adapter's own
/// contract (ADR-0041 D7/D8) — without `--disable-builtin-mcps` a mere login check
/// would CONNECT the bundled, credential-bearing MCP server.
///
/// `-p` carries two bytes because the CLI's "no prompt provided" check fires
/// BEFORE model validation: a prompt-less probe never reaches the fetch.
///
/// The `TempDir` guard stays alive until the log has been read into memory; its
/// `Drop` is the only cleanup, and nothing is written inside the repo.
pub fn fetch_catalog() -> Result<CopilotCatalog> {
    let session_id = mint_session_id();
    let dir = tempfile::tempdir().context("creating the probe log dir")?;
    let mut cmd = std::process::Command::new(resolve_program(COPILOT_BIN));
    // The cwd is the throwaway temp dir, NOT the operator's repo: with
    // `--allow-all-tools`, Copilot loads repo instructions and `*/skills/`
    // cwd-relatively (ADR-0041 D9), and a login check must not execute an
    // untrusted repo's instructions.
    cmd.current_dir(dir.path())
        .arg("-p")
        .arg("hi")
        .arg("--model")
        .arg(UNSELECTABLE_SENTINEL)
        .arg("--allow-all-tools")
        .arg("--session-id")
        .arg(&session_id)
        .arg("--no-remote")
        .arg("--no-remote-export")
        .arg("--disable-builtin-mcps")
        .arg("--no-auto-update")
        .arg("--no-ask-user")
        .arg("--log-level")
        .arg("all")
        .arg("--log-dir")
        .arg(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_remove("COPILOT_GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN");
    // A wedged child must be killed, not left hanging `ralphy init` forever.
    let out = run_headless(cmd, "", Duration::from_secs(120))
        .context("running the Copilot catalog probe")?;
    if out.timed_out {
        return Err(anyhow!(
            "the Copilot catalog probe was killed at its 120s budget"
        ));
    }
    // The exit status is never inspected: it is 0 on some hosts and 1 on others
    // for the very same intended failure.
    let mut log = String::new();
    if let Ok(entries) = std::fs::read_dir(dir.path()) {
        for entry in entries.flatten() {
            let path = entry.path();
            // A fresh dir yields one `process-<epoch>-<pid>.log`, but the name is
            // the vendor's to choose — match the extension case-insensitively.
            if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("log"))
            {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    log.push_str(&text);
                    log.push('\n');
                }
            }
        }
    }
    // Both streams carry the same markers under `--log-level all`, and the
    // rejection notice lands on stderr even when no log file was written.
    log.push_str(&out.stdout);
    log.push('\n');
    log.push_str(&out.stderr);
    let catalog = parse_catalog(&log, &session_id);
    drop(dir);
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/capi-models-2026-07-20.log");

    fn fixture() -> CopilotCatalog {
        parse_catalog(FIXTURE, "probe-1").expect("the fixture parses")
    }

    #[test]
    fn parses_the_live_catalog_fixture() {
        let cat = fixture();
        assert_eq!(cat.models.len(), 46);
        assert_eq!(cat.selectable().count(), 15);
        assert_eq!(cat.default_model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(cat.probe_session_id, "probe-1");
    }

    #[test]
    fn exposes_selectability_effort_and_rate_card() {
        let cat = fixture();

        let sonnet = cat.get("claude-sonnet-5").expect("claude-sonnet-5 present");
        assert!(sonnet.selectable);
        assert_eq!(
            sonnet.reasoning_effort.as_deref(),
            Some(
                ["low", "medium", "high", "xhigh", "max"]
                    .map(String::from)
                    .as_slice()
            )
        );
        assert_eq!(
            sonnet.prices,
            CopilotPrices {
                input: 200,
                output: 1000,
                cache_read: 20,
                cache_write: 250,
                max_prompt_tokens: Some(200_000),
            }
        );
        assert_eq!(
            sonnet.restricted_to,
            ["pro", "pro_plus", "business", "enterprise", "max"].map(String::from)
        );
        assert!(sonnet.long_context);
        // 33 of the 46 entries omit the key entirely — the flag discriminates.
        assert!(!cat.get("gpt-5-mini").expect("present").long_context);

        // An ABSENT `restricted_to` means every tier, and an absent
        // `max_prompt_tokens` means the vendor caps nothing here.
        let mini = cat.get("gpt-5-mini").expect("gpt-5-mini present");
        assert!(mini.restricted_to.is_empty());
        assert_eq!(mini.prices.max_prompt_tokens, None);

        assert!(!cat.get("claude-opus-4.8").expect("present").selectable);

        for id in [
            "kimi-k2.7-code",
            "claude-sonnet-4.5",
            "claude-haiku-4.5",
            "gemini-2.5-pro",
        ] {
            assert_eq!(
                cat.get(id)
                    .unwrap_or_else(|| panic!("{id} present"))
                    .reasoning_effort,
                None,
                "{id} takes no effort argument"
            );
        }
    }

    /// The literal logged-out stderr block (spike §5): no CAPI line anywhere.
    const LOGGED_OUT: &str = "Error: No authentication information found.\n\
        Copilot can be authenticated with GitHub using an OAuth Token or a Fine-Grained\n\
        Personal Access Token.\n\
        To authenticate, you can use any of the following methods:\n\
          • Start 'copilot' and run the '/login' command\n\
          • Set the COPILOT_GITHUB_TOKEN, GH_TOKEN, or GITHUB_TOKEN environment variable\n\
          • Run 'gh auth login' to authenticate with the GitHub CLI";

    #[test]
    fn a_log_without_the_catalog_line_is_a_readable_error() {
        let err = parse_catalog(LOGGED_OUT, "probe-1").expect_err("no catalog line");
        assert_eq!(err.to_string(), COPILOT_CATALOG_ERROR_MSG);
        assert!(parse_catalog("", "probe-1").is_err());
    }

    #[test]
    fn malformed_catalog_json_is_an_error_not_a_panic() {
        let truncated = format!(
            "2026-07-20T00:00:00.000Z [DEBUG] {CATALOG_MARKER}{}",
            r#"{"count":46,"models":"[{\"id\":\"x\""}"#
        );
        let err = parse_catalog(&truncated, "probe-1").expect_err("truncated array");
        // The marker WAS found; the payload was not parseable — the error must name
        // the payload, not the missing marker and not the freeness guard.
        assert!(
            format!("{err:#}").contains("parsing the nested `models` array"),
            "error chain: {err:#}"
        );
    }

    /// The freeness guard: a probe log whose catalog fetch succeeded but which
    /// shows NO rejection of the sentinel model may have run a billed turn, so it
    /// is refused. Red before the guard existed — the fixture minus its warning
    /// line parsed happily.
    #[test]
    fn a_probe_that_never_rejected_the_model_is_refused() {
        let no_rejection: String = FIXTURE
            .lines()
            .filter(|l| !l.contains(UNSELECTABLE_SENTINEL))
            .collect::<Vec<_>>()
            .join("\n");
        let err = parse_catalog(&no_rejection, "probe-1").expect_err("no rejection line");
        assert_eq!(err.to_string(), COPILOT_PROBE_BILLED_MSG);
        // The unmodified fixture still parses: the guard is not vacuous.
        assert!(parse_catalog(FIXTURE, "probe-1").is_ok());
    }

    /// The catalog is the vendor's to publish: no id, price or effort list may be
    /// baked into the non-test half of this file. The needles are assembled from
    /// fragments so the assertion cannot match itself.
    #[test]
    fn no_hardcoded_model_table() {
        let src = include_str!("catalog.rs");
        let head = src.split_once("mod tests").map(|(h, _)| h).unwrap_or(src);
        for needle in [
            concat!("\"", "claude-"),
            concat!("\"", "gpt-5"),
            concat!("\"", "gemini-"),
            concat!("\"", "kimi-"),
        ] {
            assert!(
                !head.contains(needle),
                "hardcoded model table: {needle} appears outside the tests"
            );
        }
    }

    /// The one test that proves the artifact RUNS — and that it runs for FREE.
    /// `#[ignore]`d (network-bound). It self-skips where `copilot` is absent
    /// (Linux CI), but LOUDLY: set `RALPHY_LIVE_COPILOT` and a missing binary
    /// FAILS, so a lane that claims to run this cannot pass by skipping.
    /// Invoked by its own `## Verify` line.
    #[test]
    #[ignore]
    fn live_probe_fetches_the_catalog_for_free() {
        let required = std::env::var_os("RALPHY_LIVE_COPILOT").is_some();
        if ralphy_adapter_support::locate_program(COPILOT_BIN).is_none() {
            assert!(
                !required,
                "RALPHY_LIVE_COPILOT is set but no `{COPILOT_BIN}` binary is on this host"
            );
            eprintln!("copilot absent — skipping the live probe");
            return;
        }

        // POSITIVE CONTROL, before the zero-usage oracle: `copilot_usage` funnels
        // every failure (no home, missing store, schema drift) to
        // `Usage::default()`, so a zero below would otherwise be indistinguishable
        // from a dead reader. Prove the reader is live on THIS host first.
        let db = crate::usage::copilot_store_db().expect("a Copilot home resolves");
        assert!(db.exists(), "no session store at {}", db.display());
        let records = ralphy_usage_scan::scan_copilot(&ralphy_usage_scan::CopilotScan {
            db_path: &db,
            run_session_ids: &std::collections::HashSet::new(),
            repos: &[],
            since: None,
        });
        let control = records
            .iter()
            .find(|r| r.tokens.as_ref().is_some_and(|t| t.input + t.output > 0))
            .expect(
                "the store holds no billed session — the zero-usage oracle would prove nothing",
            );
        assert_ne!(
            crate::usage::copilot_usage(&control.session_id),
            ralphy_core::Usage::default(),
            "the usage reader is dead on this host; the oracle below is vacuous"
        );

        let cat = fetch_catalog().expect("the live probe returns a catalog");
        assert!(cat.models.len() >= 40, "models: {}", cat.models.len());
        assert!(
            cat.selectable().count() >= 10,
            "selectable: {}",
            cat.selectable().count()
        );
        assert!(cat.default_model.is_some());
        // The zero-model-calls oracle: the probe's own session billed nothing.
        assert_eq!(
            crate::usage::copilot_usage(&cat.probe_session_id),
            ralphy_core::Usage::default()
        );
    }
}
