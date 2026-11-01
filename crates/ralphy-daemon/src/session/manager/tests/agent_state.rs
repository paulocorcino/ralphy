//! The agent state of a console as the manager shows it: the tick's
//! changes, the silence rule, the operator's input, and the order of a tick
//! and an input.

use std::time::Duration;

use super::*;
use crate::agent_state::{HookVendor, SILENT_AFTER, STALE_AFTER};

/// A Claude hook line of `event`.
fn claude_line(event: &str) -> String {
    format!("{{\"event\":\"{event}\",\"tool_name\":\"Bash\",\"tool_input\":null,\"ts\":\"t\"}}\n")
}

/// A permission request for `Bash`, at hook time `ts`.
fn permission(ts: &str) -> String {
    format!(
        "{{\"event\":\"PermissionRequest\",\"session_id\":\"s1\",\"tool_name\":\"Bash\",\"ts\":\"{ts}\"}}\n"
    )
}

/// Append `line` to the status file at `path`.
fn append(path: &std::path::Path, line: &str) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open the status file");
    file.write_all(line.as_bytes())
        .expect("append the status line");
}

/// A console with hooks, spawned on the platform shell, its session, and
/// its status file.
fn hooked_console(
    manager: &Arc<SessionManager>,
    dir: &std::path::Path,
) -> (SessionId, Attachment, Arc<ManagedSession>, PathBuf) {
    let files = crate::agent_state::StatusFiles::for_session(dir, 1);
    let status = files.status.clone();
    let mut spec = console_spec(std::env::temp_dir(), 24, 80, None);
    spec.status = Some(files);
    let (id, att) = manager
        .spawn_attached(
            "owner/r".to_string(),
            "claude".to_string(),
            "agent".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn");
    let sess = manager.sessions.lock().expect("sessions mutex")[&id].clone();
    (id, att, sess, status)
}

/// The block the list shows for `sess` at `now`.
fn block_at(sess: &ManagedSession, now: SystemTime) -> Option<crate::agent_state::AgentState> {
    sess.agent_status
        .lock()
        .expect("agent_status mutex")
        .shown_at(now)
}

/// The state and detail the list shows for `sess` now.
fn shown(sess: &ManagedSession) -> (Option<String>, Option<String>) {
    let block = block_at(sess, SystemTime::now()).expect("a state");
    (block.state, block.detail)
}

/// The pump's tick reports a change in what the list shows, so a `working`
/// that ages into `unknown` with no new hook line is a change too: it pushes
/// `sessions.dirty`, and a tab does not keep showing a hung agent as working.
#[tokio::test]
async fn a_working_state_that_ages_out_counts_as_a_change() {
    let manager = Arc::new(SessionManager::new());
    let dir = tempfile::tempdir().expect("temp dir");
    let (id, _att, sess, status) = hooked_console(&manager, dir.path());
    append(&status, &claude_line("UserPromptSubmit"));
    let now = SystemTime::now();
    let seen = now - STALE_AFTER + Duration::from_secs(1);
    assert!(
        sess.poll_status(seen, None),
        "the first state shown is a change"
    );
    assert!(!sess.poll_status(now, None), "the same state again is not");
    let later = now + Duration::from_secs(2);
    assert!(
        sess.poll_status(later, None),
        "working aged into unknown with no new line is a change"
    );
    assert!(!sess.poll_status(later, None), "unknown again is not");
    manager.close(id);
}

/// A console that becomes silent says so in the log once, not at every tick.
#[tokio::test]
async fn a_console_that_becomes_silent_logs_one_line() {
    let manager = Arc::new(SessionManager::new());
    let dir = tempfile::tempdir().expect("temp dir");
    let (id, att, sess, _status) = hooked_console(&manager, dir.path());
    att.write(b"x").expect("a write");
    let now = SystemTime::now() + SILENT_AFTER + Duration::from_secs(1);

    let log_path = dir.path().join("daemon.log");
    let log_file = std::fs::File::create(&log_path).expect("create the log");
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || log_file.try_clone().expect("a second log handle"))
        .with_ansi(false)
        .finish();
    let changes: Vec<bool> = tracing::subscriber::with_default(subscriber, || {
        (0..4u64)
            .map(|i| sess.poll_status(now + Duration::from_secs(i), None))
            .collect()
    });
    assert_eq!(changes, vec![true, false, false, false]);
    let state = manager.list_at(now)[0]
        .agent_state
        .clone()
        .expect("a block");
    assert_eq!(state.source, Some(crate::agent_state::Unreported::Silent));
    let log = std::fs::read_to_string(&log_path).expect("read the log");
    let warnings = log.lines().filter(|l| l.contains("WARN")).count();
    assert_eq!(warnings, 1, "{log}");
    manager.close(id);
}

/// The silence clock starts at the first write that carries the operator's
/// input, not at a reply the terminal sent by itself.
#[tokio::test]
async fn the_silence_clock_starts_at_the_first_input() {
    let manager = Arc::new(SessionManager::new());
    let dir = tempfile::tempdir().expect("temp dir");
    let (id, att, sess, _status) = hooked_console(&manager, dir.path());
    let silent_at = |now: SystemTime| {
        sess.poll_status(now, None);
        block_at(&sess, now)
            .is_some_and(|b| b.source == Some(crate::agent_state::Unreported::Silent))
    };
    let far = SystemTime::now() + SILENT_AFTER * 10;
    att.write(b"").expect("an empty write");
    assert!(!silent_at(far), "an empty write is no input");
    att.write(b"\x1b[12;40R").expect("a cursor report");
    assert!(!silent_at(far), "a terminal reply is no input");
    let before = SystemTime::now();
    att.write(b"x").expect("a write");
    let after = SystemTime::now();
    assert!(
        !silent_at(before + SILENT_AFTER),
        "the clock started at the write, not before it"
    );
    assert!(
        silent_at(after + SILENT_AFTER + Duration::from_millis(1)),
        "the first write with bytes started the clock"
    );
    std::thread::sleep(Duration::from_millis(20));
    att.write(b"y").expect("a second write");
    assert!(
        silent_at(after + SILENT_AFTER + Duration::from_millis(10)),
        "a second write does not move it"
    );
    manager.close(id);
}

/// A console of `agent` with hooks, on the platform shell, after its tick
/// folded one `PermissionRequest` line: the state is `waiting`.
fn waiting_for_a_permission(
    manager: &Arc<SessionManager>,
    dir: &std::path::Path,
    agent: crate::session::Agent,
) -> (SessionId, Attachment, Arc<ManagedSession>) {
    let files = crate::agent_state::StatusFiles::for_agent(agent, dir, 1).expect("hooks");
    std::fs::write(&files.status, permission("t1")).expect("write the status line");
    let mut spec = console_spec(std::env::temp_dir(), 24, 80, None);
    spec.status = Some(files);
    let (id, att) = manager
        .spawn_attached(
            "owner/r".to_string(),
            format!("{agent:?}").to_lowercase(),
            "agent".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn");
    let sess = manager.sessions.lock().expect("sessions mutex")[&id].clone();
    sess.poll_status(SystemTime::now(), None);
    (id, att, sess)
}

/// Codex sends no event when the operator grants a permission (ADR-0085
/// D7.5): the operator's next input ends the wait, and a reply the
/// terminal sent by itself does not.
#[tokio::test]
async fn operator_input_ends_a_codex_permission_wait() {
    let manager = Arc::new(SessionManager::new());
    let dir = tempfile::tempdir().expect("temp dir");
    let (id, att, sess) =
        waiting_for_a_permission(&manager, dir.path(), crate::session::Agent::Codex);
    let waiting = (Some("waiting".into()), Some("permission: Bash".into()));
    assert_eq!(shown(&sess), waiting);
    att.write(b"\x1b[12;40R").expect("a cursor report");
    assert_eq!(shown(&sess), waiting, "a terminal reply is no input");
    att.write(b"y").expect("a write");
    assert_eq!(
        shown(&sess),
        (
            Some("working".into()),
            Some("approved, the tool runs".into())
        )
    );
    assert!(
        sess.poll_status(SystemTime::now(), None),
        "the next tick reports the change"
    );
    manager.close(id);
}

/// Claude sends `PostToolUse` when the tool runs, so the operator's input
/// does not end its permission wait.
#[tokio::test]
async fn operator_input_does_not_end_a_claude_permission_wait() {
    let manager = Arc::new(SessionManager::new());
    let dir = tempfile::tempdir().expect("temp dir");
    let (id, att, sess) =
        waiting_for_a_permission(&manager, dir.path(), crate::session::Agent::Claude);
    att.write(b"y").expect("a write");
    assert_eq!(
        shown(&sess),
        (Some("waiting".into()), Some("permission: Bash".into()))
    );
    manager.close(id);
}

/// A console launched with a reason its state is not reported shows it from
/// its start, with no hook line and no input, as a console with no state
/// source: not a silent one.
#[tokio::test]
async fn a_console_launched_with_an_unreported_reason_shows_it_at_once() {
    let manager = Arc::new(SessionManager::new());
    let mut spec = console_spec(std::env::temp_dir(), 24, 80, None);
    spec.unreported_reason = Some("no hooks here".to_string());
    let (id, _att) = manager
        .spawn_attached(
            "owner/r".to_string(),
            "codex".to_string(),
            "agent".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn");
    let block = manager.list_at(SystemTime::now())[0]
        .agent_state
        .clone()
        .expect("a block");
    assert_eq!(block.source, Some(crate::agent_state::Unreported::NoSource));
    assert_eq!(block.reason.as_deref(), Some("no hooks here"));
    manager.close(id);
}

/// A time on a fixed clock, `s` seconds after its start.
fn at(s: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_000_000 + s)
}

/// The state and detail an [`AgentStatus`] shows at `now`.
fn status_shows(status: &AgentStatus, now: SystemTime) -> (Option<String>, Option<String>) {
    let block = status.shown_at(now).expect("a state");
    (block.state, block.detail)
}

/// A Codex permission `waiting`, then the operator's input, then a tick that
/// read no new line: the tick keeps the approval the input folded
/// (ADR-0085 D7.5), and reports it as a change.
#[test]
fn an_input_then_an_empty_tick_keeps_the_approval() {
    let mut status = AgentStatus::new(1, Some(HookVendor::Codex), None);
    status.on_tick(&[permission("t1")], at(0), None);
    assert_eq!(
        status_shows(&status, at(0)),
        (Some("waiting".into()), Some("permission: Bash".into()))
    );
    status.on_input(at(1));
    assert!(
        status.on_tick(&[], at(2), None),
        "the tick reports the change"
    );
    assert_eq!(
        status_shows(&status, at(2)),
        (
            Some("working".into()),
            Some("approved, the tool runs".into())
        )
    );
}

/// The operator's input, then a tick whose lines open a new permission: the
/// newer `waiting` is shown, not the older approval.
#[test]
fn a_tick_after_an_input_shows_the_newer_waiting() {
    let mut status = AgentStatus::new(1, Some(HookVendor::Codex), None);
    status.on_tick(&[permission("t1")], at(0), None);
    status.on_input(at(1));
    status.on_tick(&[permission("t2")], at(2), None);
    assert_eq!(
        status_shows(&status, at(2)),
        (Some("waiting".into()), Some("permission: Bash".into()))
    );
}

/// The daemon's start check runs the ralphy program, so its failure makes a
/// Claude or Codex console silent at once, and not an OpenCode console,
/// whose plugin never runs that program.
#[tokio::test]
async fn a_failed_start_check_silences_only_a_vendor_that_runs_ralphy() {
    use crate::session::Agent;
    for (agent, silent) in [
        (Agent::Claude, true),
        (Agent::Codex, true),
        (Agent::OpenCode, false),
    ] {
        let manager = Arc::new(SessionManager::new());
        let dir = tempfile::tempdir().expect("temp dir");
        let files =
            crate::agent_state::StatusFiles::for_agent(agent, dir.path(), 1).expect("hooks");
        files.create_status().expect("the status file");
        let mut spec = console_spec(std::env::temp_dir(), 24, 80, None);
        spec.status = Some(files);
        let (id, _att) = manager
            .spawn_attached(
                "owner/r".to_string(),
                format!("{agent:?}").to_lowercase(),
                "agent".to_string(),
                None,
                None,
                None,
                spec,
            )
            .expect("the platform shell must spawn");
        let sess = manager.sessions.lock().expect("sessions mutex")[&id].clone();
        sess.poll_status(SystemTime::now(), Some("exit 127"));
        let shown = block_at(&sess, SystemTime::now());
        let got = shown.is_some_and(|b| b.source == Some(crate::agent_state::Unreported::Silent));
        assert_eq!(got, silent, "{agent:?}");
        manager.close(id);
    }
}
