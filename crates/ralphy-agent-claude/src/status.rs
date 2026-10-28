//! The agent's own state from Claude Code's hooks (docs/adr/0059 §1, §3).
//!
//! `ralphy hook status` appends one JSON line per hook event to
//! `$RALPHY_STATUS_FILE`; this module is the other end: [`fold_line`] maps a
//! line to a state per the §1 table, and [`Watcher`] tails the file from a
//! thread while a child runs — plan, interactive and headless alike — and
//! calls `emit::agent_state` on a CHANGE only, so a fold downstream is a
//! transition. The mapping is pinned by `tests/fixtures/agent_state_mapping.json`,
//! which the daemon's own copy of the fold is pinned by too (§5: the daemon
//! never imports this crate).
//!
//! Nothing here reads the terminal: a state that is guessed is worse than one
//! that is absent (§7).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::Value;

/// The file name under the run dir, beside `status.flag`.
pub(crate) const STATUS_FILE: &str = "agent-status.jsonl";

/// The environment variable the hook reads the path from.
pub(crate) const STATUS_ENV: &str = "RALPHY_STATUS_FILE";

/// One observed state, folded from one hook line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observed {
    pub state: &'static str,
    pub detail: Option<String>,
    pub ts: String,
}

/// The §1 table, one hook line in, one state out — or `None` for a line that
/// changes no state (`SubagentStop`: a subagent never owns the state; an
/// event outside the table; a line that is not JSON).
pub(crate) fn fold_line(line: &str) -> Option<Observed> {
    let v: Value = serde_json::from_str(line).ok()?;
    let event = v.get("event").and_then(Value::as_str).unwrap_or("");
    let tool = v.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let ts = v
        .get("ts")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let (state, detail) = match event {
        "SessionStart" => ("done", None),
        "UserPromptSubmit" => ("working", None),
        "PreToolUse" if tool == "AskUserQuestion" => {
            ("waiting", Some(question_detail(v.get("tool_input"))))
        }
        "PreToolUse" => ("working", None),
        // A tool that returned: the question was answered, the permission
        // granted — `waiting` ends here, not at the next tool (2026-09-16).
        "PostToolUse" => ("working", None),
        "PermissionRequest" => (
            "waiting",
            Some(if tool.is_empty() {
                "permission".to_string()
            } else {
                format!("permission: {tool}")
            }),
        ),
        "Stop" => ("done", None),
        _ => return None,
    };
    Some(Observed { state, detail, ts })
}

/// What an `AskUserQuestion` asks, from its `tool_input`: the first question's
/// text when the shape is the vendor's (`questions: [{ question }]`), else the
/// tool name alone.
fn question_detail(input: Option<&Value>) -> String {
    let text = input
        .and_then(|i| i.get("questions"))
        .and_then(Value::as_array)
        .and_then(|qs| qs.first())
        .and_then(|q| q.get("question"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match text {
        Some(t) => format!("AskUserQuestion: {t}"),
        None => "AskUserQuestion".to_string(),
    }
}

/// The tail: reads what was appended since the last poll, folds each
/// complete line, and reports only the transitions — a change of state, or
/// a `waiting` with a new detail.
pub(crate) struct Tail {
    path: PathBuf,
    offset: u64,
    carry: Vec<u8>,
    last: Option<(&'static str, Option<String>)>,
}

impl Tail {
    pub(crate) fn new(path: PathBuf) -> Self {
        Tail {
            path,
            offset: 0,
            carry: Vec::new(),
            last: None,
        }
    }

    /// The states that CHANGED since the last poll, in order. A `waiting`
    /// with a new detail counts as a change (a second question is news); the
    /// same state with the same detail does not — the same permission asked
    /// twice is one buzz, not two.
    pub(crate) fn poll(&mut self) -> Vec<Observed> {
        let mut out = Vec::new();
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return out;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return out;
        }
        let mut buf = Vec::new();
        let Ok(n) = file.read_to_end(&mut buf) else {
            return out;
        };
        self.offset += n as u64;
        self.carry.extend_from_slice(&buf);
        while let Some(nl) = self.carry.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.carry.drain(..=nl).collect();
            let Ok(text) = std::str::from_utf8(&line) else {
                continue;
            };
            if let Some(obs) = fold_line(text.trim()) {
                let key = (obs.state, obs.detail.clone());
                if self.last.as_ref() != Some(&key) {
                    self.last = Some(key);
                    out.push(obs);
                }
            }
        }
        out
    }
}

/// Tails the status file from a thread for as long as a child runs and emits
/// each transition. `start` truncates the file (a previous issue's lines are
/// not this child's) and returns the path to hand the child as
/// `RALPHY_STATUS_FILE`; `stop` takes the last look and joins.
pub(crate) struct Watcher {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    path: PathBuf,
}

impl Watcher {
    pub(crate) fn start(run_dir: &Path) -> Self {
        let path = run_dir.join(STATUS_FILE);
        // Truncate, never fail: a status file that cannot be reset is a
        // status that may be stale, not a run that may not start.
        if let Err(e) = std::fs::write(&path, b"") {
            tracing::warn!(error = %e, path = %path.display(), "could not reset the agent-status file");
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let mut tail = Tail::new(path.clone());
        let handle = std::thread::spawn(move || loop {
            for obs in tail.poll() {
                emit(&obs);
            }
            if flag.load(Ordering::SeqCst) {
                // One last look: a `Stop` line lands right before the child
                // exits, and the loop must not lose it to the join.
                for obs in tail.poll() {
                    emit(&obs);
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        });
        Watcher {
            stop,
            handle: Some(handle),
            path,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            // A panicked watcher thread has nothing to report; the run goes on.
            let _joined = h.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _joined = h.join();
        }
    }
}

/// How long a session has to write its `SessionStart` line before the
/// prompt is refused (ADR-0072 D6, amendment of 2026-10-28). M5 and M6 of
/// `docs/research/claude-hook-probes-2026-10-28.md` measured 0.8 s to 3.2 s.
pub(crate) const PROOF_WINDOW: Duration = Duration::from_secs(30);

/// How often [`await_session_start`] reads the status file.
const PROOF_POLL: Duration = Duration::from_millis(200);

/// Wait until the status file at `path` holds a `SessionStart` line: the
/// proof that the hooks of `settings` run in this session. `project` is the
/// session's working folder, read for a `disableAllHooks` setting when the
/// line does not come. After `window` with no line, the error is the
/// [`ralphy_core::HooksUnproven`] cause, for [`refuse_or_go_on`].
pub(crate) fn await_session_start(
    path: &Path,
    settings: &Path,
    project: &Path,
    window: Duration,
) -> Result<(), ralphy_core::HooksUnproven> {
    let deadline = std::time::Instant::now() + window;
    loop {
        if session_started(path, 0) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(no_session_start(settings, project, window));
        }
        std::thread::sleep(PROOF_POLL);
    }
}

/// Whether the status file at `path` holds a `SessionStart` line after its
/// first `from` bytes. A respawned child shares the file with the first one,
/// so it passes the length the file had before its spawn. Never blocks.
pub(crate) fn session_started(path: &Path, from: u64) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let start = usize::try_from(from).map_or(bytes.len(), |f| f.min(bytes.len()));
    String::from_utf8_lossy(&bytes[start..])
        .lines()
        .any(|line| {
            serde_json::from_str::<Value>(line.trim())
                .is_ok_and(|v| v.get("event").and_then(Value::as_str) == Some("SessionStart"))
        })
}

/// The cause of a session that wrote no `SessionStart` line in `window`.
pub(crate) fn no_session_start(
    settings: &Path,
    project: &Path,
    window: Duration,
) -> ralphy_core::HooksUnproven {
    ralphy_core::HooksUnproven {
        cause: no_proof_cause(settings, project, window),
    }
}

/// The cause of a missing proof. A `disableAllHooks` file is named first: it
/// alone explains the silence.
fn no_proof_cause(settings: &Path, project: &Path, window: Duration) -> String {
    let home = ralphy_proc_util::home_dir();
    let mut cause = String::new();
    if let Some(file) =
        ralphy_proc_util::hook_form::disable_all_hooks(home.as_deref(), Some(project))
    {
        cause.push_str(&format!(
            "{} sets disableAllHooks to true, so Claude Code runs no hook; ",
            file.display()
        ));
    }
    let program = ralphy_proc_util::exe::running_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|e| format!("the ralphy program (not found: {e:#})"));
    cause.push_str(&format!(
        "no SessionStart hook event came in {} s; the hooks in {} run {}. Fix the cause and start again",
        window.as_secs(),
        settings.display(),
        program
    ));
    cause
}

/// The cause of a session whose hooks would run through a shell: such a
/// hook cannot prove the program it runs. `reason` is the hook form module's.
pub(crate) fn shell_form(reason: &str) -> ralphy_core::HooksUnproven {
    ralphy_core::HooksUnproven {
        cause: format!("{reason}; update Claude Code to 2.1.139 or later"),
    }
}

/// The settings key that lets an unproven session go on.
pub(crate) const WAIVER_KEY: &str = "claude.run_without_guard_proof_i_understand_the_risk";

/// Every unproven session ends here. With the waiver (a local-only setting,
/// ADR-0072 D6) the session goes on and one warning event carries the
/// cause; without it, the refusal names the cause and the way out.
pub(crate) fn refuse_or_go_on(
    cause: ralphy_core::HooksUnproven,
    waiver: bool,
) -> anyhow::Result<()> {
    if waiver {
        ralphy_core::emit::hooks_unproven_allowed(&cause.cause);
        return Ok(());
    }
    Err(anyhow::Error::new(ralphy_core::HooksUnproven {
        cause: format!(
            "{}. To run without the proof on this machine, run `ralphy config set {WAIVER_KEY} true`",
            cause.cause
        ),
    }))
}

/// The gate of a headless session: refuse a shell form now, before the
/// spawn, else hold the prompt until the session's `SessionStart` line lands
/// in `status_file`. With the waiver, a shell form sends the prompt at once
/// and a missing line sends it after the window.
pub(crate) fn proof_gate<'a>(
    settings: &'a crate::settings::SettingsFile,
    status_file: PathBuf,
    project: &'a Path,
    waiver: bool,
) -> anyhow::Result<ralphy_adapter_support::PromptGate<'a>> {
    if let Some(reason) = &settings.unproven {
        refuse_or_go_on(shell_form(reason), waiver)?;
        return Ok(Box::new(|| Ok(())));
    }
    Ok(Box::new(move || {
        match await_session_start(&status_file, &settings.path, project, PROOF_WINDOW) {
            Ok(()) => Ok(()),
            Err(cause) => refuse_or_go_on(cause, waiver),
        }
    }))
}

fn emit(obs: &Observed) {
    ralphy_core::emit::agent_state(obs.state, &obs.ts, obs.detail.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared mapping fixture: every row is one hook line and the state
    /// it must fold to. The daemon pins ITS fold against the same file
    /// (ADR-0059 §5), so the two cannot drift.
    const MAPPING: &str = include_str!("../tests/fixtures/agent_state_mapping.json");

    #[test]
    fn fold_line_matches_the_shared_mapping_fixture() {
        let rows: Vec<Value> = serde_json::from_str(MAPPING).unwrap();
        assert!(rows.len() >= 8, "the fixture covers the whole table");
        for row in rows {
            let line = serde_json::to_string(&row["line"]).unwrap();
            let got = fold_line(&line);
            match row["state"].as_str() {
                None => assert_eq!(got, None, "{line} must fold to nothing"),
                Some(state) => {
                    let got = got.unwrap_or_else(|| panic!("{line} must fold to {state}"));
                    assert_eq!(got.state, state, "{line}");
                    assert_eq!(
                        got.detail.as_deref(),
                        row["detail"].as_str(),
                        "detail for {line}"
                    );
                }
            }
        }
    }

    /// The tail reports transitions only, survives a partial line across two
    /// polls, and treats a second `waiting` with a new question as news.
    #[test]
    fn tail_reports_transitions_and_handles_partial_lines() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATUS_FILE);
        let mut f = std::fs::File::create(&path).unwrap();
        let mut tail = Tail::new(path.clone());
        assert!(tail.poll().is_empty());

        let line = |event: &str, tool: &str| {
            format!(
                r#"{{"event":"{event}","tool_name":{},"tool_input":null,"ts":"t"}}"#,
                if tool.is_empty() {
                    "null".to_string()
                } else {
                    format!("\"{tool}\"")
                }
            )
        };
        writeln!(f, "{}", line("UserPromptSubmit", "")).unwrap();
        writeln!(f, "{}", line("PreToolUse", "Bash")).unwrap();
        writeln!(f, "{}", line("PreToolUse", "Edit")).unwrap();
        f.flush().unwrap();
        let got: Vec<&str> = tail.poll().iter().map(|o| o.state).collect::<Vec<_>>();
        assert_eq!(got, vec!["working"], "three working lines, one transition");

        // A partial line: nothing until the newline lands.
        let stop = line("Stop", "");
        let (head, rest) = stop.split_at(10);
        write!(f, "{head}").unwrap();
        f.flush().unwrap();
        assert!(tail.poll().is_empty());
        writeln!(f, "{rest}").unwrap();
        f.flush().unwrap();
        let got: Vec<&str> = tail.poll().iter().map(|o| o.state).collect();
        assert_eq!(got, vec!["done"]);

        // Two DIFFERENT questions in a row are two `waiting`s; the same
        // question re-asked is not a third.
        let ask = |q: &str| {
            format!(
                r#"{{"event":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{{"questions":[{{"question":"{q}"}}]}},"ts":"t"}}"#
            )
        };
        writeln!(f, "{}", ask("port?")).unwrap();
        writeln!(f, "{}", ask("host?")).unwrap();
        writeln!(f, "{}", ask("host?")).unwrap();
        f.flush().unwrap();
        let got: Vec<String> = tail
            .poll()
            .into_iter()
            .map(|o| o.detail.unwrap_or_default())
            .collect();
        assert_eq!(
            got,
            vec!["AskUserQuestion: port?", "AskUserQuestion: host?"]
        );
    }

    /// Every child shape hands the hook its file: the plan `Command`, the PTY
    /// `PtyCommand` and the headless `Command` each set `STATUS_ENV` from a
    /// `Watcher` they started. A source pin, because the three spawns are
    /// not reachable without a vendor binary; the daemon's launch is pinned
    /// the same way against its helper child.
    #[test]
    fn every_child_shape_is_handed_the_status_file() {
        let bind = regex::Regex::new(r"let(\w+)=(?:crate::)?status::Watcher::start\(").unwrap();
        for (name, src) in [
            ("lib.rs (plan)", include_str!("lib.rs")),
            ("interactive.rs (PTY)", include_str!("interactive.rs")),
            ("headless.rs", include_str!("headless.rs")),
        ] {
            // Production text with whitespace removed: the call, not its layout.
            let code: String = src.split_whitespace().collect();
            let code = code.split("#[cfg(test)]mod").next().unwrap_or_default();
            let watcher = bind
                .captures(code)
                .map(|c| c[1].to_string())
                .unwrap_or_else(|| panic!("{name} must start a Watcher"));
            assert!(
                code.contains(&format!("status::STATUS_ENV,{watcher}.path())")),
                "{name} must hand the child RALPHY_STATUS_FILE from that Watcher"
            );
        }
        // The tasks run no Watcher: each hands its child a status file of its own.
        let tasks: String = include_str!("tasks.rs").split_whitespace().collect();
        let tasks = tasks.split("#[cfg(test)]mod").next().unwrap_or_default();
        assert!(
            tasks.contains("cmd.env(crate::status::STATUS_ENV,&status_file)"),
            "tasks.rs must hand the child RALPHY_STATUS_FILE"
        );
    }

    // ---- the Watcher, end to end through `tracing` (ADR-0059 §3) --------
    //
    // A process-global capturing layer, installed once: the Watcher emits from
    // ITS OWN thread, so a thread-local subscriber would never see it. Every
    // `agent state` event lands in one shared sink; each test picks its own
    // events out by the `since` it wrote into the lines.
    use std::sync::{Arc, Mutex, OnceLock};
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::Layer;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Seen {
        state: String,
        since: String,
        detail: String,
    }

    type Sink = Arc<Mutex<Vec<Seen>>>;

    struct Capture(Sink);

    impl<S: tracing::Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            #[derive(Default)]
            struct Fields {
                message: String,
                state: String,
                since: String,
                detail: String,
            }
            impl tracing::field::Visit for Fields {
                fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
                    match f.name() {
                        "state" => self.state = v.into(),
                        "since" => self.since = v.into(),
                        "detail" => self.detail = v.into(),
                        _ => {}
                    }
                }
                fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                    if f.name() == "message" {
                        self.message = format!("{v:?}");
                    }
                }
            }
            let mut fields = Fields::default();
            event.record(&mut fields);
            if fields.message == ralphy_core::emit::AGENT_STATE_MSG {
                self.0.lock().expect("sink").push(Seen {
                    state: fields.state,
                    since: fields.since,
                    detail: fields.detail,
                });
            }
        }
    }

    fn sink() -> Sink {
        static SINK: OnceLock<Sink> = OnceLock::new();
        SINK.get_or_init(|| {
            let sink: Sink = Arc::new(Mutex::new(Vec::new()));
            // A default set by another test in this binary would make this a
            // no-op and the assertions below would fail loudly, not silently.
            tracing::subscriber::set_global_default(
                tracing_subscriber::registry().with(Capture(sink.clone())),
            )
            .expect("this test binary installs the one global subscriber");
            sink
        })
        .clone()
    }

    fn mine(sink: &Sink, tag: &str) -> Vec<Seen> {
        sink.lock()
            .expect("sink")
            .iter()
            .filter(|s| s.since.starts_with(tag))
            .cloned()
            .collect()
    }

    /// The whole run-path contract: `start` truncates a previous issue's
    /// lines; lines the "hook" appends while the child runs become
    /// `emit::agent_state` calls on TRANSITIONS only (three `working`s are
    /// one event); a `Stop` written right before `stop()` is not lost to the
    /// join; and the events carry the line's own `ts`, detail and interrupt.
    #[test]
    fn watcher_emits_one_agent_state_per_transition_including_the_last_line() {
        use std::io::Write;
        let sink = sink();
        let tag = format!("watcher-{}-", std::process::id());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATUS_FILE);
        // A previous issue's leftover: must NOT be folded into this run.
        std::fs::write(
            &path,
            format!(r#"{{"event":"PermissionRequest","tool_name":"Bash","ts":"{tag}stale"}}"#)
                + "\n",
        )
        .unwrap();

        let watcher = Watcher::start(dir.path());
        assert_eq!(watcher.path(), path, "the path handed to the child");
        assert_eq!(std::fs::read(&path).unwrap(), b"", "start truncates");

        let line = |event: &str, tool: &str, extra: &str, ts: &str| {
            let tool = if tool.is_empty() {
                "null".to_string()
            } else {
                format!("\"{tool}\"")
            };
            format!(r#"{{"event":"{event}","tool_name":{tool},{extra}"ts":"{tag}{ts}"}}"#)
        };
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{}", line("UserPromptSubmit", "", "", "t1")).unwrap();
        writeln!(f, "{}", line("PreToolUse", "Bash", "", "t2")).unwrap();
        writeln!(f, "{}", line("PreToolUse", "Edit", "", "t3")).unwrap();
        writeln!(
            f,
            "{}",
            line(
                "PreToolUse",
                "AskUserQuestion",
                r#""tool_input":{"questions":[{"question":"which port?"}]},"#,
                "t4"
            )
        )
        .unwrap();
        f.flush().unwrap();
        // Let the thread take at least one poll (500 ms cadence).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while mine(&sink, &tag).len() < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // The last line lands right before the stop — `stop()`'s final look
        // must fold it, whatever the poll cadence did.
        writeln!(
            f,
            "{}",
            r#"{"event":"Stop","tool_name":null,"ts":"TAGt5"}"#.replace("TAG", &tag)
        )
        .unwrap();
        f.flush().unwrap();
        watcher.stop();

        let got = mine(&sink, &tag);
        let brief: Vec<(&str, &str, &str)> = got
            .iter()
            .map(|s| {
                (
                    s.state.as_str(),
                    s.since.trim_start_matches(tag.as_str()),
                    s.detail.as_str(),
                )
            })
            .collect();
        assert_eq!(
            brief,
            vec![
                ("working", "t1", ""),
                ("waiting", "t4", "AskUserQuestion: which port?"),
                ("done", "t5", ""),
            ],
            "transitions only, in order, with the line's own ts; the stale line never folded: {got:?}"
        );
    }

    #[test]
    fn the_proof_passes_on_the_session_start_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATUS_FILE);
        std::fs::write(&path, b"").unwrap();
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                std::fs::write(
                    &path,
                    "{\"event\":\"SessionStart\",\"ts\":\"t\"}
",
                )
                .unwrap();
            })
        };
        let got = await_session_start(
            &path,
            Path::new("s.json"),
            dir.path(),
            Duration::from_secs(5),
        );
        writer.join().unwrap();
        assert!(got.is_ok(), "{got:?}");
    }

    #[test]
    fn no_session_start_line_is_refused_with_the_cause() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        let disabling = project.join(".claude").join("settings.json");
        std::fs::write(&disabling, r#"{"disableAllHooks": true}"#).unwrap();
        let path = dir.path().join(STATUS_FILE);
        std::fs::write(
            &path,
            "{\"event\":\"Stop\",\"ts\":\"t\"}
",
        )
        .unwrap();
        let settings = dir.path().join("ralphy.settings.json");

        let started = std::time::Instant::now();
        let err = await_session_start(&path, &settings, &project, Duration::from_secs(1))
            .expect_err("no SessionStart line in the window");
        assert!(started.elapsed() < Duration::from_secs(3));
        let cause = &err.cause;
        let program = ralphy_proc_util::exe::running_exe().unwrap();
        let at = |needle: &Path| {
            cause
                .find(&needle.display().to_string())
                .unwrap_or_else(|| panic!("{cause} names {}", needle.display()))
        };
        let (disabled_at, settings_at, program_at) = (at(&disabling), at(&settings), at(&program));
        assert!(
            disabled_at < settings_at && disabled_at < program_at,
            "{cause}"
        );
    }

    #[test]
    fn a_shell_form_session_is_refused_with_the_update_message() {
        let dir = tempfile::tempdir().unwrap();
        let settings = crate::settings::SettingsFile {
            path: dir.path().join("ralphy.settings.json"),
            unproven: Some("Claude Code 2.1.138 runs a hook through a shell".into()),
        };
        let Err(err) = proof_gate(&settings, dir.path().join(STATUS_FILE), dir.path(), false)
        else {
            panic!("a shell form gets no gate");
        };
        // The runner reads the type through the context the session adds.
        let err = err.context("the prompt was held and not sent");
        let refusal = err
            .downcast_ref::<ralphy_core::HooksUnproven>()
            .expect("the refusal type");
        assert!(
            refusal
                .cause
                .contains("update Claude Code to 2.1.139 or later"),
            "{}",
            refusal.cause
        );
    }

    /// The `cause` of each `hooks not proven` event emitted on this thread
    /// while `f` runs.
    fn waiver_events(f: impl FnOnce()) -> Vec<String> {
        struct Causes(Arc<Mutex<Vec<String>>>);
        impl<S: tracing::Subscriber> Layer<S> for Causes {
            fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
                #[derive(Default)]
                struct F {
                    message: String,
                    cause: String,
                }
                impl tracing::field::Visit for F {
                    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                        match f.name() {
                            "message" => self.message = format!("{v:?}"),
                            "cause" => self.cause = format!("{v:?}"),
                            _ => {}
                        }
                    }
                }
                let mut fields = F::default();
                event.record(&mut fields);
                if fields.message == ralphy_core::emit::HOOKS_UNPROVEN_ALLOWED_MSG {
                    self.0.lock().expect("causes").push(fields.cause);
                }
            }
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Causes(seen.clone()));
        tracing::subscriber::with_default(subscriber, f);
        let got = seen.lock().expect("causes").clone();
        got
    }

    fn a_cause() -> ralphy_core::HooksUnproven {
        ralphy_core::HooksUnproven {
            cause: "no SessionStart hook event came in 30 s".into(),
        }
    }

    #[test]
    fn the_waiver_lets_an_unproven_session_go_on_with_one_warning() {
        let mut got = None;
        let causes = waiver_events(|| got = Some(refuse_or_go_on(a_cause(), true)));
        let got = got.expect("ran");
        assert!(got.is_ok(), "{got:?}");
        assert_eq!(causes, vec!["no SessionStart hook event came in 30 s"]);
    }

    #[test]
    fn without_the_waiver_the_refusal_names_the_way_out() {
        let mut got = None;
        let causes = waiver_events(|| got = Some(refuse_or_go_on(a_cause(), false)));
        let err = got.expect("ran").expect_err("refused");
        let refusal = err
            .downcast_ref::<ralphy_core::HooksUnproven>()
            .expect("the refusal type");
        assert!(
            refusal
                .cause
                .starts_with("no SessionStart hook event came in 30 s"),
            "{}",
            refusal.cause
        );
        assert!(
            refusal
                .cause
                .contains("claude.run_without_guard_proof_i_understand_the_risk"),
            "{}",
            refusal.cause
        );
        assert!(causes.is_empty(), "a refusal emits no warning: {causes:?}");
    }

    /// A `PostToolUse` after a `waiting` is the operator having answered:
    /// the state is `working` again without waiting for the next tool.
    #[test]
    fn a_tool_returning_ends_a_waiting() {
        let ask = fold_line(
            r#"{"event":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{},"ts":"t"}"#,
        )
        .unwrap();
        assert_eq!(ask.state, "waiting");
        let back =
            fold_line(r#"{"event":"PostToolUse","tool_name":"AskUserQuestion","ts":"t"}"#).unwrap();
        assert_eq!((back.state, back.detail), ("working", None));
    }
}
