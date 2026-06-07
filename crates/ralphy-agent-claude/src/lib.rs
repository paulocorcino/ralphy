//! The Claude Code adapter: drives `claude -p` (headless planning) behind the
//! core [`Agent`] contract. Everything Claude-specific — the binary, the model
//! and effort flags, the settings file — is confined here. The walking-skeleton
//! slice implements planning only; `execute` lands in a later slice.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use ralphy_core::{plan, Agent, Issue, Outcome, Plan, Workspace};
use tracing::info;

/// The planning prompt, embedded so the binary is self-contained as a global
/// tool. Single source of truth lives at the repo root.
const PROMPT_PLAN: &str = include_str!("../../../prompt.plan.md");

/// Minimal settings that keep a headless `claude -p` from hanging on a prompt.
/// The guard/Stop hooks are an execution concern and land in a later slice.
const SETTINGS_JSON: &str = r#"{"skipDangerousModePermissionPrompt":true,"skipAutoPermissionPrompt":true,"autoCompactEnabled":false}"#;

/// Drives the `claude` CLI. `plan_model`/`plan_effort` are the deterministic
/// planning knobs (the operator's choice); `run_dir` is where the settings file
/// and the captured plan log are written.
pub struct ClaudeAgent {
    plan_model: Option<String>,
    plan_effort: Option<String>,
    run_dir: PathBuf,
}

impl ClaudeAgent {
    pub fn new(plan_model: Option<String>, plan_effort: Option<String>, run_dir: PathBuf) -> Self {
        Self {
            plan_model,
            plan_effort,
            run_dir,
        }
    }
}

impl Agent for ClaudeAgent {
    fn plan(&self, _issue: &Issue, ws: &Workspace) -> Result<Plan> {
        fs::create_dir_all(ws.ralphy_dir()).ok();
        fs::create_dir_all(&self.run_dir).ok();

        let plan_path = ws.plan_path();
        // Plan fresh every run; never reuse a stale artifact.
        let _ = fs::remove_file(&plan_path);

        let settings_path = self.run_dir.join("ralphy.settings.json");
        fs::write(&settings_path, SETTINGS_JSON).context("writing claude settings")?;

        // `--model` first (as the ps1 oracle does), then the headless flags.
        let mut args: Vec<String> = Vec::new();
        if let Some(m) = &self.plan_model {
            args.push("--model".into());
            args.push(m.clone());
        }
        args.push("-p".into());
        args.push("--dangerously-skip-permissions".into());
        args.push("--settings".into());
        args.push(settings_path.to_string_lossy().into_owned());
        if let Some(e) = &self.plan_effort {
            args.push("--effort".into());
            args.push(e.clone());
        }

        info!(model = ?self.plan_model, effort = ?self.plan_effort, "planning with claude -p");
        let mut child = Command::new("claude")
            .args(&args)
            .current_dir(ws.repo_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn the `claude` CLI (is it installed and on PATH?)")?;

        // Pipe the prompt on stdin; dropping the handle closes it so claude sees EOF.
        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(PROMPT_PLAN.as_bytes())
            .context("piping the plan prompt to claude")?;

        let out = child.wait_with_output().context("waiting for claude")?;
        let mut log = String::from_utf8_lossy(&out.stdout).into_owned();
        log.push_str(&String::from_utf8_lossy(&out.stderr));
        let _ = fs::write(self.run_dir.join("plan.log"), &log);

        if !plan_path.exists() {
            bail!(
                "claude produced no plan at {} (see {})",
                plan_path.display(),
                self.run_dir.join("plan.log").display()
            );
        }
        let md = fs::read_to_string(&plan_path).context("reading the written plan.md")?;
        Ok(Plan {
            open_steps: plan::count_open_steps(&md),
            recommended_model: plan::recommended_model(&md),
            path: plan_path,
        })
    }

    fn execute(&self, _plan: &Plan, _ws: &Workspace) -> Result<Outcome> {
        bail!("execute is not implemented in the walking-skeleton slice — run with --dry-run");
    }
}
