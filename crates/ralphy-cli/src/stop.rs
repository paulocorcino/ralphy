//! `ralphy stop` — ask a live run in this repo to stop (docs/adr/0054).
//!
//! This process does not stop anything. It writes a runid-scoped sentinel next
//! to the target run's snapshot document and exits; the RUN notices its own
//! sentinel on the 250 ms snapshot tick, sets [`ralphy_core::stop`], kills its
//! own vendor child, and unwinds through the ordinary teardown — branch handed
//! back, working tree untouched, the issue in flight left open.
//!
//! That indirection is the whole point. It is what lets the daemon offer a Stop
//! button while keeping ADR-0032 §5/§6's invariant literally true: the daemon
//! dispatches this as one more blessed `ralphy` child and never signals, kills,
//! or writes repo state itself.
//!
//! **This is the one Mutate that deliberately does NOT consult `.ralphy/run.lock`.**
//! Every other write verb refuses under [`crate::runlock::LockState::HeldAlive`]
//! (ADR-0036 §6) because a run owns the tree while it works. This one exists
//! precisely to act *while* a run holds the lock — guarding it would make it
//! refuse in the only situation it is for. `runlock.rs`'s own refusal message
//! already says "wait for it to finish or stop it"; this is that.
//!
//! `--format json` is for humans, CI and `jq`, never for the workbench: the
//! daemon's Mutate branch collapses a successful exit to `{"status":"ok"}` and
//! discards stdout (mirrors `sync.rs`).

use std::path::PathBuf;

use clap::Args;

/// The sentinel's body. Forensics only — the run's read is existence-only, so a
/// torn or truncated write can never make a stop fail to fire.
#[derive(serde::Serialize)]
struct StopRequest {
    requested_at: String,
    by_pid: u32,
}

#[derive(Args)]
pub(crate) struct StopArgs {
    /// Any path inside the target repo; resolved to its git toplevel.
    #[arg(long, default_value = ".")]
    pub(crate) repo: PathBuf,

    /// Which run to stop. Optional while the repo has exactly one live run;
    /// required once it has more than one.
    #[arg(long)]
    pub(crate) runid: Option<String>,

    /// Output format: `json` emits `{stop}`; omitted prints one human line.
    #[arg(long)]
    pub(crate) format: Option<String>,
}

/// `ralphy stop [--repo <path>] [--runid <id>] [--format json]`.
pub(crate) fn stop(args: StopArgs) -> anyhow::Result<()> {
    let repo_root = ralphy_core::git::resolve_toplevel(&args.repo)?;
    let listing = ralphy_run_snapshot::list_runs(&repo_root, ralphy_proc_util::pid_is_alive);
    let live: Vec<(String, u32)> = listing
        .live
        .iter()
        .map(|s| (s.runid.clone(), s.pid))
        .collect();

    let target = choose_target(&live, args.runid.as_deref())?;

    let Some((runid, pid)) = target else {
        // Nothing to stop is exit 0, not a failure: the operator asked for "this
        // repo is not running" and that is already true. Same reasoning as
        // `run --if-idle`'s clean deferral — a scheduler's history, and the
        // workbench's Stop button, must not fill with false failures for a race
        // they cannot avoid.
        if args.format.as_deref() == Some("json") {
            println!(
                "{}",
                serde_json::json!({ "stop": { "requested": false, "reason": "no live run" } })
            );
        } else {
            println!("no live run in this repo — nothing to stop.");
        }
        return Ok(());
    };

    let path = ralphy_run_snapshot::stop_path(&repo_root, &runid);
    // The directory exists whenever a run does (the snapshot document is in it),
    // but create it anyway: `list_runs` on an absent directory is an empty
    // listing, and a target resolved from a listing that raced a sweep must not
    // fail on a missing parent.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("could not create {}: {e}", parent.display()))?;
    }
    let body = serde_json::to_string(&StopRequest {
        requested_at: now_rfc3339(),
        by_pid: std::process::id(),
    })
    .map_err(|e| anyhow::anyhow!("could not encode the stop request: {e}"))?;
    std::fs::write(&path, body)
        .map_err(|e| anyhow::anyhow!("could not write {}: {e}", path.display()))?;

    if args.format.as_deref() == Some("json") {
        println!(
            "{}",
            serde_json::json!({ "stop": { "requested": true, "runid": runid, "pid": pid } })
        );
    } else {
        println!("stop requested for run {runid} (pid {pid}).");
    }
    Ok(())
}

/// Which run the request targets, pure so the refusals are testable without a
/// repo. `Ok(None)` is "nothing live"; `Err` is the operator pointing at
/// something that is not there, which is a refusal rather than a no-op.
///
/// Ambiguity REFUSES rather than guessing. Stopping is not reversible in the
/// sense that matters — the killed child's work is gone — so picking "probably
/// the newest one" on the operator's behalf is the wrong kind of helpful.
fn choose_target(
    live: &[(String, u32)],
    wanted: Option<&str>,
) -> anyhow::Result<Option<(String, u32)>> {
    match wanted {
        Some(id) => match live.iter().find(|(runid, _)| runid == id) {
            Some(hit) => Ok(Some(hit.clone())),
            None => anyhow::bail!("no live run {id} in this repo"),
        },
        None => match live {
            [] => Ok(None),
            [only] => Ok(Some(only.clone())),
            many => {
                let ids: Vec<&str> = many.iter().map(|(runid, _)| runid.as_str()).collect();
                anyhow::bail!(
                    "{} live runs in this repo — name one with --runid: {}",
                    many.len(),
                    ids.join(", ")
                )
            }
        },
    }
}

/// The request stamp. Local wall clock formatted as RFC 3339, matching
/// `runlock`'s `started_at` — this is a human-readable forensic field, never
/// something the run compares against.
fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(ids: &[&str]) -> Vec<(String, u32)> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| ((*id).to_string(), 1000 + i as u32))
            .collect()
    }

    #[test]
    fn no_live_run_is_not_an_error() {
        assert!(choose_target(&[], None)
            .expect("nothing live is a clean answer")
            .is_none());
    }

    #[test]
    fn a_single_live_run_needs_no_runid() {
        let got = choose_target(&runs(&["01A"]), None).expect("one run resolves");
        assert_eq!(got, Some(("01A".to_string(), 1000)));
    }

    #[test]
    fn several_live_runs_refuse_and_name_them() {
        let err = choose_target(&runs(&["01A", "01B"]), None)
            .expect_err("ambiguity must refuse, never guess");
        let msg = err.to_string();
        assert!(msg.contains("--runid"), "the refusal must say how: {msg}");
        assert!(
            msg.contains("01A") && msg.contains("01B"),
            "names them: {msg}"
        );
    }

    #[test]
    fn an_explicit_runid_picks_that_run_out_of_several() {
        let got = choose_target(&runs(&["01A", "01B"]), Some("01B")).expect("named run resolves");
        assert_eq!(got, Some(("01B".to_string(), 1001)));
    }

    /// The negative control for the arm above: naming a run that is not live is
    /// a refusal, NOT the "nothing to stop" clean exit. Collapsing the two would
    /// let a typo report success while the run kept going.
    #[test]
    fn an_unknown_runid_refuses_rather_than_reporting_nothing_to_stop() {
        let err = choose_target(&runs(&["01A"]), Some("01Z")).expect_err("a wrong target refuses");
        assert!(err.to_string().contains("01Z"), "{err}");
    }
}
