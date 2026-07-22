//! One-shot headless `gemini` sessions for the `init`/`triage` flows (ADR-0012
//! stages 2 & 8, ADR-0017, ADR-0043) — repo diagnosis, backlog → issues drafting,
//! agent-triage drafting, and knowledge consolidation. None of these publish to
//! GitHub; the cli applies the drafted artifact after the operator confirms.
//!
//! **The isolation is not a run-loop property, it is an adapter one.** Every verb
//! here pays the SAME [`crate::prepare_root`] a run pays — the owned
//! configuration root (D4), the sovereign policy document (D5), the
//! administrator-tier bail (D5) — through the one command builder
//! [`crate::command::build_gemini_command`], never a one-shot-shaped copy of it.
//! A second builder is precisely the drift that lets a one-shot read the
//! operator's own `~/.gemini`.
//!
//! The root's BASE is per-workspace (`<repo>/.ralphy`), with a defined
//! no-workspace fallback (D4): `<home>/.ralphy`, the same base
//! `auth::probe_gemini_login` ensures, so a machine ends with one root rather than
//! two. `draft_issues` and `consolidate_knowledge` legitimately run where there is
//! no repository at all, and must reach their spawn rather than be refused.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use tracing::info;

use ralphy_core::{
    build_diagnose_prompt, build_init_issues_prompt, build_triage_prompt, DiagnosisReport,
    DraftRequest, IssuesDraft, TriageDraft, TriageRequest, Usage, Workspace, PROMPT_CONSOLIDATE,
};

use crate::auth::{is_gemini_auth_error, GEMINI_AUTH_ERROR_MSG};
use crate::command::{self, build_gemini_command, check_stdin_ceiling, mint_session_id};
use crate::{outcome, revocation, PreparedRoot};

/// The spawn-failure sentence every verb shares, matching the run path's.
const SPAWN_ERR: &str = "failed to spawn the `gemini` CLI (is it installed?)";

/// Where this one-shot's owned root lives (D4).
///
/// `<repo>/.ralphy` when `repo` is a repository — the same base a run uses, so a
/// diagnosis and the run that follows it share one identity. Otherwise
/// `<home>/.ralphy`, which is exactly what `auth::probe_gemini_login` ensures.
/// A home that cannot be named falls back to the system temp dir with a warning
/// rather than a bail: a one-shot the operator asked for still runs, and the cost
/// is identity persistence, which is logged.
pub(crate) fn one_shot_base(repo: &Path) -> PathBuf {
    if ralphy_core::git::is_repo(repo) {
        return repo.join(".ralphy");
    }
    match ralphy_proc_util::home_dir() {
        Some(home) => home.join(".ralphy"),
        None => {
            tracing::warn!(
                "gemini: no home directory — the one-shot's configuration root falls back to \
                 the system temp dir, so its installation identity will not persist"
            );
            std::env::temp_dir().join("ralphy")
        }
    }
}

/// Build the one-shot's child command, having first prepared the owned root and
/// written the policy document under `base` (D4/D5). The single call site of
/// [`build_gemini_command`] on this path — the argv IS the isolation here, so no
/// verb gets to choose its own root argument.
///
/// The skill-discovery receipt is deliberately NOT paid here: it is an advisory
/// extra child spawn per verb, and a one-shot acts on nothing it reports.
pub(crate) fn one_shot_command(
    base: &Path,
    work_dir: &Path,
    model: Option<&str>,
) -> Result<Command> {
    let PreparedRoot {
        root,
        policy_path,
        auth_type,
        ..
    } = crate::prepare_root(base)?;
    Ok(build_gemini_command(
        &mint_session_id(),
        model,
        work_dir,
        &root.home,
        &policy_path,
        auth_type.as_deref(),
    ))
}

/// Whether a one-shot's child SUCCEEDED: a natural exit `0` that was not killed.
///
/// This is the gate [`one_shot_stop`] must not be consulted past. Two of the
/// ladder's rungs key on free text in the combined log, and on this vendor that
/// text is routine rather than diagnostic: a managed host prints "disabled by
/// administrator" in EVERY log, and `draft_issues`/`triage_issues` pipe the
/// model's own prose through stdout, so a backlog that merely MENTIONS a rate
/// limit would otherwise be reported as one. Both run-path counterparts gate the
/// same way — `plan()`'s ladder is `run_plan_session`'s `on_missing`, consulted
/// only when no plan was written, and `classify_gemini_outcome`'s is
/// `(!succeeded).then(…)` ("a revocation must never flip a run that succeeded").
fn one_shot_succeeded(out: &ralphy_adapter_support::HeadlessOutput) -> bool {
    out.exit.map(|s| s.success()).unwrap_or(false) && !out.timed_out
}

/// The one-shot's stop ladder for a session that did NOT succeed, in the SAME
/// order `plan()` uses: a hard-stop revocation, then the wall timeout, then a
/// provider limit, then an informational revocation notice, then the exit-code
/// taxonomy. `None` means the failure carries no sentence better than the
/// caller's own.
///
/// The timeout sits second rather than last (where a naive reading of `plan()`'s
/// ladder would put it): on a timeout `exit` is `None`, so the exit-code rung is
/// inert, and a routine administrator notice in the log would otherwise be
/// reported instead of the reap. Both shared session runners bail on a timeout
/// immediately after auth for the same reason.
///
/// It is exit-code FIRST in the sense that matters (D3): the vendor's own exit
/// taxonomy is consulted here rather than discarded, which is why the one-shots
/// cannot go through `run_text_session` — that runner drops the child's status.
pub(crate) fn one_shot_stop(log: &str, exit_code: Option<i32>, timed_out: bool) -> Option<String> {
    let rev = revocation::detect_revocation(log);
    rev.filter(|r| r.is_hard_stop())
        .map(|r| r.message(exit_code, log))
        .or_else(|| timed_out.then(|| "gemini session hit the wall timeout".to_string()))
        .or_else(|| outcome::gemini_limit_note(log))
        .or_else(|| rev.map(|r| r.message(exit_code, log)))
        .or_else(|| {
            outcome::classify_exit(exit_code)
                .actionable_stop()
                .map(str::to_string)
        })
}

/// Drive one headless `gemini` child to completion, persist its combined log, and
/// — only when the child did NOT succeed — turn the failure into the sentence
/// [`one_shot_stop`] chose.
///
/// **Cross-path invariant:** the log is written on EVERY return path after the
/// child ran — including each bail — so a failing one-shot never leaves the
/// operator without the log its own error message points at.
///
/// The prompt's stdin ceiling (D2) is checked by each VERB before it prepares a
/// root, not here: by the time this is reached the root, the skills and the
/// policy document have already been written, and a charter Ralphy refuses to
/// send must cost none of that.
fn run_one_shot(cmd: Command, prompt: &str, timeout: Duration, log_path: &Path) -> Result<()> {
    let out = ralphy_adapter_support::run_headless(cmd, prompt, timeout).context(SPAWN_ERR)?;
    let succeeded = one_shot_succeeded(&out);
    let (code, timed_out) = (out.exit.and_then(|s| s.code()), out.timed_out);
    let mut log = out.stdout;
    log.push_str(&out.stderr);
    let _ = fs::write(log_path, &log);

    if is_gemini_auth_error(&log) {
        bail!("{GEMINI_AUTH_ERROR_MSG} (see {})", log_path.display());
    }
    if succeeded {
        return Ok(());
    }
    match one_shot_stop(&log, code, timed_out) {
        Some(msg) => bail!("{msg} (see {})", log_path.display()),
        // A non-zero exit the taxonomy does not name is still a failure: saying so
        // beats returning `Ok` and letting the caller report a missing artifact.
        None => bail!(
            "the gemini session failed (exit {}) — see {}",
            code.map_or_else(|| "killed".to_string(), |c| c.to_string()),
            log_path.display()
        ),
    }
}

/// Ensure the artifact's and the log's parent dirs exist, then drop any stale
/// artifact — a prior run's file must never masquerade as this session's output.
fn clear_stale_artifact(out_path: &Path, log_path: &Path) {
    for parent in [out_path.parent(), log_path.parent()].into_iter().flatten() {
        fs::create_dir_all(parent).ok();
    }
    let _ = fs::remove_file(out_path);
}

/// Read and validate the JSON artifact a one-shot was asked to write.
fn read_artifact<T: DeserializeOwned>(
    out_path: &Path,
    log_path: &Path,
    missing_msg: &str,
    label: &str,
) -> Result<T> {
    let raw = fs::read_to_string(out_path).with_context(|| {
        format!(
            "{} at {} (see {})",
            missing_msg,
            out_path.display(),
            log_path.display()
        )
    })?;
    // A vendor CLI on Windows may write the artifact UTF-8-BOM-prefixed; left in
    // place the BOM reads as a schema mismatch at "line 1 column 1".
    serde_json::from_str(ralphy_adapter_support::strip_bom(&raw)).with_context(|| {
        format!(
            "{} at {} did not match the schema",
            label,
            out_path.display()
        )
    })
}

/// Run a one-shot headless `gemini` repo-diagnosis session (ADR-0012 stage 2)
/// from `neutral_cwd` — a directory OUTSIDE the target repo, so the CLI cannot
/// auto-load the target's own agent instructions. The target `repo` is passed as
/// data in the prompt.
///
/// The owned root's base is still the TARGET's (`one_shot_base(repo)`), not the
/// throwaway cwd's: the identity belongs to the repository being diagnosed, and a
/// root under a temp dir the caller deletes would be a new installation every run.
/// `effort` is unused — this vendor's headless surface has no reasoning-effort
/// axis.
pub fn diagnose_repo(
    repo: &Path,
    neutral_cwd: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<DiagnosisReport> {
    let _ = effort;
    let out_path = neutral_cwd.join("diagnosis.json");
    let log_path = neutral_cwd.join("diagnose.log");
    let prompt = build_diagnose_prompt(repo, &out_path);
    // D2 before any side effect: a charter ralphy refuses to send must cost
    // neither a materialized root nor a written policy document.
    check_stdin_ceiling(&prompt)?;
    clear_stale_artifact(&out_path, &log_path);

    info!(?model, "diagnosing repo with gemini");
    let cmd = one_shot_command(&one_shot_base(repo), neutral_cwd, model)?;
    run_one_shot(cmd, &prompt, timeout, &log_path)?;
    read_artifact(
        &out_path,
        &log_path,
        "diagnosis session left no report",
        "diagnosis report",
    )
}

/// Run a one-shot headless `gemini` backlog/milestone → issues session (ADR-0012
/// stage 8). Unlike [`diagnose_repo`] this runs IN the repo cwd — it needs the
/// repo's domain glossary/ADRs and (on the milestone path) writes a PRD under
/// `docs/prd/`. Never publishes to GitHub: that is the cli's job after the
/// operator confirms.
pub fn draft_issues(
    repo: &Path,
    out_path: &Path,
    req: &DraftRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<IssuesDraft> {
    let _ = effort;
    let prompt =
        build_init_issues_prompt(repo, req.mode, req.source_docs, req.triage_label, out_path);
    let log_path = repo.join(".ralphy").join("init-issues.log");
    check_stdin_ceiling(&prompt)?;
    clear_stale_artifact(out_path, &log_path);

    info!(
        ?model,
        mode = req.mode.as_str(),
        "drafting issues with gemini"
    );
    let cmd = one_shot_command(&one_shot_base(repo), repo, model)?;
    run_one_shot(cmd, &prompt, timeout, &log_path)?;
    read_artifact(
        out_path,
        &log_path,
        "issues session left no draft",
        "issues draft",
    )
}

/// Assemble the triage charter: the core prompt, then `attachments_manifest`
/// (the vendor-neutral textual inventory, ADR-0025 §6), then this vendor's own
/// `@`-reference block per fetched image (ADR-0043 D14). The `@` syntax is
/// this vendor's alone, so it is appended here rather than folded into the
/// core-owned manifest.
pub(crate) fn triage_prompt(repo: &Path, req: &TriageRequest, out_path: &Path) -> String {
    format!(
        "{}{}{}",
        build_triage_prompt(repo, req.issue_numbers, req.queue_label, out_path),
        req.attachments_manifest,
        command::attachment_block(req.image_paths)
    )
}

/// Run a one-shot headless `gemini` agent-triage session (ADR-0017). Mirrors
/// [`draft_issues`] but drives the triage charter over each `triage-agent` issue's
/// body + full comment thread, writing a [`TriageDraft`] JSON to `out_path` for the
/// cli to apply after the operator confirms.
pub fn triage_issues(
    repo: &Path,
    out_path: &Path,
    req: &TriageRequest,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<TriageDraft> {
    let _ = effort;
    let prompt = triage_prompt(repo, req, out_path);
    let log_path = repo.join(".ralphy").join("triage.log");
    check_stdin_ceiling(&prompt)?;
    clear_stale_artifact(out_path, &log_path);

    info!(?model, "triaging issues with gemini");
    let mut cmd = one_shot_command(&one_shot_base(repo), repo, model)?;
    command::add_include_directories(&mut cmd, &command::attachment_dirs(req.image_paths));
    run_one_shot(cmd, &prompt, timeout, &log_path)?;
    read_artifact(
        out_path,
        &log_path,
        "triage session left no draft",
        "triage draft",
    )
}

/// Run a one-shot headless `gemini` knowledge-consolidation session in `ws`'s repo
/// cwd: pipe the shared consolidation charter on stdin and wait up to `timeout`.
/// The session's only deliverable is the rewritten `KNOWLEDGE.md`, which the caller
/// verifies; the consumed notes are archived by the caller, not here.
///
/// Returns `Usage::default()` for now (issue #269): the run-level fold and ledger
/// line are uniform across vendors, but this adapter's headless consolidation
/// stream is not yet parsed for tokens — only Cursor's is live-validated. Wiring
/// this vendor's own parser here is a best-effort follow-up (ADR-0008 D9).
pub fn consolidate_knowledge(
    ws: &Workspace,
    run_dir: &Path,
    model: Option<&str>,
    effort: Option<&str>,
    timeout: Duration,
) -> Result<Usage> {
    let _ = effort;
    check_stdin_ceiling(PROMPT_CONSOLIDATE)?;
    fs::create_dir_all(run_dir).ok();
    let log_path = run_dir.join("consolidate.log");

    info!(?model, "consolidating knowledge with gemini");
    let cmd = one_shot_command(&one_shot_base(ws.repo_root()), ws.repo_root(), model)?;
    run_one_shot(cmd, PROMPT_CONSOLIDATE, timeout, &log_path)?;
    Ok(Usage::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The argv/env the tests read back out of a built `Command`.
    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// D4's no-workspace case, which the plain "join `.ralphy` onto the repo"
    /// implementation gets silently wrong: `draft_issues` and
    /// `consolidate_knowledge` legitimately run outside any repository, and the
    /// root they land on must be the SAME `<home>/.ralphy` the login probe
    /// ensures — asserted as that literal value, not merely "some path".
    #[test]
    fn one_shot_base_falls_back_outside_a_repository() {
        // A REAL repository: `git::is_repo` shells out to `rev-parse`, which an
        // empty `.git` directory does not satisfy.
        let repo = tempfile::tempdir().expect("tempdir");
        ralphy_core::git::init(repo.path()).expect("git init");
        assert_eq!(
            one_shot_base(repo.path()),
            repo.path().join(".ralphy"),
            "in a workspace the one-shot shares the run's root"
        );

        // `git rev-parse` walks UP, so a `TMPDIR` that happens to sit inside a
        // checkout makes the no-workspace branch unreachable. Assert the
        // precondition rather than the fallback in that case: a red here on a
        // correct implementation would be worse than a stated skip.
        let plain = tempfile::tempdir().expect("tempdir");
        if ralphy_core::git::is_repo(plain.path()) {
            assert_eq!(one_shot_base(plain.path()), plain.path().join(".ralphy"));
            return;
        }
        assert_eq!(
            one_shot_base(plain.path()),
            ralphy_proc_util::home_dir()
                .expect("this host has a home directory")
                .join(".ralphy"),
            "with no workspace the one-shot shares the login probe's root"
        );
    }

    /// AC2, on the one-shot path: the child is pointed at the root Ralphy owns and
    /// carries Ralphy's own policy document — neither is bypassed here. No child is
    /// spawned: the discovery probe lives outside `prepare_root`, which is what
    /// keeps this test spawn-free.
    #[test]
    fn the_one_shot_command_carries_the_owned_root_and_the_policy() {
        let base = tempfile::tempdir().expect("tempdir");
        let work = tempfile::tempdir().expect("tempdir");
        let cmd = one_shot_command(base.path(), work.path(), None).expect("prepare + build");

        let owned = base.path().join("gemini-home");
        let home = cmd
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("GEMINI_CLI_HOME"))
            .and_then(|(_, v)| v)
            .expect("GEMINI_CLI_HOME must be set on the one-shot too");
        assert_eq!(
            Path::new(home),
            owned,
            "D4: the owned root, not the operator's"
        );

        let args = argv(&cmd);
        let i = args
            .iter()
            .position(|a| a == "--policy")
            .unwrap_or_else(|| panic!("the one-shot must carry a policy document: {args:?}"));
        let policy = owned.join(".gemini").join("ralphy-policy.toml");
        assert_eq!(Path::new(&args[i + 1]), policy, "argv: {args:?}");
        let body = fs::read_to_string(&policy).expect("the policy document must be written");
        assert!(body.contains(r#"toolName = "invoke_agent""#), "{body}");
        assert!(body.contains(r#"decision = "deny""#), "{body}");

        let j = args
            .iter()
            .position(|a| a == "--approval-mode")
            .unwrap_or_else(|| panic!("autonomy must be requested: {args:?}"));
        assert_eq!(args[j + 1], "yolo", "argv: {args:?}");

        // AC5, the half an argv CAN carry: the turn is an addressable session
        // under the owned `GEMINI_CLI_HOME` asserted above, which is where the
        // vendor puts its session record. That the record actually LANDS there is
        // a live observation, not an argv property — see ADR-0043's #259 section.
        let k = args
            .iter()
            .position(|a| a == "--session-id")
            .unwrap_or_else(|| panic!("the one-shot must be an addressable session: {args:?}"));
        let id = &args[k + 1];
        assert_eq!(id.len(), 36, "a v4 uuid: {id}");
        assert_eq!(id.matches('-').count(), 4, "a v4 uuid: {id}");
    }

    /// AC4's ordering, with the discriminating control: a managed host prints the
    /// tool-server notice in EVERY log, so a ladder that let any revocation
    /// pre-empt the limit would permanently misroute quota exhaustion — and a hard
    /// stop must still outrank the limit, because it will not heal on a retry.
    #[test]
    fn the_one_shot_ladder_orders_revocation_before_limit() {
        const BOTH: &str = "Gemini CLI is not running in a trusted directory.\n\
                            Error: quota exceeded for this project\n";
        let msg = one_shot_stop(BOTH, Some(55), false).expect("a stop is reported");
        assert!(
            msg.contains("refused the workspace as untrusted"),
            "the hard stop must win: {msg}"
        );
        assert!(
            !msg.contains("provider limit"),
            "the limit note must not be what is reported: {msg}"
        );

        // The limit alone still routes as the limit.
        let limit = one_shot_stop("Error: quota exceeded\n", Some(1), false).expect("a stop");
        assert!(limit.contains("provider limit"), "{limit}");

        // The exit taxonomy alone, verbatim — the reason this path cannot go
        // through a runner that discards the child's status.
        assert_eq!(
            one_shot_stop("", Some(54), false).as_deref(),
            outcome::classify_exit(Some(54)).actionable_stop(),
            "exit 54 must carry its own diagnosis"
        );
        assert!(one_shot_stop("", Some(54), false)
            .unwrap()
            .contains("exit 54"));

        // A clean exit with nothing to report is not an error.
        assert_eq!(one_shot_stop("", Some(0), false), None);
        // …and a wall timeout with no other signal still is.
        assert!(one_shot_stop("", None, true)
            .expect("a timeout is a stop")
            .contains("wall timeout"));
        // On a timeout the child was KILLED, so `exit` is `None` and the
        // exit-code rung is inert — a routine administrator notice in the log
        // must not be reported in the reap's place.
        let reaped = one_shot_stop("MCP server disabled by administrator\n", None, true)
            .expect("a timeout is a stop");
        assert!(reaped.contains("wall timeout"), "{reaped}");
    }

    /// The gate the ladder must not be consulted past (HIGH, self-review): two of
    /// its rungs key on FREE TEXT, and on this vendor that text is routine. A
    /// managed host prints the tool-server notice in every log, and a backlog or
    /// triage thread may merely MENTION a rate limit — neither may cost a session
    /// that exited 0 its artifact.
    ///
    /// Pinned on the source because the alternative is spawning a real child: the
    /// production path must consult `one_shot_stop` only AFTER the success gate,
    /// and must return early on success.
    #[test]
    fn a_successful_one_shot_is_never_failed_by_its_own_log() {
        // The rungs are live on these strings — which is exactly why the gate
        // must exist.
        assert!(one_shot_stop("MCP server disabled by administrator\n", Some(0), false).is_some());
        assert!(one_shot_stop("the backlog mentions a rate limit\n", Some(0), false).is_some());

        let src = include_str!("tasks.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let body = &src[src
            .find("fn run_one_shot(")
            .expect("run_one_shot must exist")..];
        let at = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("run_one_shot must contain {needle:?}"))
        };
        assert!(
            at("if succeeded {") < at(concat!("one_shot_", "stop(&log")),
            "the success gate must precede the ladder, or a green session's own \
             words can fail it"
        );
        assert!(
            at("return Ok(())") < at(concat!("one_shot_", "stop(&log")),
            "a successful session must return before the ladder is consulted"
        );
        // …and the log is persisted before ANY bail, so every error message's
        // `(see <log>)` points at a file that exists.
        assert!(
            at("fs::write(log_path") < at("bail!("),
            "the combined log must be written before the first bail"
        );
    }

    /// A stale artifact is the one failure mode with a GitHub-visible blast
    /// radius: a drafting session that writes nothing would otherwise hand the cli
    /// the PREVIOUS run's draft to publish.
    #[test]
    fn a_stale_artifact_never_survives_into_a_new_session() {
        let d = tempfile::tempdir().expect("tempdir");
        let out = d.path().join("nested").join("draft.json");
        let log = d.path().join("logs").join("init-issues.log");
        fs::create_dir_all(out.parent().unwrap()).unwrap();
        fs::write(&out, r#"{"issues":["a previous run's draft"]}"#).unwrap();

        clear_stale_artifact(&out, &log);
        assert!(!out.exists(), "the previous run's draft must be gone");
        assert!(
            log.parent().unwrap().is_dir(),
            "the log's parent is created"
        );
        assert!(out.parent().unwrap().is_dir(), "the artifact's parent too");
    }

    /// `diagnose_repo`'s child runs in the throwaway `neutral_cwd`, but its owned
    /// root belongs to the TARGET: a root under a directory the caller deletes
    /// would make every diagnosis a fresh installation. Pinned on the source — no
    /// test here spawns a child, so nothing else would catch the swap.
    #[test]
    fn each_verb_roots_itself_at_the_target_not_the_scratch_cwd() {
        let src = include_str!("tasks.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(
            src.contains(concat!(
                "one_shot_",
                "command(&one_shot_base(repo), neutral_cwd,"
            )),
            "diagnose_repo must root at the target repo while running in the neutral cwd"
        );
        assert_eq!(
            src.matches(concat!("one_shot_", "command(&one_shot_base("))
                .count(),
            4,
            "every verb derives its base through one_shot_base, none hand-rolls one"
        );
        // The artifact BOM guard is the shared one, which is why `strip_bom` was
        // promoted to `pub` rather than copied here.
        assert!(src.contains("ralphy_adapter_support::strip_bom("));
    }

    /// AC2: an image fetched during triage reaches the charter as an
    /// `@`-reference, appended after the manifest — never in its place.
    #[test]
    fn triage_prompt_carries_an_at_reference_per_fetched_image() {
        let repo = tempfile::tempdir().expect("tempdir");
        ralphy_core::git::init(repo.path()).expect("git init");
        let png = PathBuf::from("/tmp/attachments/1/red.png");
        let req = TriageRequest {
            issue_numbers: &[1],
            queue_label: "triage-agent",
            attachments_manifest: "\n\n## Attachments\n- #1: red.png\n",
            image_paths: std::slice::from_ref(&png),
        };
        let out_path = repo.path().join("triage-draft.json");

        let prompt = triage_prompt(repo.path(), &req, &out_path);
        assert!(
            prompt.contains(&command::at_reference(&png, cfg!(windows))),
            "prompt: {prompt}"
        );
        assert!(
            prompt.contains(req.attachments_manifest),
            "the manifest text must survive verbatim: {prompt}"
        );
    }

    /// AC2's argv half: the triage command widens its workspace by exactly
    /// each distinct fetched-attachment directory, and no other verb does.
    #[test]
    fn the_triage_command_includes_every_attachment_directory() {
        let base = tempfile::tempdir().expect("tempdir");
        let work = tempfile::tempdir().expect("tempdir");
        let images = [
            PathBuf::from("/tmp/attachments/1/a.png"),
            PathBuf::from("/tmp/attachments/1/b.png"),
            PathBuf::from("/tmp/attachments/2/c.png"),
        ];

        let mut cmd = one_shot_command(base.path(), work.path(), None).expect("prepare + build");
        command::add_include_directories(&mut cmd, &command::attachment_dirs(&images));

        let args = argv(&cmd);
        let positions: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--include-directories")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(positions.len(), 2, "argv: {args:?}");
        assert_eq!(
            args[positions[0] + 1],
            "/tmp/attachments/1",
            "argv: {args:?}"
        );
        assert_eq!(
            args[positions[1] + 1],
            "/tmp/attachments/2",
            "argv: {args:?}"
        );

        // Only `triage_issues` may widen the workspace this way.
        let src = include_str!("tasks.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert_eq!(
            src.matches(concat!("add_include_", "directories(")).count(),
            1,
            "the other three one-shot verbs must not widen their workspace"
        );
    }
}
