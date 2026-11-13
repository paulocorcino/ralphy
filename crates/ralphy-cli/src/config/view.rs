//! `ralphy config get`: the resolved value of every supported key, as text
//! for a person or as JSON for the daemon's config Query verb.

use anyhow::Result;
use ralphy_agent_antigravity::AntigravitySettings;
use ralphy_agent_claude::ClaudeSettings;
use ralphy_agent_copilot::CopilotSettings;
use ralphy_agent_cursor::CursorSettings;
use ralphy_agent_opencode::OpenCodeSettings;
use ralphy_core::{git, Settings, Workspace};

/// Every agent section of the settings, read once for the two views.
struct AgentSections {
    opencode: OpenCodeSettings,
    claude: ClaudeSettings,
    copilot: CopilotSettings,
    cursor: CursorSettings,
    agy: AntigravitySettings,
}

impl AgentSections {
    fn load(s: &Settings) -> Result<Self> {
        Ok(Self {
            opencode: s.agent_settings(OpenCodeSettings::SECTION)?,
            claude: s.agent_settings(ClaudeSettings::SECTION)?,
            copilot: s.agent_settings(CopilotSettings::SECTION)?,
            cursor: s.agent_settings(CursorSettings::SECTION)?,
            agy: s.agent_settings(AntigravitySettings::SECTION)?,
        })
    }
}

pub(crate) fn get(ws: &Workspace, json: bool) -> Result<()> {
    if json {
        outln!("{}", serde_json::to_string(&config_json(ws)?)?);
        return Ok(());
    }
    let s = Settings::load(ws)?;
    let AgentSections {
        opencode,
        claude,
        copilot,
        cursor,
        agy,
    } = AgentSections::load(&s)?;
    if let Some(warning) = stored_gemini_warning(&s) {
        eoutln!("{warning}");
    }
    print_str("opencode.model", opencode.model);
    print_str("verify.command", s.verify.command);
    match s.verify.require_verify_gate {
        Some(b) => outln!("verify.require_verify_gate = {b}"),
        None => outln!("verify.require_verify_gate: not set"),
    }
    print_str("base_branch", s.base_branch);
    print_str("branch_mode", s.branch_mode);
    print_str("files.encoding", s.files.encoding);
    match s.files.upload_max_mb {
        Some(n) => outln!("files.upload_max_mb = {n}"),
        None => outln!("files.upload_max_mb: not set"),
    }
    match s.runs.keep {
        Some(n) => outln!("runs.keep = {n}"),
        None => outln!("runs.keep: not set"),
    }
    match s.remote_control {
        Some(b) => outln!("remote_control = {b}"),
        None => outln!("remote_control: not set"),
    }
    print_str("queue.assignee", s.queue.assignee);
    match s.queue.trust_all_comments {
        Some(b) => outln!("queue.trust_all_comments = {b}"),
        None => outln!("queue.trust_all_comments: not set"),
    }
    print_str("claude.plan_model", claude.plan_model);
    print_str("claude.plan_effort", claude.plan_effort);
    print_str("claude.default_exec_model", claude.default_exec_model);
    print_str("claude.exec_effort", claude.exec_effort);
    match claude.max_minutes_per_issue {
        Some(n) => outln!("claude.max_minutes_per_issue = {n}"),
        None => outln!("claude.max_minutes_per_issue: not set"),
    }
    outln!("claude.console_name = {}", claude.console_name);
    outln!(
        "claude.console_classic_screen = {}",
        claude.console_classic_screen
    );
    outln!(
        "claude.run_without_guard_proof_i_understand_the_risk = {}",
        claude.run_without_guard_proof_i_understand_the_risk
    );
    print_str("copilot.plan_model", copilot.plan_model);
    print_str("copilot.exec_model", copilot.exec_model);
    print_str("copilot.plan_effort", copilot.plan_effort);
    print_str("copilot.exec_effort", copilot.exec_effort);
    outln!(
        "copilot.allow_builtin_mcp_servers_i_understand_the_risk = {}",
        copilot.allow_builtin_mcp_servers_i_understand_the_risk
    );
    outln!(
        "cursor.allow_codebase_indexing_i_understand_the_risk = {}",
        cursor.allow_codebase_indexing_i_understand_the_risk
    );
    print_str("antigravity.plan_model", agy.plan_model);
    print_str("antigravity.exec_model", agy.exec_model);
    print_str("antigravity.plan_effort", agy.plan_effort);
    print_str("antigravity.exec_effort", agy.exec_effort);
    outln!(
        "antigravity.run_without_guard_proof_i_understand_the_risk = {}",
        agy.run_without_guard_proof_i_understand_the_risk
    );
    // The CloudEvents sink knobs come from the global per-repo store, printed for
    // the current repo's slug (the token masked).
    let slug = git::project_slug(ws.repo_root());
    let events = crate::events::config::EventsStore::load().unwrap_or_default();
    let entry = events.entry(&slug);
    print_str("events.url", entry.and_then(|e| e.url.clone()));
    match entry.and_then(|e| e.token.as_deref()) {
        Some(t) => outln!(
            "events.token = {}",
            crate::telegram::config::masked_token(t)
        ),
        None => outln!("events.token: not set"),
    }
    Ok(())
}

/// Build the resolved-config JSON object the daemon's config Query verb reads:
/// every supported key mapped to its resolved value or JSON `null`. `events.token`
/// is MASKED (never echoed in full), mirroring the human `get`. Keys match
/// [`super::SUPPORTED_KEYS`]; the daemon parses this verbatim.
pub(super) fn config_json(ws: &Workspace) -> Result<serde_json::Value> {
    let s = Settings::load(ws)?;
    let AgentSections {
        opencode,
        claude,
        copilot,
        cursor,
        agy,
    } = AgentSections::load(&s)?;
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
        "antigravity.plan_model": agy.plan_model,
        "antigravity.exec_model": agy.exec_model,
        "antigravity.plan_effort": agy.plan_effort,
        "antigravity.exec_effort": agy.exec_effort,
        "antigravity.run_without_guard_proof_i_understand_the_risk":
            agy.run_without_guard_proof_i_understand_the_risk,
    }))
}

/// Print one `key = value` / `key: not set` line for an optional string knob,
/// treating an empty string as unset.
fn print_str(key: &str, value: Option<String>) {
    match value.filter(|v| !v.is_empty()) {
        Some(v) => outln!("{key} = {v}"),
        None => outln!("{key}: not set"),
    }
}

/// The one warning for a `gemini` section that an older version stored: it is ignored.
pub(super) fn stored_gemini_warning(s: &Settings) -> Option<String> {
    s.extra.contains_key("gemini").then(|| {
        concat!(
            "warning: the stored gemini settings are ignored, because the Gemini agent ",
            "was removed; use antigravity.plan_model and antigravity.exec_model instead"
        )
        .to_owned()
    })
}
