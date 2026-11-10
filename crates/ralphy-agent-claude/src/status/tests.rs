use super::*;

mod watcher;

use ralphy_adapter_support::agent_state::{Fold, Tail};
use std::sync::{Arc, Mutex};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

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
                    "cause": o.cause.map(agent_state::Cause::as_str),
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
    assert_eq!(tail.poll(), Vec::<agent_state::Observed>::new());

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
    assert_eq!(tail.poll(), Vec::<agent_state::Observed>::new());
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
        let watcher = bind.captures(code).map_or_else(
            || panic!("{name} must start a Watcher"),
            |c| c[1].to_string(),
        );
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
