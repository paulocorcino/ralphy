//! The agent state of a console as the manager shows it: the tick's
//! changes, the silence rule, the operator's input, and the lock order
//! between the tick and an input.

use super::*;

/// The pump's tick reports a change in what the list shows, so a `working`
/// that ages into `unknown` with no new hook line is a change too: it pushes
/// `sessions.dirty`, and a tab does not keep showing a hung agent as working.
#[tokio::test]
async fn a_working_state_that_ages_out_counts_as_a_change() {
    use crate::agent_state::{Observed, StatusFiles, STALE_AFTER};
    let manager = Arc::new(SessionManager::new());
    let dir = std::env::temp_dir().join(format!("ralphy-age-out-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut spec = console_spec(std::env::temp_dir(), 24, 80, None);
    spec.status = Some(StatusFiles::for_session(&dir, 1));
    let (id, _att) = manager
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
    let now = SystemTime::now();
    *sess.agent_state.lock().expect("agent_state mutex") = Some(Observed {
        state: "working",
        detail: None,
        since: "t".into(),
        seen: now - STALE_AFTER + std::time::Duration::from_secs(1),
        cause: None,
    });
    assert!(sess.poll_status(now), "the first state shown is a change");
    assert!(!sess.poll_status(now), "the same state again is not");
    let later = now + std::time::Duration::from_secs(2);
    assert!(
        sess.poll_status(later),
        "working aged into unknown with no new line is a change"
    );
    assert!(!sess.poll_status(later), "unknown again is not");
    manager.close(id);
    std::fs::remove_dir_all(&dir).ok();
}

/// A console with hooks, spawned on the platform shell, and its session.
fn hooked_console(
    manager: &Arc<SessionManager>,
    dir: &std::path::Path,
) -> (SessionId, Attachment, Arc<ManagedSession>) {
    let mut spec = console_spec(std::env::temp_dir(), 24, 80, None);
    spec.status = Some(crate::agent_state::StatusFiles::for_session(dir, 1));
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
    (id, att, sess)
}

/// A console that becomes silent says so in the log once, not at every tick.
#[tokio::test]
async fn a_console_that_becomes_silent_logs_one_line() {
    let manager = Arc::new(SessionManager::new());
    let dir = tempfile::tempdir().expect("temp dir");
    let (id, _att, sess) = hooked_console(&manager, dir.path());
    let now = SystemTime::now();
    *sess.first_input.lock().expect("first_input mutex") =
        Some(now - crate::agent_state::SILENT_AFTER - std::time::Duration::from_secs(1));

    let log_path = dir.path().join("daemon.log");
    let log_file = std::fs::File::create(&log_path).expect("create the log");
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || log_file.try_clone().expect("a second log handle"))
        .with_ansi(false)
        .finish();
    let changes: Vec<bool> = tracing::subscriber::with_default(subscriber, || {
        (0..4u64)
            .map(|i| sess.poll_status_with(now + std::time::Duration::from_secs(i), None))
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
    let (id, att, sess) = hooked_console(&manager, dir.path());
    let first_input = || *sess.first_input.lock().expect("first_input mutex");
    att.write(b"").expect("an empty write");
    assert_eq!(first_input(), None, "an empty write is no input");
    att.write(b"\x1b[12;40R").expect("a cursor report");
    assert_eq!(first_input(), None, "a terminal reply is no input");
    let before = SystemTime::now();
    att.write(b"x").expect("a write");
    let after = SystemTime::now();
    let first = first_input().expect("the first write with bytes sets it");
    assert!(before <= first && first <= after, "{first:?}");
    std::thread::sleep(std::time::Duration::from_millis(20));
    att.write(b"y").expect("a second write");
    assert_eq!(
        first_input(),
        Some(first),
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
    std::fs::write(
        &files.status,
        "{\"event\":\"PermissionRequest\",\"session_id\":\"s1\",\"tool_name\":\"Bash\",\"ts\":\"t1\"}\n",
    )
    .expect("write the status line");
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
    sess.poll_status_with(SystemTime::now(), None);
    (id, att, sess)
}

/// The state and detail the list shows for `sess` now.
fn shown(sess: &ManagedSession) -> (Option<String>, Option<String>) {
    let block = sess.shown_at(SystemTime::now()).expect("a state");
    (block.state, block.detail)
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
        sess.poll_status_with(SystemTime::now(), None),
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

/// An operator input that lands while a tick stores an older `waiting` is
/// not lost: the tick keeps the tail locked until it stored the state, so
/// the input folds after it and the shown state is the grant.
#[tokio::test]
async fn an_input_during_a_tick_is_not_overwritten_by_an_older_wait() {
    let manager = Arc::new(SessionManager::new());
    let dir = tempfile::tempdir().expect("temp dir");
    let files =
        crate::agent_state::StatusFiles::for_agent(crate::session::Agent::Codex, dir.path(), 1)
            .expect("hooks");
    std::fs::write(
        &files.status,
        "{\"event\":\"PermissionRequest\",\"session_id\":\"s1\",\"tool_name\":\"Bash\",\"ts\":\"t1\"}\n",
    )
    .expect("write the status line");
    let mut spec = console_spec(std::env::temp_dir(), 24, 80, None);
    spec.status = Some(files);
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
    let sess = manager.sessions.lock().expect("sessions mutex")[&id].clone();
    let input = std::sync::Mutex::new(None);
    sess.poll_status_between(SystemTime::now(), None, || {
        let writer = sess.clone();
        *input.lock().expect("input mutex") =
            Some(std::thread::spawn(move || writer.operator_input()));
        // Time for the input to fold and store, if the tail let it.
        std::thread::sleep(std::time::Duration::from_millis(200));
    });
    let handle = input
        .lock()
        .expect("input mutex")
        .take()
        .expect("the input thread");
    handle.join().expect("the input thread ends");
    assert_eq!(
        shown(&sess),
        (
            Some("working".into()),
            Some("approved, the tool runs".into())
        )
    );
    manager.close(id);
}

/// A tick that lands while an operator input stores its `working` is not
/// overwritten by it: the tick waits for the tail, then reads the newer
/// `waiting` line.
#[tokio::test]
async fn a_tick_during_an_input_is_not_overwritten_by_the_older_input() {
    let manager = Arc::new(SessionManager::new());
    let dir = tempfile::tempdir().expect("temp dir");
    let files =
        crate::agent_state::StatusFiles::for_agent(crate::session::Agent::Codex, dir.path(), 1)
            .expect("hooks");
    let permission = |ts: &str| {
        format!(
            "{{\"event\":\"PermissionRequest\",\"session_id\":\"s1\",\"tool_name\":\"Bash\",\"ts\":\"{ts}\"}}\n"
        )
    };
    std::fs::write(&files.status, permission("t1")).expect("write the status line");
    let status = files.status.clone();
    let mut spec = console_spec(std::env::temp_dir(), 24, 80, None);
    spec.status = Some(files);
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
    let sess = manager.sessions.lock().expect("sessions mutex")[&id].clone();
    sess.poll_status_with(SystemTime::now(), None);
    let tick = std::sync::Mutex::new(None);
    sess.operator_input_between(|| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&status)
            .expect("open the status file");
        file.write_all(permission("t2").as_bytes())
            .expect("append the newer wait");
        let poller = sess.clone();
        *tick.lock().expect("tick mutex") = Some(std::thread::spawn(move || {
            poller.poll_status_with(SystemTime::now(), None)
        }));
        // Time for the tick to read and store, if the tail let it.
        std::thread::sleep(std::time::Duration::from_millis(200));
    });
    let handle = tick
        .lock()
        .expect("tick mutex")
        .take()
        .expect("the tick thread");
    handle.join().expect("the tick thread ends");
    assert_eq!(shown(&sess).0, Some("waiting".into()));
    manager.close(id);
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
        sess.poll_status_with(SystemTime::now(), Some("exit 127"));
        let shown = sess.shown_at(SystemTime::now());
        let got = shown.is_some_and(|b| b.source == Some(crate::agent_state::Unreported::Silent));
        assert_eq!(got, silent, "{agent:?}");
        manager.close(id);
    }
}
