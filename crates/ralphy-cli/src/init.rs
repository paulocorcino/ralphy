//! `ralphy init`: deterministic environment gate (ADR-0012 stage 1), then a
//! read-only repo diagnosis from a neutral cwd (stage 2) and a diagnosis-seeded
//! console Q&A captured into a typed config (stage 3), the git-safety snapshot +
//! `ralphy/init` branch (stage 4), the deterministic scaffold from the embedded
//! setup-pocock templates (stage 5), the optional sparse-checkout download of
//! engineering skills pinned to `RALPHY_VERSION` (stage 6), the idempotent
//! GitHub label vocabulary creation (stage 7), the conditional
//! backlog/milestone → issues judgment with a local preview the dev confirms
//! before any publish (stage 8), the `init-state.json` checkpoint (stage 9),
//! and the static verification + final report with an optional dry-run smoke
//! test (stage 10).

mod gate;
mod issues;
mod render;
mod resolve;
mod run;
mod scaffold;
mod skills;
mod verify;
mod wizard;

#[allow(unused_imports)]
pub use gate::{evaluate_gate, format_report, Agent, EnvFindings, HardFail};
#[allow(unused_imports)]
pub use run::{
    create_repo_decision, labels_decision, private_visibility_decision, repo_name_from_path, run,
    InitArgs,
};
#[allow(unused_imports)]
pub use skills::{
    download_decision, install_skills_from, install_skills_step, skill_names, skills_target,
    sparse_fetch_commands, Outcome,
};
#[allow(unused_imports)]
pub use verify::{
    format_final_report, required_artifacts_missing, smoke_test_decision, suggested_issue,
    VerifyReport,
};
#[allow(unused_imports)]
pub use wizard::{InitConfig, InitState, Question, Stage};

pub(crate) use gate::agent_logged_in;
pub(crate) use issues::{resolve_human_label, resolve_triage_label};
