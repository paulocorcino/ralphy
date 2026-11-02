//! The attention list: the requests this daemon's own sources open
//! (docs/adr/0085 D10, D11, D13). Each daemon works out its own list; the
//! daemon the page talks to joins its peers' lists (`routes/api_attention.rs`).
//!
//! A request shows only what is in the agent state: the vendor, the console,
//! the kind, its age (`since`) and the one-line detail (D13). A run's request
//! comes from its snapshot, which [`tick`] reads from each registered repo; a
//! finished run's requests come from the final snapshot in its run folder,
//! and last until the operator dismisses them ([`dismissals`], D12), a newer
//! run works the same issue, or the run folder is removed.

use ralphy_run_snapshot::{IssueBlock, RunSnapshot};

use crate::agent_state::Unreported;
use crate::session::SessionInfo;

pub(crate) mod dismissals;
pub(crate) mod done;
mod finals;
pub(crate) mod telegram;
pub(crate) mod tick;

/// The kind of a request. Matched exhaustively, so a new kind has to say
/// where it comes from and whether it may be dismissed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A console whose agent state is `waiting`.
    ConsoleWaiting,
    /// A console whose hooks sent nothing: Ralphy cannot see its agent.
    ConsoleSilent,
    /// A console that finished the turn the operator marked "tell me when it
    /// is done" for. It lasts until the operator opens the console or
    /// dismisses it.
    ConsoleDone,
    /// A run whose agent (`phase.agent` of its snapshot) is `waiting`.
    RunWaiting,
    /// An issue a finished run ended `blocked`, `hitl` or `needs_split`, or
    /// whose agent the idle watchdog stopped.
    RunFinished,
}

impl Kind {
    /// Whether the operator may dismiss a request of this kind. A live
    /// source cannot be dismissed: an agent that still waits stays listed
    /// (docs/adr/0085 D12).
    pub fn dismissable(self) -> bool {
        match self {
            Kind::ConsoleWaiting | Kind::ConsoleSilent | Kind::RunWaiting => false,
            Kind::ConsoleDone | Kind::RunFinished => true,
        }
    }
}

/// One request on the wire of `GET /api/attention`. `id` is
/// `console:<session id>` for a console, `done:<session id>:<n>` for the
/// n-th finished turn a console was marked for, `run:<runid>` for a run and
/// `issue:<run stamp>:<issue>` for a finished run's issue; a peer's request reaches the page
/// with `id`, `console` and `repo` prefixed `<daemon_id>/`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttentionRequest {
    pub id: String,
    pub kind: Kind,
    pub vendor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console: Option<String>,
    /// The console's name, when it has one; a main daemon's message for a
    /// peer's console names it from here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// When the request opened: the agent state's own `since`.
    pub since: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub dismissable: bool,
}

/// The body of `GET /api/attention`. `present` is only in the `?local=1`
/// reply: whether the operator is at one of the pages of the daemon that
/// answers (docs/adr/0085 D14). A main daemon reads it from each peer, and
/// an older peer, which has no such field, is "not present".
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct AttentionList {
    pub(crate) requests: Vec<AttentionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) present: Option<bool>,
}

/// Who reads a peer's request, which decides its `repo`: the page finds the
/// console under the peer's repo ref, so it reads `repo` prefixed; a person
/// reads the repo as the peer's slug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerReader {
    Page,
    Person,
}

/// A peer's request as `reader` reads it. `id` and `console` carry the peer's
/// daemon id (`<daemon_id>/<x>`), so two daemons' `console:1` stay two
/// requests and a link names the peer's console.
pub(crate) fn from_peer(
    daemon_id: &str,
    mut request: AttentionRequest,
    reader: PeerReader,
) -> AttentionRequest {
    let prefix = |value: &str| format!("{daemon_id}/{value}");
    request.id = format!("{daemon_id}/{}", request.id);
    request.console = request.console.as_deref().map(prefix);
    if reader == PeerReader::Page {
        request.repo = request.repo.as_deref().map(prefix);
    }
    request
}

/// The live run snapshots of one registered repo, under its registry key.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSource {
    pub repo: String,
    pub snapshot: RunSnapshot,
}

/// The final snapshot of a finished run of one registered repo, and the name
/// of its run folder.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedRun {
    pub repo: String,
    /// The run folder's name: `<repo>/.ralphy/runs/<stamp>/`.
    pub stamp: String,
    /// When the run ended (RFC 3339): when it wrote its final snapshot.
    pub ended: String,
    pub snapshot: RunSnapshot,
}

/// What the attention tick read of the runs of every registered repo, and
/// the done marks it fired.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunsRead {
    pub live: Vec<RunSource>,
    pub finished: Vec<FinishedRun>,
    pub done: Vec<done::DoneFired>,
}

/// The detail of a request for an issue whose agent the idle watchdog stopped.
pub const IDLE_REAPED_DETAIL: &str = "the idle watchdog stopped the agent";

/// The requests the consoles open, in the order of `sessions`, then the
/// finished turns of marked consoles, then the ones the live runs open, then
/// the ones the finished runs open, in the order of `runs`. `sessions` is the
/// list as `/api/sessions` shows it, already rendered at the time of the
/// read. A request whose id `is_dismissed` is not listed.
pub fn list(
    sessions: &[SessionInfo],
    runs: &RunsRead,
    is_dismissed: &dyn Fn(&str) -> bool,
) -> Vec<AttentionRequest> {
    sessions
        .iter()
        .filter_map(console_request)
        .chain(done::requests(sessions, &runs.done))
        .chain(runs.live.iter().filter_map(run_request))
        .chain(finished_requests(runs))
        .filter(|request| !is_dismissed(&request.id))
        .collect()
}

/// One request per console whose shown agent state is `waiting`, and one per
/// silent console, with the silence reason as its detail. A console with no
/// state source by design opens no request: it would stay open for the
/// console's whole life.
fn console_request(info: &SessionInfo) -> Option<AttentionRequest> {
    let state = info.agent_state.as_ref()?;
    let (kind, detail) = match state.source {
        Some(Unreported::Silent) => (Kind::ConsoleSilent, state.reason.clone()),
        Some(Unreported::NoSource) => return None,
        None if state.state.as_deref() == Some("waiting") => {
            (Kind::ConsoleWaiting, state.detail.clone())
        }
        None => return None,
    };
    Some(AttentionRequest {
        id: format!("console:{}", info.id),
        kind,
        vendor: info.agent.clone(),
        console: Some(info.id.to_string()),
        name: info.name.clone(),
        repo: Some(info.repo.clone()),
        run: None,
        issue: None,
        since: state.since.clone(),
        detail,
        dismissable: kind.dismissable(),
    })
}

/// One request per run whose agent is `waiting`: the vendor of the phase the
/// run is in, and the issue it works on.
fn run_request(source: &RunSource) -> Option<AttentionRequest> {
    let snap = &source.snapshot;
    let agent = snap.phase.agent.as_ref()?;
    if agent.state != "waiting" {
        return None;
    }
    let kind = Kind::RunWaiting;
    let vendor = if snap.phase.state == "planning" {
        &snap.plan_agent
    } else {
        &snap.exec_agent
    };
    Some(AttentionRequest {
        id: format!("run:{}", snap.runid),
        kind,
        vendor: vendor.clone(),
        console: None,
        name: None,
        repo: Some(source.repo.clone()),
        run: Some(snap.runid.clone()),
        issue: snap.phase.active.map(|n| n.to_string()),
        since: agent.since.clone(),
        detail: agent.detail.clone(),
        dismissable: kind.dismissable(),
    })
}

/// One request per issue of a finished run that needs the operator, unless a
/// newer run works the same issue.
fn finished_requests(runs: &RunsRead) -> impl Iterator<Item = AttentionRequest> + '_ {
    runs.finished.iter().flat_map(move |run| {
        run.snapshot
            .issues
            .iter()
            .filter(move |issue| !worked_again(runs, run, issue.number))
            .filter_map(move |issue| finished_request(run, issue))
    })
}

/// What a finished issue asks of the operator, if anything: an idle reap
/// first, else the status the run ended it with. `done`, `skipped`,
/// `non_green` and the other statuses ask nothing.
fn finished_detail(issue: &IssueBlock) -> Option<String> {
    if issue.idle_reaped {
        return Some(IDLE_REAPED_DETAIL.to_string());
    }
    matches!(issue.status.as_str(), "blocked" | "hitl" | "needs_split")
        .then(|| issue.status.clone())
}

fn finished_request(run: &FinishedRun, issue: &IssueBlock) -> Option<AttentionRequest> {
    let detail = finished_detail(issue)?;
    let kind = Kind::RunFinished;
    Some(AttentionRequest {
        id: format!("issue:{}:{}", run.stamp, issue.number),
        kind,
        vendor: run.snapshot.exec_agent.clone(),
        console: None,
        name: None,
        repo: Some(run.repo.clone()),
        run: Some(run.snapshot.runid.clone()),
        issue: Some(issue.number.to_string()),
        since: run.ended.clone(),
        detail: Some(detail),
        dismissable: kind.dismissable(),
    })
}

/// Whether a run of the same repo that started after `run`, live or
/// finished, has `number` in its issues. The order is the snapshots' own
/// `started_at`; a run whose start cannot be read is never "after".
fn worked_again(runs: &RunsRead, run: &FinishedRun, number: u64) -> bool {
    let started = |snap: &RunSnapshot| chrono::DateTime::parse_from_rfc3339(&snap.started_at).ok();
    let Some(this) = started(&run.snapshot) else {
        return false;
    };
    let live = runs.live.iter().map(|r| (&r.repo, &r.snapshot));
    let finished = runs.finished.iter().map(|f| (&f.repo, &f.snapshot));
    live.chain(finished).any(|(repo, snap)| {
        *repo == run.repo
            && snap.issues.iter().any(|i| i.number == number)
            && started(snap).is_some_and(|at| at > this)
    })
}

#[cfg(test)]
mod tests;
