use std::path::Path;
use std::process::Stdio;

use clap::ValueEnum;
use ralphy_adapter_support::{find_program, locate_program, resolve_program};
use ralphy_core::git;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Copilot,
    Kimi,
    Opencode,
}

impl Agent {
    /// ORDER IS LOAD-BEARING: `init`/`triage` auto-selection takes the FIRST
    /// logged-in agent in this array (`init::run::select_agent`,
    /// `triage::select_triage_agent`). `Copilot` is last on purpose — its one-shot
    /// verbs bail until the `tasks.rs` slice lands (ADR-0040 Tier 1), so placing it
    /// ahead of a fully-wired vendor would turn a working `ralphy init` into a hard
    /// bail on any machine where both are logged in. Move it up when its one-shots
    /// exist, not before.
    pub const ALL: [Agent; 5] = [
        Agent::Claude,
        Agent::Codex,
        Agent::Kimi,
        Agent::Opencode,
        Agent::Copilot,
    ];

    pub fn cli_name(&self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Copilot => "copilot",
            Agent::Kimi => "kimi",
            Agent::Opencode => "opencode",
        }
    }

    /// Whether this agent's adapter can consume image input. The VALUE lives as
    /// `ACCEPTS_IMAGES` in each adapter crate (ADR-0025 §4); this match only
    /// routes enum → crate, never hardcoding a capability here.
    pub fn accepts_images(&self) -> bool {
        match self {
            Agent::Claude => ralphy_agent_claude::ACCEPTS_IMAGES,
            Agent::Codex => ralphy_agent_codex::ACCEPTS_IMAGES,
            Agent::Copilot => ralphy_agent_copilot::ACCEPTS_IMAGES,
            Agent::Kimi => ralphy_agent_kimi::ACCEPTS_IMAGES,
            Agent::Opencode => ralphy_agent_opencode::ACCEPTS_IMAGES,
        }
    }
}

pub struct EnvFindings {
    pub python: bool,
    pub gh_authenticated: bool,
    pub github_remote: bool,
    pub agents_present: Vec<Agent>,
    pub agents_logged_in: Vec<Agent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardFail {
    MissingPython,
    GhNotAuthenticated,
    NoGithubRemote,
    NoAgentCli,
    NoAgentLoggedIn,
}

/// Pure gate evaluation: returns all hard failures given the environment findings.
/// The agent-login rule fires only when ≥1 agent is present.
pub fn evaluate_gate(f: &EnvFindings) -> Vec<HardFail> {
    let mut fails = Vec::new();
    if !f.python {
        fails.push(HardFail::MissingPython);
    }

    if !f.gh_authenticated {
        fails.push(HardFail::GhNotAuthenticated);
    }

    if !f.github_remote {
        fails.push(HardFail::NoGithubRemote);
    }

    if f.agents_present.is_empty() {
        fails.push(HardFail::NoAgentCli);
    } else if f.agents_logged_in.is_empty() {
        fails.push(HardFail::NoAgentLoggedIn);
    }

    fails
}

/// Pure report formatter. Produces a human-readable string with one line per
/// prerequisite and a summary. The substrings `"<name>: logged in"` and
/// `"<name>: not logged in"` are guaranteed for present agents so tests can
/// assert them literally.
pub fn format_report(f: &EnvFindings, fails: &[HardFail]) -> String {
    let mut out = String::new();

    let py = if f.python { "ok" } else { "MISSING" };
    out.push_str(&format!("python:        {py}\n"));

    let gh = if f.gh_authenticated {
        "ok"
    } else {
        "NOT AUTHENTICATED"
    };
    out.push_str(&format!("gh auth:       {gh}\n"));

    let remote = if f.github_remote {
        "ok"
    } else {
        "NO GITHUB REMOTE"
    };
    out.push_str(&format!("github remote: {remote}\n"));

    out.push_str("agents:\n");
    for agent in &Agent::ALL {
        let name = agent.cli_name();
        let present = f.agents_present.contains(agent);
        if present {
            let logged_in = f.agents_logged_in.contains(agent);
            if logged_in {
                out.push_str(&format!("  {name}: logged in\n"));
            } else {
                out.push_str(&format!("  {name}: not logged in\n"));
            }
        } else {
            out.push_str(&format!("  {name}: absent\n"));
        }
    }

    let blocker_count = fails.len();
    if blocker_count == 0 {
        out.push_str("result: all checks passed\n");
    } else {
        out.push_str(&format!("result: {blocker_count} blocker(s)\n"));
    }

    out
}

pub(crate) fn python_present() -> bool {
    let path = std::env::var_os("PATH");
    let pathext = std::env::var_os("PATHEXT");
    find_program("python", path.clone(), pathext.clone()).is_some()
        || find_program("python3", path, pathext).is_some()
}

pub(crate) fn gh_authenticated() -> bool {
    std::process::Command::new("gh")
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub(crate) fn github_remote(repo: &Path) -> bool {
    git::origin_url(repo)
        .map(|url| url.contains("github.com"))
        .unwrap_or(false)
}

// The gate's presence/login probes resolve each CLI through the SAME locator the
// adapters spawn through (`locate_program`/`resolve_program`), so detection and
// execution agree — a `claude` under `~/.local/bin` but off `PATH` is reported
// present and is the binary actually run, rather than being falsely called absent.
pub(crate) fn agent_present(a: &Agent) -> bool {
    locate_program(a.cli_name()).is_some()
}

/// The Copilot login verdict, split out from the spawning probe so the mapping
/// itself is testable: a catalog came back ⇒ the operator is logged in AND the
/// account may pin a model. An `Err` carries `COPILOT_CATALOG_ERROR_MSG` (or the
/// billed-probe refusal); the gate reports only the boolean, and the message is
/// surfaced by the report the operator reads.
fn copilot_logged_in(probe: anyhow::Result<ralphy_agent_copilot::CopilotCatalog>) -> bool {
    probe.is_ok()
}

pub(crate) fn agent_logged_in(a: &Agent) -> bool {
    let hello = "hello";
    let bin = resolve_program(a.cli_name());
    let mut cmd = std::process::Command::new(&bin);
    match a {
        Agent::Claude => {
            cmd.args(["-p", hello]);
        }

        Agent::Codex => {
            cmd.args(["exec", hello]);
            cmd.env_remove("OPENAI_API_KEY");
        }

        // The only arm that returns instead of falling through to the shared
        // `status().success()` tail: the catalog probe is judged by the CAPI log
        // line it leaves, never by the exit status, which has been observed as both
        // 0 and 1 for the very same intended model-selection failure — an exit-code
        // gate would report every Copilot operator logged in on one of those hosts.
        // The probe also costs no model call, unlike the `-p hello` it replaces, and
        // still scrubs the three token vars (ADR-0041 D8: an ambient token would
        // authenticate the child and make a logged-out operator look logged in) —
        // that now happens inside `fetch_catalog`.
        Agent::Copilot => return copilot_logged_in(ralphy_agent_copilot::fetch_catalog()),

        Agent::Kimi => {
            // `hello` is passed as the VALUE of `-p`, never a positional word:
            // Typer parses a bare positional as a subcommand (`No such command`,
            // exit 2) → an always-false login probe. Logged-out → exit 1
            // (`LLM not set`), logged-in → exit 0.
            cmd.args([
                "--print",
                "--output-format",
                "stream-json",
                "-m",
                "kimi-code/kimi-for-coding",
                "-p",
                hello,
            ]);
            // Mirror the adapter's mandatory encoding contract (command.rs): strip
            // PYTHONIOENCODING (an inherited value flips Kimi into the Textual TUI,
            // falsely failing a logged-in operator's probe) and set PYTHONUTF8=1 (so
            // a non-cp1252 char in Kimi's reply can't crash the probe on Windows).
            cmd.env_remove("PYTHONIOENCODING");
            cmd.env("PYTHONUTF8", "1");
        }

        Agent::Opencode => {
            cmd.args(["run", hello]);
        }
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_green() -> EnvFindings {
        EnvFindings {
            python: true,
            gh_authenticated: true,
            github_remote: true,
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![Agent::Claude],
        }
    }

    // Capability values are authored in each adapter crate; the CLI only routes.
    #[test]
    fn accepts_images_reflects_crate_consts() {
        assert!(Agent::Claude.accepts_images());
        assert!(Agent::Codex.accepts_images());
        assert!(Agent::Copilot.accepts_images());
        assert!(!Agent::Kimi.accepts_images());
        assert!(!Agent::Opencode.accepts_images());
        // The hardcoded ALL array length must track the enum: a new variant that
        // never joins ALL is invisible to `ralphy init`'s agent report.
        assert_eq!(Agent::ALL.len(), 5);
    }

    /// `init`/`triage` auto-selection takes the FIRST logged-in agent in `ALL`, and
    /// `Agent::ALL` is the auto-selection ORDER for a no-flag `ralphy init`/
    /// `triage` on a multi-login machine. The one-shots exist now (#237), so the
    /// reason to pin Copilot last is no longer a missing `tasks.rs` — it is
    /// auto-selection STABILITY: promoting Copilot would silently change which
    /// vendor drives a no-flag run, a behavior change no issue has asked for.
    #[test]
    fn copilot_stays_last_in_all() {
        assert_eq!(Agent::ALL.last(), Some(&Agent::Copilot));
    }

    /// The Copilot login probe is the FREE catalog fetch (#231), not a paid
    /// `-p hello` model call, and it is judged by what the probe logged rather than
    /// by the exit status the shared tail reads. Source-text pin: `agent_logged_in`
    /// spawns real processes, so the routing is what a test can hold. The needles
    /// are assembled from fragments so this assertion cannot match itself.
    #[test]
    fn copilot_login_probe_is_the_free_catalog_fetch() {
        let src = include_str!("gate.rs");
        // Scope to the probe fn: `cli_name` carries a `Copilot` arm of its own.
        let probe = src
            .split_once("fn agent_logged_in")
            .expect("the probe fn")
            .1;
        let probe = probe
            .split_once("\n#[cfg(test)]")
            .map(|(p, _)| p)
            .unwrap_or(probe);
        let arm = probe
            .split_once(concat!("Agent::", "Copilot =>"))
            .expect("the Copilot arm")
            .1;
        // Slice to the NEXT arm, not to the next newline: rustfmt may reflow this
        // arm into a block at any time, and a pin that reds on reflow names no
        // real defect.
        let arm = arm
            .split_once("\n        Agent::")
            .map(|(a, _)| a)
            .unwrap_or(arm);
        assert!(
            arm.contains(concat!("fetch_", "catalog()")),
            "the Copilot arm must probe through the free catalog fetch: {arm}"
        );
        // `return`: this arm must not fall through to the shared exit-status tail,
        // which would judge the probe by an exit code observed as both 0 and 1.
        assert!(arm.contains("return"), "arm: {arm}");
        // No paid model call anywhere in the probe.
        assert!(
            !probe.contains(concat!("\"-p\", ", "hello, \"--allow-all-tools\"")),
            "the paid Copilot probe is gone"
        );
    }

    /// The Ok⇒logged-in mapping itself, asserted in BOTH directions — the source
    /// pin above can only see that the arm calls the probe, not what it does with
    /// the answer.
    #[test]
    fn copilot_logged_in_maps_a_catalog_to_true_and_an_error_to_false() {
        let catalog = ralphy_agent_copilot::CopilotCatalog {
            models: Vec::new(),
            default_model: None,
            probe_session_id: String::new(),
        };
        assert!(copilot_logged_in(Ok(catalog)));
        assert!(!copilot_logged_in(Err(anyhow::anyhow!(
            "{}",
            ralphy_agent_copilot::COPILOT_CATALOG_ERROR_MSG
        ))));
        assert!(!copilot_logged_in(Err(anyhow::anyhow!(
            "{}",
            ralphy_agent_copilot::COPILOT_PROBE_BILLED_MSG
        ))));
    }

    // (a) All-green: evaluate_gate returns empty vec when ≥1 agent is logged in.
    #[test]
    fn evaluate_gate_all_green_returns_empty() {
        assert!(evaluate_gate(&all_green()).is_empty());
    }

    // (b) Missing python.
    #[test]
    fn evaluate_gate_missing_python() {
        let f = EnvFindings {
            python: false,
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::MissingPython));
    }

    // (c) gh not authenticated.
    #[test]
    fn evaluate_gate_gh_not_authenticated() {
        let f = EnvFindings {
            gh_authenticated: false,
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::GhNotAuthenticated));
    }

    // (d) No github remote.
    #[test]
    fn evaluate_gate_no_github_remote() {
        let f = EnvFindings {
            github_remote: false,
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::NoGithubRemote));
    }

    // (e) No agent CLI present.
    #[test]
    fn evaluate_gate_no_agent_cli() {
        let f = EnvFindings {
            agents_present: vec![],
            agents_logged_in: vec![],
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::NoAgentCli));
    }

    // (f) Two agents present, none logged in → NoAgentLoggedIn.
    #[test]
    fn evaluate_gate_agents_present_none_logged_in() {
        let f = EnvFindings {
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![],
            ..all_green()
        };
        let fails = evaluate_gate(&f);
        assert!(fails.contains(&HardFail::NoAgentLoggedIn));
        assert!(!fails.contains(&HardFail::NoAgentCli));
    }

    // (g) Two present, one logged in → empty vec (≥1 passes rule).
    #[test]
    fn evaluate_gate_one_of_two_logged_in_passes() {
        let f = EnvFindings {
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![Agent::Codex],
            ..all_green()
        };
        assert!(evaluate_gate(&f).is_empty());
    }

    // (h) format_report literal substring assertions.
    #[test]
    fn format_report_logged_in_and_not_logged_in_substrings() {
        let f = EnvFindings {
            python: true,
            gh_authenticated: true,
            github_remote: true,
            agents_present: vec![Agent::Claude, Agent::Codex],
            agents_logged_in: vec![Agent::Claude],
        };
        let fails = evaluate_gate(&f);
        let report = format_report(&f, &fails);
        assert!(
            report.contains("claude: logged in"),
            "expected 'claude: logged in' in:\n{report}"
        );
        assert!(
            report.contains("codex: not logged in"),
            "expected 'codex: not logged in' in:\n{report}"
        );
    }
}
