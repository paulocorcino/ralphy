//! The pure and small steps `run_cmd` takes before the queue work: the run
//! lock outcome, the settings and their adapter sections, the run title, and
//! the parts of the queue config read from the settings.

use ralphy_core::{git, BranchMode, Settings, Workspace};
use tracing::warn;

use super::derived::{repo_relative_plan_path, resolve_verify_timeout_minutes};
use super::lifecycle::{finish_if_idle, start_delivery, Observability};
use super::wiring::ResolvedClaude;
use crate::cli::RunArgs;
use crate::{config, runlock, runstate, telegram};

/// The `--if-idle` deferral (#222): a live run holds the lock, so this run
/// emits `run skipped` and ends with exit 0. The deferral is a run border
/// (ADR-0019 amendment): both `shutdown()`s run after the emit so the ring drains.
pub(super) fn defer_to_live_run(
    obs: &Observability,
    info: runlock::LockInfo,
    title: &str,
    ws: &Workspace,
    runid: &str,
) {
    let since = chrono::DateTime::parse_from_rfc3339(&info.started_at)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or(info.started_at);
    let msg = format!("skipped: run in progress since {since}, pid {}", info.pid);
    let repo_root = ws.repo_root();
    // No branch was cut: report the branch the repo is on.
    let (notifier, events_handle, snapshot_handle) = start_delivery(
        obs,
        title,
        0,
        &git::current_branch(repo_root).unwrap_or_default(),
        repo_root,
        ws,
        runid,
        None,
    );
    finish_if_idle(
        &obs.presenter,
        &msg,
        notifier,
        events_handle,
        snapshot_handle,
    );
}

/// Without `--if-idle` a live lock only warns; a stale lock (dead PID) is taken over.
pub(super) fn warn_on_lock(lock: &runlock::LockState) {
    match lock {
        runlock::LockState::HeldAlive(info) => {
            warn!(
                pid = info.pid,
                since = %info.started_at,
                "a run is already active in this repo — proceeding anyway"
            );
        }
        runlock::LockState::Stale(info) => {
            warn!(
                pid = info.pid,
                "ignoring stale run.lock (process not running)"
            );
        }
        runlock::LockState::Corrupt => warn!("ignoring unreadable run.lock"),
        runlock::LockState::Free => {}
    }
}

/// The settings, loaded once (ADR-0010). A load failure warns and uses the defaults.
pub(super) fn load_settings(ws: &Workspace) -> Settings {
    match Settings::load(ws) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "could not load .ralphy/settings.json — persisted defaults ignored");
            Settings::default()
        }
    }
}

/// The run title seeds both the console header and the Telegram card. A single
/// named issue titles the card with its own title; a longer list falls back to
/// "N issues". An explicit `--issues` selection is not label-scoped: no default
/// labels on the card.
pub(super) fn run_title(
    args: &RunArgs,
    repo_name: &str,
    queue: &[ralphy_core::Issue],
    forced_issues: &[u64],
    effective_labels: &[String],
) -> String {
    let single_title = if forced_issues.len() == 1 {
        queue.first().map(|i| i.title.clone())
    } else {
        None
    };
    let title_labels: &[String] = if args.issues.is_empty() {
        effective_labels
    } else {
        &[]
    };
    telegram::notifier::derive_title(
        repo_name,
        queue.len(),
        title_labels,
        single_title.as_deref(),
        args.title.as_deref(),
    )
}

/// The adapter sections of the settings. A section is opaque JSON to the core:
/// each typed slice is read here, warning and defaulting on a malformed section
/// (ADR-0002 amendment).
pub(super) struct AdapterSettings {
    pub(super) claude: ralphy_agent_claude::ClaudeSettings,
    pub(super) opencode: ralphy_agent_opencode::OpenCodeSettings,
    pub(super) copilot: ralphy_agent_copilot::CopilotSettings,
    pub(super) cursor: ralphy_agent_cursor::CursorSettings,
    pub(super) antigravity: ralphy_agent_antigravity::AntigravitySettings,
}

impl AdapterSettings {
    pub(super) fn load(settings: &Settings) -> Self {
        AdapterSettings {
            claude: section(settings, ralphy_agent_claude::ClaudeSettings::SECTION),
            opencode: section(settings, ralphy_agent_opencode::OpenCodeSettings::SECTION),
            copilot: section(settings, ralphy_agent_copilot::CopilotSettings::SECTION),
            cursor: section(settings, ralphy_agent_cursor::CursorSettings::SECTION),
            antigravity: section(
                settings,
                ralphy_agent_antigravity::AntigravitySettings::SECTION,
            ),
        }
    }
}

/// The adapter section `name` of the settings; its defaults, with a warning,
/// when the section is malformed.
fn section<T: serde::de::DeserializeOwned + Default>(settings: &Settings, name: &str) -> T {
    settings.agent_settings(name).unwrap_or_else(|e| {
        warn!(
            error = %e,
            section = name,
            "malformed agent settings section — its persisted defaults ignored"
        );
        T::default()
    })
}

/// The branch mode: the flag, else `branch_mode` in the settings, else `new`.
pub(super) fn resolve_branch_mode(args: &RunArgs, settings: &Settings) -> BranchMode {
    args.branch_mode
        .map(BranchMode::from)
        .or_else(|| {
            settings
                .branch_mode
                .as_deref()
                .and_then(|m| config::parse_branch_mode(m).ok())
        })
        .unwrap_or(BranchMode::New)
}

/// The `branch_mode` word of `run.started` (ADR-0019).
pub(super) fn branch_mode_word(mode: BranchMode) -> &'static str {
    match mode {
        BranchMode::New => "new",
        BranchMode::Current => "current",
    }
}

/// The Claude settings of this run: each knob from its flag, else its setting,
/// else its default.
pub(super) fn resolve_claude(
    args: &RunArgs,
    claude: &ralphy_agent_claude::ClaudeSettings,
    settings: &Settings,
) -> ResolvedClaude {
    ResolvedClaude {
        plan_model: config::resolve_str(args.plan_model.clone(), claude.plan_model.clone(), "opus"),
        default_exec_model: config::resolve_str(
            args.default_exec_model.clone(),
            claude.default_exec_model.clone(),
            "sonnet",
        ),
        max_minutes_per_issue: config::resolve_u64(
            args.max_minutes_per_issue,
            claude.max_minutes_per_issue,
            ralphy_core::DEFAULT_MAX_MINUTES_PER_ISSUE,
        ),
        remote_control: config::resolve_remote_control(
            args.remote_control,
            args.no_remote_control,
            settings.remote_control,
        ),
        run_without_guard_proof: claude.run_without_guard_proof_i_understand_the_risk,
    }
}

/// ADR-0011: the per-repo fallback command (one argv), used only when a plan
/// has no `## Verify` section.
pub(super) fn verify_fallback(settings: &Settings) -> Option<Vec<Vec<String>>> {
    settings
        .verify
        .command
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| vec![ralphy_core::verify::tokenize(c)])
}

/// The gate owns its clock (docs/adr/0038): the per-issue cap is `0` by default.
pub(super) fn verify_timeout(settings: &Settings) -> std::time::Duration {
    std::time::Duration::from_secs(
        resolve_verify_timeout_minutes(settings.verify.timeout_minutes).saturating_mul(60),
    )
}

/// The run-snapshot context (ADR-0047): the run's identity and the plan the panel reads.
pub(super) fn snapshot_ctx(
    runid: &str,
    obs: &Observability,
    operating_branch: &str,
    ws: &Workspace,
) -> runstate::snapshot::SnapshotCtx {
    runstate::snapshot::SnapshotCtx {
        runid: runid.to_owned(),
        pid: std::process::id(),
        repo: obs.events_slug.clone(),
        branch: operating_branch.to_owned(),
        started_at: chrono::Local::now().to_rfc3339(),
        plan_path: repo_relative_plan_path(ws.repo_root(), ws),
    }
}
