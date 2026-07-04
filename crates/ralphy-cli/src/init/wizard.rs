use anyhow::{Context, Result};
use ralphy_core::{DiagnosisReport, RepoKind, Workspace};
use serde::{Deserialize, Serialize};

/// The typed config the interactive Q&A captures — the dev's confirmed/corrected
/// view of the [`DiagnosisReport`]. Each field mirrors a report field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitConfig {
    pub repo_kind: RepoKind,
    pub language_build: Option<String>,
    pub backlog_location: Option<String>,
    pub milestone_docs: Vec<String>,
    pub skills_dir: Option<String>,
    pub has_context_or_adrs: bool,
    pub remote_host: Option<String>,
    pub adopt_prd_roadmap: bool,
}

/// One onboarding stage of [`run`] that the checkpoint tracks as completed
/// (ADR-0012 stage 9). Recorded so a re-run skips a stage already done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Diagnose,
    Git,
    Scaffold,
    Skills,
    Labels,
    Issues,
}

/// The `ralphy init` checkpoint persisted to `.ralphy/init-state.json`
/// (ADR-0012 stage 9): which stages completed, the captured config (so a resume
/// skips the costly diagnosis + Q&A), and — crucially — the milestone and issue
/// numbers already published, so a re-run NEVER recreates them. `#[serde(default)]`
/// on every field keeps an older checkpoint loadable as the schema grows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitState {
    #[serde(default)]
    pub completed: Vec<Stage>,
    #[serde(default)]
    pub config: Option<InitConfig>,
    #[serde(default)]
    pub milestone_created: Option<String>,
    #[serde(default)]
    pub created_issues: Vec<u64>,
}

impl InitState {
    /// Load the checkpoint from `<repo>/.ralphy/init-state.json`, or a fresh
    /// default when the file does not exist (a first run).
    pub fn load(ws: &Workspace) -> Result<Self> {
        let path = ws.init_state_path();
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    /// Persist the checkpoint under `.ralphy/`, mirroring [`persist_report`].
    pub fn save(&self, ws: &Workspace) -> Result<()> {
        std::fs::create_dir_all(ws.ralphy_dir()).context("creating .ralphy dir")?;
        let json = serde_json::to_string_pretty(self).context("serializing init state")?;
        std::fs::write(ws.init_state_path(), json).context("writing .ralphy/init-state.json")?;
        Ok(())
    }

    /// Whether `stage` has already completed.
    pub fn is_done(&self, s: Stage) -> bool {
        self.completed.contains(&s)
    }

    /// Record `stage` as completed (idempotent — never duplicated).
    pub fn mark(&mut self, s: Stage) {
        if !self.completed.contains(&s) {
            self.completed.push(s);
        }
    }
}

/// One seeded console question: a short label, a one-line explanation of what
/// the field means and how to answer it, and the diagnosis-derived default the
/// dev confirms (empty input) or overrides. `clearable` is true for optional
/// fields that `none` can blank — it tailors the per-question keep/clear hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub label: String,
    pub help: String,
    pub default: String,
    pub clearable: bool,
}

/// Persist a validated diagnosis report under the workspace's `.ralphy/` so the
/// later init stages (and a re-run) can read it back (ADR-0012 stage 2).
pub(crate) fn persist_report(ws: &Workspace, report: &DiagnosisReport) -> Result<()> {
    std::fs::create_dir_all(ws.ralphy_dir()).context("creating .ralphy dir")?;
    let json = serde_json::to_string_pretty(report).context("serializing diagnosis report")?;
    std::fs::write(ws.diagnosis_path(), json).context("writing .ralphy/diagnosis.json")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> DiagnosisReport {
        DiagnosisReport {
            repo_kind: RepoKind::Existing,
            language_build: Some("Rust / cargo".into()),
            backlog_location: Some("docs/backlog.md".into()),
            milestone_docs: vec!["docs/roadmap.md".into(), "docs/prd/0001.md".into()],
            skills_dir: Some(".claude".into()),
            has_context_or_adrs: true,
            remote_host: Some("github.com".into()),
        }
    }

    fn config_of(report: &DiagnosisReport) -> InitConfig {
        InitConfig {
            repo_kind: report.repo_kind,
            language_build: report.language_build.clone(),
            backlog_location: report.backlog_location.clone(),
            milestone_docs: report.milestone_docs.clone(),
            skills_dir: report.skills_dir.clone(),
            has_context_or_adrs: report.has_context_or_adrs,
            remote_host: report.remote_host.clone(),
            adopt_prd_roadmap: !report.milestone_docs.is_empty(),
        }
    }

    #[test]
    fn persist_report_round_trips_through_ralphy_dir() {
        // Mirror gitignore.rs/queue.rs: no tempfile dep, manual temp dir.
        let dir = std::env::temp_dir().join(format!("ralphy-init-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(&dir);
        let report = sample_report();

        persist_report(&ws, &report).unwrap();
        let raw = std::fs::read_to_string(ws.diagnosis_path()).unwrap();
        let back: DiagnosisReport = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, report);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_state_round_trips_through_ralphy_dir() {
        // Mirror persist_report_round_trips: no tempfile dep, manual temp dir.
        let dir = std::env::temp_dir().join(format!("ralphy-init-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(&dir);

        let state = InitState {
            completed: vec![Stage::Diagnose, Stage::Git],
            config: Some(config_of(&sample_report())),
            milestone_created: Some("M1".into()),
            created_issues: vec![101, 102],
        };

        state.save(&ws).unwrap();
        let back = InitState::load(&ws).unwrap();
        assert_eq!(back, state);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_state_load_defaults_when_absent() {
        let dir =
            std::env::temp_dir().join(format!("ralphy-init-state-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ws = Workspace::new(&dir);
        // No file on disk → a fresh default, not an error.
        assert_eq!(InitState::load(&ws).unwrap(), InitState::default());
    }

    #[test]
    fn init_state_mark_is_idempotent() {
        let mut state = InitState::default();
        state.mark(Stage::Git);
        state.mark(Stage::Git);
        assert_eq!(state.completed, vec![Stage::Git]);
        assert!(state.is_done(Stage::Git));
        assert!(!state.is_done(Stage::Labels));
    }

    #[test]
    fn init_state_path_is_under_gitignored_ralphy_dir() {
        let d = std::env::temp_dir().join("ralphy-init-state-path-check");
        assert!(Workspace::new(&d)
            .init_state_path()
            .starts_with(Workspace::new(&d).ralphy_dir()));
    }
}
