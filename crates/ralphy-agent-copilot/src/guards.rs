//! In-band assertions that Copilot's blast-radius flags actually took effect
//! (ADR-0041 D7/D11). `command.rs` only *sends* the flags; this module proves
//! the vendor honoured them, and fails the run when it cannot.
//!
//! Two independent guards:
//! - D7 — the builtin-MCP receipt: `--disable-builtin-mcps` is observable in the
//!   JSONL stream as a `session.mcp_servers_loaded` record. A builtin server
//!   still `connected` holds the operator's GitHub credential and can open a PR
//!   without `git push`, so it fails the run; an ABSENT receipt fails too
//!   (fail closed — an unverifiable kill switch is not a verified one).
//! - D11 — `continueOnAutoMode`: read from the vendor's GLOBAL config
//!   (`$COPILOT_HOME/config.json`, else `<home>/.copilot/config.json`). A
//!   repo-level `settings.json` may also carry config keys, but its path is
//!   *unverified — not documented in `copilot help`* — so it is not read here.

use std::path::{Path, PathBuf};

/// Scan a Copilot JSONL stream for a builtin MCP server that survived
/// `--disable-builtin-mcps`. `None` means the receipt was seen and every builtin
/// was off; `Some(msg)` is a run-failing violation.
///
/// The scan deliberately applies **no `ephemeral` filter**, unlike
/// `copilot_final_text`: a live probe (`copilot 1.0.71`, 2026-07-20) emitted the
/// receipt three times with every copy carrying `"ephemeral":true`, so filtering
/// ephemerals would find nothing and fail closed on every run.
pub(crate) fn builtin_mcp_violation(stdout: &str) -> Option<String> {
    let mut saw_receipt = false;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("session.mcp_servers_loaded") {
            continue;
        }
        saw_receipt = true;
        let servers = v
            .get("data")
            .and_then(|d| d.get("servers"))
            .and_then(|s| s.as_array());
        for server in servers.into_iter().flatten() {
            let field = |k: &str| server.get(k).and_then(|x| x.as_str()).unwrap_or_default();
            if field("source") == "builtin" && field("status") == "connected" {
                let name = field("name");
                return Some(format!(
                    "Copilot's builtin MCP server `{name}` is CONNECTED despite \
                     --disable-builtin-mcps; it holds the operator's GitHub credential \
                     and can open a PR without `git push` (ADR-0041 D7)"
                ));
            }
        }
    }
    if !saw_receipt {
        return Some(
            "no session.mcp_servers_loaded receipt in the Copilot stream — the \
             builtin-MCP kill switch is unverifiable, failing closed (ADR-0041 D7)"
                .into(),
        );
    }
    None
}

/// Drop whole-line `//` comments. Copilot's `config.json` is JSONC — the file on
/// a real host opens with two `//` lines, which `serde_json` rejects outright.
/// Block comments are deliberately not handled: none were observed, and a naive
/// `/* */` stripper would corrupt string literals.
pub(crate) fn strip_jsonc_line_comments(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `$COPILOT_HOME/config.json`, else `<home>/.copilot/config.json`
/// (`USERPROFILE` on Windows, `HOME` elsewhere). `None` when no home is known.
pub(crate) fn copilot_config_path() -> Option<PathBuf> {
    ralphy_adapter_support::home_scoped_path(
        std::env::var_os("COPILOT_HOME"),
        Path::new(".copilot"),
        Path::new("config.json"),
    )
}

/// `Some(msg)` only when `continueOnAutoMode` is literally `true`. An absent key
/// takes the documented default (`false`), and an UNPARSABLE file is a pass:
/// failing every run over an unreadable machine-managed file trades one silent
/// risk for a loud outage.
pub(crate) fn continue_on_auto_mode_violation(config_src: &str) -> Option<String> {
    let stripped = strip_jsonc_line_comments(config_src);
    let parsed = match serde_json::from_str::<serde_json::Value>(&stripped) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "could not parse Copilot's config.json ({e}); assuming continueOnAutoMode is off"
            );
            return None;
        }
    };
    if parsed.get("continueOnAutoMode").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }
    let path = copilot_config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<copilot config>".into());
    Some(format!(
        "Copilot's `continueOnAutoMode` is enabled in {path}: a vendor-internal retry \
         that silently switches model and hides a rate limit from Ralphy (ADR-0041 D11) \
         — set it to false"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continue_on_auto_mode_true_fails() {
        let msg = continue_on_auto_mode_violation(r#"{"continueOnAutoMode": true}"#)
            .expect("true must fail");
        assert!(msg.contains("continueOnAutoMode"), "{msg}");
    }

    #[test]
    fn continue_on_auto_mode_absent_or_false_passes() {
        assert_eq!(continue_on_auto_mode_violation("{}"), None);
        assert_eq!(
            continue_on_auto_mode_violation(r#"{"continueOnAutoMode": false}"#),
            None
        );
    }

    /// The real file on a live host opens with two `//` lines — `serde_json`
    /// rejects that text, so without the strip the guard would silently pass.
    #[test]
    fn copilot_config_jsonc_line_comments_are_stripped() {
        let src = "// User settings belong in settings.json.\n// This file is managed automatically.\n{\"continueOnAutoMode\": true}";
        assert!(
            serde_json::from_str::<serde_json::Value>(src).is_err(),
            "the fixture must be JSONC, not JSON"
        );
        assert!(continue_on_auto_mode_violation(src).is_some());
    }

    fn receipt(status: &str) -> String {
        format!(
            r#"{{"type":"session.mcp_servers_loaded","data":{{"servers":[{{"name":"github-mcp-server","status":"{status}","source":"builtin","transport":"http"}}]}}}}"#
        )
    }

    #[test]
    fn builtin_mcp_receipt_connected_fails_naming_the_server() {
        let stream = format!(
            "{}\n{}\n",
            r#"{"type":"assistant.message","data":{"text":"hi"}}"#,
            receipt("connected")
        );
        let msg = builtin_mcp_violation(&stream).expect("connected must fail");
        assert!(msg.contains("github-mcp-server"), "{msg}");
    }

    #[test]
    fn builtin_mcp_receipt_all_disabled_passes() {
        let stream = format!("{}\nnot json at all\n", receipt("disabled"));
        assert_eq!(builtin_mcp_violation(&stream), None);
    }

    #[test]
    fn builtin_mcp_receipt_absent_fails_closed() {
        let stream = concat!(
            r#"{"type":"assistant.message","data":{"text":"hi"}}"#,
            "\n",
            r#"{"type":"result","data":{"text":"done"}}"#,
            "\n"
        );
        let msg = builtin_mcp_violation(stream).expect("an absent receipt must fail closed");
        assert!(msg.contains("failing closed"), "{msg}");
    }

    /// The live capture: every copy of the receipt carries `"ephemeral":true`, so
    /// an ephemeral filter here would fail closed on every real run. The
    /// `ephemeral` assertion keeps that trap pinned if the fixture is regenerated.
    #[test]
    fn builtin_mcp_receipt_is_read_from_ephemeral_records() {
        let fixture = include_str!("../fixtures/mcp-servers-loaded-2026-07-20.jsonl");
        assert!(
            fixture.contains(r#""ephemeral":true"#),
            "the live receipt is ephemeral; the guard must not filter on it"
        );
        assert_eq!(builtin_mcp_violation(fixture), None);
    }
}
