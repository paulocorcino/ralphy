use super::*;

use ralphy_adapter_support::agent_state::{Fold, Tail};

/// The shared mapping fixture: every row is a sequence of status lines
/// and the state after each one. The daemon pins ITS fold against the
/// same file (ADR-0059 §5), so the two cannot drift.
const MAPPING: &str = include_str!("../../tests/fixtures/agent_state_mapping.json");

// The daemon has the same test against the same fixture (ADR-0059 §5).
// jscpd:ignore-start
#[test]
fn fold_matches_the_shared_mapping_fixture() {
    let rows: Vec<Value> = serde_json::from_str(MAPPING).unwrap();
    assert!(rows.len() >= 20, "the fixture covers the whole table");
    for row in rows {
        let why = row["why"].as_str().unwrap_or_default();
        let mut fold = Fold::new();
        let lines = row["lines"].as_array().expect("lines");
        let states = row["states"].as_array().expect("states");
        assert_eq!(lines.len(), states.len(), "{why}");
        for (i, (line, want)) in lines.iter().zip(states).enumerate() {
            let text = serde_json::to_string(line).unwrap();
            let parsed = StatusLine::parse(&text).expect("a status line");
            let got = fold.apply(&parsed, claude_signal).map(|o| {
                serde_json::json!({
                    "state": o.state.as_str(),
                    "detail": o.detail,
                    "cause": o.cause.map(|c| c.as_str()),
                    "ts": o.ts,
                })
            });
            let want = (!want.is_null()).then(|| {
                serde_json::json!({
                    "state": want["state"],
                    "detail": want.get("detail"),
                    "cause": want.get("cause"),
                    "ts": line["ts"],
                })
            });
            assert_eq!(got, want, "{why}: line {i} {text}");
        }
    }
}
// jscpd:ignore-end

/// The tail reports transitions only, survives a partial line across two
/// polls, and treats a second `waiting` with a new question as news.
#[test]
fn tail_reports_transitions_and_handles_partial_lines() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(STATUS_FILE);
    let mut f = std::fs::File::create(&path).unwrap();
    let mut tail = Tail::new(path.clone(), claude_signal);
    assert!(tail.poll().is_empty());

    let line = |event: &str, tool: &str| {
        format!(
            r#"{{"event":"{event}","tool_name":{},"ts":"t"}}"#,
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
    let got: Vec<&str> = tail.poll().iter().map(|o| o.state.as_str()).collect();
    assert_eq!(got, vec!["working"], "three working lines, one transition");

    // A partial line: nothing until the newline lands.
    let stop = line("Stop", "");
    let (head, rest) = stop.split_at(10);
    write!(f, "{head}").unwrap();
    f.flush().unwrap();
    assert!(tail.poll().is_empty());
    writeln!(f, "{rest}").unwrap();
    f.flush().unwrap();
    let got: Vec<&str> = tail.poll().iter().map(|o| o.state.as_str()).collect();
    assert_eq!(got, vec!["done"]);

    // Two DIFFERENT questions in a row are two `waiting`s; the same
    // question re-asked is not a third.
    let ask = |q: &str| {
        format!(
            r#"{{"event":"PreToolUse","tool_name":"AskUserQuestion","question":"{q}","ts":"t"}}"#
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
        ("lib.rs (plan)", include_str!("../lib.rs")),
        ("interactive.rs (PTY)", include_str!("../interactive.rs")),
        ("headless.rs", include_str!("../headless.rs")),
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
    let tasks: String = include_str!("../tasks.rs").split_whitespace().collect();
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
        format!(r#"{{"event":"PermissionRequest","tool_name":"Bash","ts":"{tag}stale"}}"#) + "\n",
    )
    .unwrap();

    let watcher = Watcher::start(dir.path()).unwrap();
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
            r#""question":"which port?","#,
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

/// A status file that cannot be reset may hold an old `SessionStart`
/// line, which would prove a session whose hooks never ran.
#[test]
fn a_status_file_that_cannot_be_reset_fails_the_start() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(STATUS_FILE)).unwrap();
    let err = Watcher::start(dir.path()).err().expect("start refuses");
    assert!(format!("{err:#}").contains(STATUS_FILE), "{err:#}");
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
fn the_refusal_names_the_settings_file_with_one_kind_of_slash() {
    // A root with `/`, as `git rev-parse --show-toplevel` gives on Windows.
    let root = if cfg!(windows) { "C:/Dev/X" } else { "/tmp/x" };
    let settings = PathBuf::from(root)
        .join(".ralphy")
        .join("run")
        .join("ralphy.settings.json");
    let project = PathBuf::from(root);

    let err = no_session_start(&settings, &project, Duration::from_secs(1));

    let want = ralphy_proc_util::hook_form::native(&settings)
        .display()
        .to_string();
    assert!(err.cause.contains(&want), "{} names {want}", err.cause);
    if cfg!(windows) {
        assert!(!want.contains('/'), "{want}");
        assert!(!err.cause.contains("C:/Dev/X"), "{}", err.cause);
    }
}

#[test]
fn a_shell_form_session_is_refused_with_the_update_message() {
    let dir = tempfile::tempdir().unwrap();
    let settings = crate::settings::SettingsFile {
        path: dir.path().join("ralphy.settings.json"),
        unproven: Some("Claude Code 2.1.138 runs a hook through a shell".into()),
    };
    let Err(err) = proof_gate(&settings, dir.path().join(STATUS_FILE), dir.path(), false) else {
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

#[test]
fn a_refusal_gets_no_spawn_context() {
    let held = |e: anyhow::Error| e.context("the prompt was held and not sent");
    let ctx = "running the CLI (is it installed and on PATH?)";
    let refused = spawn_context(held(anyhow::Error::new(a_cause())), ctx);
    assert!(!format!("{refused:#}").contains("on PATH"), "{refused:#}");
    assert!(refused
        .downcast_ref::<ralphy_core::HooksUnproven>()
        .is_some());
    let failed = spawn_context(anyhow::anyhow!("not found"), ctx);
    assert!(format!("{failed:#}").starts_with(ctx), "{failed:#}");
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
    let mut fold = Fold::new();
    let ask = StatusLine::parse(
        r#"{"event":"PreToolUse","tool_name":"AskUserQuestion","tool_use_id":"q","ts":"t"}"#,
    )
    .unwrap();
    let ask = fold.apply(&ask, claude_signal).unwrap();
    assert_eq!(ask.state.as_str(), "waiting");
    let back = StatusLine::parse(
        r#"{"event":"PostToolUse","tool_name":"AskUserQuestion","tool_use_id":"q","ts":"t"}"#,
    )
    .unwrap();
    let back = fold.apply(&back, claude_signal).unwrap();
    assert_eq!((back.state.as_str(), back.detail), ("working", None));
}
