use super::kept::{push_capped, SCROLLBACK_CAP_BYTES};
use super::*;
use crate::session::console_spec;
use crate::session::screen::Screen;

mod attempt;
mod replay;
mod resume;

#[test]
fn scrollback_ring_is_bounded() {
    let mut ring = std::collections::VecDeque::new();
    // Fed in ≥2 chunks: proves the cap holds across successive pushes, not
    // only on a single oversized write.
    push_capped(&mut ring, b"012345", 8);
    push_capped(&mut ring, b"6789AB", 8);
    assert_eq!(ring.len(), 8, "ring must not exceed the cap");
    assert_eq!(
        ring.iter().copied().collect::<Vec<u8>>(),
        b"456789AB".to_vec(),
        "the FRONT is dropped and the tail retained"
    );
}

/// The replay of a reattach is the ring; a cap of 256 KiB gave 407 of 600
/// measured reattaches a full ring (#689).
#[test]
fn scrollback_cap_is_512_kib() {
    assert_eq!(SCROLLBACK_CAP_BYTES, 512 * 1024);
}

/// The writer slot and the watcher list are separate registers: a watcher may
/// never make the slot LOOK occupied (which would refuse an honest attach with
/// `409`), and it must be reachable while the slot IS held (which is the whole
/// point — a second workbench sees the session instead of stealing it).
///
/// Spawns the platform shell rather than the helper child bin: `CARGO_BIN_EXE_*`
/// is visible only to integration tests (docs/TESTING.md → What may be faked), and
/// nothing here talks to the child — the session only has to be LIVE.
#[tokio::test]
async fn a_watcher_does_not_occupy_the_writer_slot() {
    let manager = Arc::new(SessionManager::new());
    let spec = console_spec(std::env::temp_dir(), 24, 80, None);
    let (id, writer) = manager
        .spawn_attached(
            "~".to_string(),
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn — the free console depends on it");
    drop(writer); // the slot is free; only the watcher list is about to fill

    let watcher = manager
        .watch(id, None)
        .expect("a live session is always watchable");
    assert!(!watcher.writer, "watch() yields a reader, never the baton");
    let taker = manager
        .attach(id, false)
        .expect("a registered watcher must not make the writer slot look busy");
    assert!(taker.writer, "a plain attach takes the baton");
    let second = manager
        .watch(id, None)
        .expect("a BUSY session is still watchable — watch never refuses with Busy");
    assert!(!second.writer);

    // A departing watcher deregisters EXACTLY its own token: drop the first
    // and the second must still be told the session ended. Inverting the
    // guard's `ptr_eq` (keep the departing one, drop everyone else) makes
    // this reason `None`.
    let survivor = second.evict.clone();
    drop(watcher);
    manager.close(id);
    assert_eq!(
        survivor.reason(),
        Some(EndReason::ChildExited),
        "a watcher that outlives another must still receive the end signal"
    );
    assert_eq!(
        taker.evict.reason(),
        Some(EndReason::ChildExited),
        "…and so must the writer"
    );
}

/// ADR-0059 §6 through the listing, not the pure `render`: a `working`
/// observed longer than `STALE_AFTER` ago lists as `unknown`, a fresh one
/// as `working`, a `waiting` never ages — and `list` reads the observation
/// the pump stored, so replacing the render with a bare clone would red.
/// A status-less console (a shell) lists no state at all.
#[tokio::test]
async fn list_renders_the_agent_state_with_the_staleness_rule() {
    use crate::agent_state::{Observed, STALE_AFTER};
    let manager = Arc::new(SessionManager::new());
    let (id, _att) = manager
        .spawn_attached(
            "owner/r".to_string(),
            "claude".to_string(),
            "agent".to_string(),
            None,
            None,
            None,
            console_spec(std::env::temp_dir(), 24, 80, None),
        )
        .expect("the platform shell must spawn");
    let now = SystemTime::now();
    assert_eq!(
        manager.list_at(now)[0].agent_state,
        None,
        "no hook fired, no state"
    );
    let observe = |state: &'static str, seen: SystemTime| {
        let sess = manager.sessions.lock().expect("sessions mutex")[&id].clone();
        *sess.agent_state.lock().expect("agent_state mutex") = Some(Observed {
            state,
            detail: None,
            since: "t".into(),
            seen,
            cause: None,
        });
    };
    observe(
        "working",
        now - STALE_AFTER + std::time::Duration::from_secs(60),
    );
    assert_eq!(
        manager.list_at(now)[0]
            .agent_state
            .as_ref()
            .and_then(|a| a.state.as_deref()),
        Some("working")
    );
    observe(
        "working",
        now - STALE_AFTER - std::time::Duration::from_secs(60),
    );
    assert_eq!(
        manager.list_at(now)[0]
            .agent_state
            .as_ref()
            .and_then(|a| a.state.as_deref()),
        Some("unknown")
    );
    assert_eq!(
        manager
            .get(id)
            .and_then(|i| i.agent_state)
            .and_then(|a| a.state),
        Some("unknown".to_string()),
        "`get` renders too"
    );
    observe(
        "waiting",
        now - STALE_AFTER - std::time::Duration::from_secs(60),
    );
    assert_eq!(
        manager.list_at(now)[0]
            .agent_state
            .as_ref()
            .and_then(|a| a.state.as_deref()),
        Some("waiting"),
        "a question never goes stale"
    );
    manager.close(id);
}

/// `console_in` is the (repo, checkout) pair, exactly: another repo's
/// console in a same-named worktree, or this repo's console in the primary
/// (`checkout: None`), never gates a remove. Spawns the platform shell for
/// the same reason as the watcher test above.
#[tokio::test]
async fn console_in_matches_only_the_repo_and_checkout_pair() {
    let manager = Arc::new(SessionManager::new());
    let spawn = |repo: &str, checkout: Option<&str>| {
        manager
            .spawn_attached(
                repo.to_string(),
                "claude".to_string(),
                "agent".to_string(),
                None,
                checkout.map(str::to_string),
                None,
                console_spec(std::env::temp_dir(), 24, 80, None),
            )
            .expect("the platform shell must spawn")
    };
    assert!(!manager.console_in("owner/r", "wt-a"), "empty table");
    let (in_wt, _w1) = spawn("owner/r", Some("wt-a"));
    let (_primary, _w2) = spawn("owner/r", None);
    let (_other, _w3) = spawn("owner/other", Some("wt-a"));
    assert!(manager.console_in("owner/r", "wt-a"));
    assert!(
        !manager.console_in("owner/r", "wt-b"),
        "a different worktree"
    );
    assert!(
        !manager.console_in("owner/x", "wt-a"),
        "a same-named worktree of another repo"
    );
    manager.close(in_wt);
    assert!(
        !manager.console_in("owner/r", "wt-a"),
        "closed: the primary's console (checkout None) is not a match"
    );
    for id in manager.list().iter().map(|s| s.id) {
        manager.close(id);
    }
}

/// A restarted daemon must not re-issue an id the last one used: a browser
/// still holding `?id=N` would otherwise reattach to whichever NEW session drew
/// N. A manager over the same record continues past it; a first boot starts at 1.
#[test]
fn a_manager_over_the_same_record_continues_past_the_last_id() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("daemon-session-id");

    let first = SessionManager::continuing(file.clone());
    assert_eq!(first.reserve_id(), 1, "a first boot starts at 1");
    assert_eq!(first.reserve_id(), 2);
    drop(first);

    let restarted = SessionManager::continuing(file.clone());
    assert_eq!(
        restarted.reserve_id(),
        3,
        "a restart continues past the record"
    );
    assert_eq!(
        std::fs::read_to_string(&file).expect("the record").trim(),
        "3"
    );
}

/// A corrupt record restarts the sequence rather than refusing sessions.
#[test]
fn a_corrupt_record_restarts_at_one() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("daemon-session-id");
    std::fs::write(&file, "not a number").expect("seeding the record");
    assert_eq!(SessionManager::continuing(file).reserve_id(), 1);
}

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
    assert_eq!(state.source.as_deref(), Some("silent"));
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

/// A console launched with a silence reason is silent with it from its
/// start, with no hook line and no input.
#[tokio::test]
async fn a_console_launched_with_a_silence_reason_is_silent_at_once() {
    let manager = Arc::new(SessionManager::new());
    let mut spec = console_spec(std::env::temp_dir(), 24, 80, None);
    spec.silent_reason = Some("no hooks here".to_string());
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
    assert_eq!(block.source.as_deref(), Some("silent"));
    assert_eq!(block.reason.as_deref(), Some("no hooks here"));
    manager.close(id);
}
