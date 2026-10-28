//! `ralphy config get`: the resolved value of every supported key, as text
//! for a person or as JSON for the daemon's config Query verb.

use anyhow::Result;
use ralphy_agent_claude::ClaudeSettings;
use ralphy_agent_copilot::CopilotSettings;
use ralphy_agent_cursor::CursorSettings;
use ralphy_agent_gemini::GeminiSettings;
use ralphy_agent_opencode::OpenCodeSettings;
use ralphy_core::{git, Settings, Workspace};

pub fn get(ws: &Workspace, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(&config_json(ws)?)?);
        return Ok(());
    }
    let s = Settings::load(ws)?;
    let opencode: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION)?;
    let claude: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION)?;
    let copilot: CopilotSettings = s.agent_settings(CopilotSettings::SECTION)?;
    let cursor: CursorSettings = s.agent_settings(CursorSettings::SECTION)?;
    let gemini: GeminiSettings = s.agent_settings(GeminiSettings::SECTION)?;
    print_str("opencode.model", opencode.model);
    print_str("verify.command", s.verify.command);
    match s.verify.require_verify_gate {
        Some(b) => println!("verify.require_verify_gate = {b}"),
        None => println!("verify.require_verify_gate: not set"),
    }
    print_str("base_branch", s.base_branch);
    print_str("branch_mode", s.branch_mode);
    print_str("files.encoding", s.files.encoding);
    match s.files.upload_max_mb {
        Some(n) => println!("files.upload_max_mb = {n}"),
        None => println!("files.upload_max_mb: not set"),
    }
    match s.runs.keep {
        Some(n) => println!("runs.keep = {n}"),
        None => println!("runs.keep: not set"),
    }
    match s.remote_control {
        Some(b) => println!("remote_control = {b}"),
        None => println!("remote_control: not set"),
    }
    print_str("queue.assignee", s.queue.assignee);
    match s.queue.trust_all_comments {
        Some(b) => println!("queue.trust_all_comments = {b}"),
        None => println!("queue.trust_all_comments: not set"),
    }
    print_str("claude.plan_model", claude.plan_model);
    print_str("claude.plan_effort", claude.plan_effort);
    print_str("claude.default_exec_model", claude.default_exec_model);
    print_str("claude.exec_effort", claude.exec_effort);
    match claude.max_minutes_per_issue {
        Some(n) => println!("claude.max_minutes_per_issue = {n}"),
        None => println!("claude.max_minutes_per_issue: not set"),
    }
    println!("claude.console_name = {}", claude.console_name);
    println!(
        "claude.console_classic_screen = {}",
        claude.console_classic_screen
    );
    println!(
        "claude.run_without_guard_proof_i_understand_the_risk = {}",
        claude.run_without_guard_proof_i_understand_the_risk
    );
    print_str("copilot.plan_model", copilot.plan_model);
    print_str("copilot.exec_model", copilot.exec_model);
    print_str("copilot.plan_effort", copilot.plan_effort);
    print_str("copilot.exec_effort", copilot.exec_effort);
    println!(
        "copilot.allow_builtin_mcp_servers_i_understand_the_risk = {}",
        copilot.allow_builtin_mcp_servers_i_understand_the_risk
    );
    println!(
        "cursor.allow_codebase_indexing_i_understand_the_risk = {}",
        cursor.allow_codebase_indexing_i_understand_the_risk
    );
    print_str("gemini.plan_model", gemini.plan_model);
    print_str("gemini.exec_model", gemini.exec_model);
    // The CloudEvents sink knobs come from the global per-repo store, printed for
    // the current repo's slug (the token masked).
    let slug = git::project_slug(ws.repo_root());
    let events = crate::events::config::EventsStore::load().unwrap_or_default();
    let entry = events.entry(&slug);
    print_str("events.url", entry.and_then(|e| e.url.clone()));
    match entry.and_then(|e| e.token.as_deref()) {
        Some(t) => println!(
            "events.token = {}",
            crate::telegram::config::masked_token(t)
        ),
        None => println!("events.token: not set"),
    }
    Ok(())
}

/// Build the resolved-config JSON object the daemon's config Query verb reads:
/// every supported key mapped to its resolved value or JSON `null`. `events.token`
/// is MASKED (never echoed in full), mirroring the human `get`. Keys match
/// [`super::SUPPORTED_KEYS`]; the daemon parses this verbatim.
pub(super) fn config_json(ws: &Workspace) -> Result<serde_json::Value> {
    let s = Settings::load(ws)?;
    let opencode: OpenCodeSettings = s.agent_settings(OpenCodeSettings::SECTION)?;
    let claude: ClaudeSettings = s.agent_settings(ClaudeSettings::SECTION)?;
    let copilot: CopilotSettings = s.agent_settings(CopilotSettings::SECTION)?;
    let cursor: CursorSettings = s.agent_settings(CursorSettings::SECTION)?;
    let gemini: GeminiSettings = s.agent_settings(GeminiSettings::SECTION)?;
    let slug = git::project_slug(ws.repo_root());
    let events = crate::events::config::EventsStore::load().unwrap_or_default();
    let entry = events.entry(&slug);
    let masked_token = entry
        .and_then(|e| e.token.as_deref())
        .map(crate::telegram::config::masked_token);
    Ok(serde_json::json!({
        "opencode.model": opencode.model,
        "base_branch": s.base_branch,
        "branch_mode": s.branch_mode,
        "remote_control": s.remote_control,
        "queue.assignee": s.queue.assignee,
        "queue.trust_all_comments": s.queue.trust_all_comments,
        "verify.command": s.verify.command,
        "verify.require_verify_gate": s.verify.require_verify_gate,
        "files.encoding": s.files.encoding,
        "files.upload_max_mb": s.files.upload_max_mb,
        "runs.keep": s.runs.keep,
        "events.url": entry.and_then(|e| e.url.clone()),
        "events.token": masked_token,
        "claude.plan_model": claude.plan_model,
        "claude.plan_effort": claude.plan_effort,
        "claude.default_exec_model": claude.default_exec_model,
        "claude.exec_effort": claude.exec_effort,
        "claude.max_minutes_per_issue": claude.max_minutes_per_issue,
        "claude.console_name": claude.console_name,
        "claude.console_classic_screen": claude.console_classic_screen,
        "claude.run_without_guard_proof_i_understand_the_risk":
            claude.run_without_guard_proof_i_understand_the_risk,
        "copilot.plan_model": copilot.plan_model,
        "copilot.exec_model": copilot.exec_model,
        "copilot.plan_effort": copilot.plan_effort,
        "copilot.exec_effort": copilot.exec_effort,
        "copilot.allow_builtin_mcp_servers_i_understand_the_risk":
            copilot.allow_builtin_mcp_servers_i_understand_the_risk,
        "cursor.allow_codebase_indexing_i_understand_the_risk":
            cursor.allow_codebase_indexing_i_understand_the_risk,
        "gemini.plan_model": gemini.plan_model,
        "gemini.exec_model": gemini.exec_model,
    }))
}

/// Print one `key = value` / `key: not set` line for an optional string knob,
/// treating an empty string as unset.
fn print_str(key: &str, value: Option<String>) {
    match value.filter(|v| !v.is_empty()) {
        Some(v) => println!("{key} = {v}"),
        None => println!("{key}: not set"),
    }
}
