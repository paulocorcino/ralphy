use super::*;
use ralphy_proc_util::hook_form::{hook_for_version, HookCheck};
use serde_json::Value;

#[test]
fn the_start_check_runs_in_the_background_and_logs_one_line() {
    let serve: String = include_str!("../serve.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace(",)", ")");
    let call = "spawn_blocking(crate::agent_state::run_start_check)";
    let start = serve
        .find(call)
        .expect("serve starts the check with spawn_blocking");
    let statement = serve[start..].split(';').next().unwrap_or("");
    assert!(
        !statement.contains(".await"),
        "the check is not awaited: {statement}"
    );

    let home = tempfile::tempdir().expect("temp dir");
    let missing = home.path().join("no-such-ralphy");
    let hook = hook_for_version(Some("2.1.139 (Claude Code)"), &missing, &["hook", "status"]);
    let log_path = home.path().join("daemon.log");
    let log_file = std::fs::File::create(&log_path).expect("create the log");
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || log_file.try_clone().expect("a second log handle"))
        .with_ansi(false)
        .finish();
    let got =
        tracing::subscriber::with_default(subscriber, || start_check(&hook, Some(home.path())));
    assert!(matches!(got, HookCheck::Failed { .. }), "{got:?}");
    let log = std::fs::read_to_string(&log_path).expect("read the log");
    let warnings = log.lines().filter(|l| l.contains("WARN")).count();
    assert_eq!(warnings, 1, "{log}");
    assert!(log.contains("no-such-ralphy"), "{log}");
}

/// §6: only a `working` goes stale, and only past the window.
#[test]
fn render_ages_working_into_unknown_and_nothing_else() {
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let obs = |state: &'static str| Observed {
        state,
        detail: None,
        since: "t".into(),
        seen: t0,
        cause: None,
    };
    let fresh = t0 + STALE_AFTER - Duration::from_secs(1);
    let old = t0 + STALE_AFTER + Duration::from_secs(1);
    assert_eq!(
        render(&obs("working"), fresh).state.as_deref(),
        Some("working")
    );
    assert_eq!(
        render(&obs("working"), old).state.as_deref(),
        Some("unknown")
    );
    assert_eq!(
        render(&obs("waiting"), old).state.as_deref(),
        Some("waiting")
    );
    assert_eq!(render(&obs("done"), old).state.as_deref(), Some("done"));
    // A clock that went backwards is not an age.
    assert_eq!(
        render(&obs("working"), t0 - Duration::from_secs(5))
            .state
            .as_deref(),
        Some("working")
    );
}

/// The silence rule, one row per case.
#[test]
fn the_silence_rule_decides_each_case() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let ago = |s: u64| Some(now - Duration::from_secs(s));
    let hooks = StateSource::Hooks;
    assert_eq!(silence(hooks, None, None, None, now), None, "no input");
    assert_eq!(silence(hooks, ago(29), None, None, now), None, "29 s");
    assert_eq!(
        silence(hooks, ago(31), None, None, now).as_deref(),
        Some(CLOCK_REASON),
        "31 s"
    );
    assert_eq!(silence(hooks, ago(31), ago(1), None, now), None, "a line");
    let start = silence(hooks, None, None, Some("boom: not found"), now)
        .expect("a failed start check is silent at once");
    assert_eq!(
        start,
        "The hook command failed when the daemon started: boom: not found"
    );
    assert_eq!(
        silence(StateSource::None, ago(31), None, None, now),
        None,
        "no hooks, no silence"
    );
}

/// A write the terminal sends by itself is not input; a key is.
#[test]
fn terminal_reports_are_told_from_keys() {
    for (bytes, report) in [
        (&b"\x1b[12;40R"[..], true),
        (b"\x1b[?1;2c", true),
        (b"\x1b[?1;2c\x1b[12;40R", true),
        (b"\x1b[I", true),
        (b"hello", false),
        (b"\x1b[B\r", false),
        (b"\x1b[B", false),
        (b"\x1b[12;40", false),
        (b"", false),
    ] {
        assert_eq!(is_terminal_report(bytes), report, "{bytes:?}");
    }
}

/// Transitions only — but every folded line is ACTIVITY (§6's clock), and
/// a `waiting` repeats only with a new detail.
#[test]
fn tail_reports_transitions_and_activity_separately() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    let (mut tail, mut fold) = (Tail::new(path), Fold::new());
    let mut poll =
        || HookVendor::Claude.fold_lines(&mut fold, &tail.read_lines(), SystemTime::now());
    let line = |event: &str| {
        format!(r#"{{"event":"{event}","tool_name":"Bash","tool_input":null,"ts":"t"}}"#)
    };
    writeln!(f, "{}", line("UserPromptSubmit")).unwrap();
    writeln!(f, "{}", line("PreToolUse")).unwrap();
    writeln!(f, "{}", line("Stop")).unwrap();
    f.flush().unwrap();
    let polled = poll();
    let got: Vec<&str> = polled.transitions.iter().map(|o| o.state).collect();
    assert_eq!(got, vec!["working", "done"]);
    assert!(polled.activity);
    let polled = poll();
    assert!(
        polled.transitions.is_empty() && !polled.activity,
        "nothing new"
    );

    // A repeated `working` is activity without a transition.
    writeln!(f, "{}", line("UserPromptSubmit")).unwrap();
    writeln!(f, "{}", line("PreToolUse")).unwrap();
    f.flush().unwrap();
    let polled = poll();
    assert_eq!(polled.transitions.len(), 1, "one working transition");
    writeln!(f, "{}", line("PreToolUse")).unwrap();
    f.flush().unwrap();
    let polled = poll();
    assert!(
        polled.transitions.is_empty() && polled.activity,
        "{polled:?}"
    );

    // The same permission twice is one waiting; a different tool is news.
    let perm = |tool: &str| {
        format!(
            r#"{{"event":"PermissionRequest","tool_name":"{tool}","tool_input":null,"ts":"t"}}"#
        )
    };
    writeln!(f, "{}", perm("Bash")).unwrap();
    writeln!(f, "{}", perm("Bash")).unwrap();
    writeln!(f, "{}", perm("Edit")).unwrap();
    f.flush().unwrap();
    let details: Vec<String> = poll()
        .transitions
        .into_iter()
        .map(|o| o.detail.unwrap_or_default())
        .collect();
    assert_eq!(details, vec!["permission: Bash", "permission: Edit"]);
}

const EXE: &str = if cfg!(windows) {
    r"C:\r\ralphy.exe"
} else {
    "/r/ralphy"
};

fn hook_of(version: &str) -> HookCommand {
    hook_for_version(Some(version), Path::new(EXE), &["hook", "status"])
}

/// The console settings carry the adapter's shared event list, with its
/// matchers, and NOTHING but hooks: the operator's own settings must keep
/// their say.
#[test]
fn console_settings_match_the_shared_event_list() {
    let shared: Vec<Value> = serde_json::from_str(include_str!(
        "../../../ralphy-agent-claude/tests/fixtures/hook_events.json"
    ))
    .unwrap();
    let mut want: Vec<(String, String)> = shared
        .iter()
        .map(|e| {
            (
                e["event"].as_str().unwrap().to_owned(),
                e["matcher"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    want.sort();
    let hook = hook_of("2.1.139");
    let json = console_settings_json(&hook);
    let v: Value = serde_json::from_str(&json).unwrap();
    let obj = v.as_object().unwrap();
    assert_eq!(obj.keys().collect::<Vec<_>>(), vec!["hooks"]);
    let hooks = v["hooks"].as_object().unwrap();
    let mut got: Vec<(String, String)> = hooks
        .iter()
        .map(|(event, list)| {
            assert_eq!(list.as_array().map(Vec::len), Some(1), "{event}");
            assert_eq!(list[0]["hooks"][0], hook.entry(), "{event}");
            (
                event.clone(),
                list[0]["matcher"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(got, want);
    for event in ["Elicitation", "ElicitationResult"] {
        assert!(hooks.contains_key(event), "{event}");
    }
}

/// Below 2.1.139 the entry is one shell string; from it, the program and
/// its arguments apart.
#[test]
fn console_settings_use_the_form_of_the_claude_version() {
    let old: Value = serde_json::from_str(&console_settings_json(&hook_of("2.1.138"))).unwrap();
    let shell = old["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(shell.ends_with(" hook status"), "{shell}");
    assert!(old["hooks"]["Stop"][0]["hooks"][0]["args"].is_null());

    let new: Value = serde_json::from_str(&console_settings_json(&hook_of("2.1.139"))).unwrap();
    assert_eq!(
        new["hooks"]["Stop"][0]["hooks"][0],
        serde_json::json!({"type": "command", "command": EXE, "args": ["hook", "status"]})
    );
}

/// A daemon started through a canonicalized path runs as `\\?\C:\...`;
/// Git Bash, which runs the hooks, cannot run that form.
#[cfg(windows)]
#[test]
fn console_hooks_name_the_binary_without_the_verbatim_prefix() {
    let hook = hook_for_version(
        Some("2.1.139"),
        Path::new(r"\\?\C:\r\ralphy.exe"),
        &["hook", "status"],
    );
    let doc: Value = serde_json::from_str(&console_settings_json(&hook)).unwrap();
    assert_eq!(
        doc["hooks"]["Stop"][0]["hooks"][0]["command"],
        r"C:\r\ralphy.exe"
    );
}

/// Claude and Codex have hooks, `OpenCode` has its state plugin; no other
/// agent gives a state. The match names every variant, so a new agent
/// has to choose.
#[test]
fn every_agent_has_a_state_source() {
    for agent in Agent::ALL {
        let want = if matches!(agent, Agent::Claude | Agent::Codex | Agent::OpenCode) {
            StateSource::Hooks
        } else {
            StateSource::None
        };
        assert_eq!(state_source(agent), want, "{agent:?}");
        assert_eq!(
            HookVendor::of(agent).is_some(),
            want == StateSource::Hooks,
            "{agent:?}: a source of hooks has a vendor map"
        );
    }
    let src = include_str!("../agent_state.rs");
    let start = src.find("pub fn state_source(").unwrap();
    let end = start + src[start..].find("\n}").unwrap();
    let body = &src[start..end];
    assert!(!body.contains("_ =>"), "the match must name every agent");
}

#[test]
fn status_files_are_written_under_the_sessions_dir_and_removed_together() {
    let dir = tempfile::tempdir().unwrap();
    let files = StatusFiles::for_session(&dir.path().join("sessions"), 7);
    let settings = files.settings.clone().expect("a Claude settings file");
    assert!(settings.ends_with("7.settings.json"));
    assert!(files.status.ends_with("7.agent-status.jsonl"));
    files.write(&hook_of("2.1.139")).unwrap();
    assert!(settings.is_file() && files.status.is_file());
    assert_eq!(std::fs::read(&files.status).unwrap(), b"");
    files.remove();
    assert!(!settings.exists() && !files.status.exists());
    // A Codex console has its status file only.
    let codex = StatusFiles::for_agent(Agent::Codex, &dir.path().join("sessions"), 8)
        .expect("Codex has hooks");
    assert_eq!(codex.settings, None);
    codex.create_status().unwrap();
    assert!(codex.status.ends_with("8.agent-status.jsonl") && codex.status.is_file());
    codex.remove();
    assert!(!codex.status.exists());
    // An OpenCode console has its status file and its state plugin.
    let opencode = StatusFiles::for_agent(Agent::OpenCode, &dir.path().join("sessions"), 9)
        .expect("OpenCode has a plugin");
    assert_eq!(opencode.settings, None);
    let plugin = opencode.plugin.clone().expect("a plugin file");
    assert!(plugin.ends_with("9.state-plugin.js"), "{plugin:?}");
    opencode.create_status().unwrap();
    assert_eq!(
        std::fs::read_to_string(&plugin).unwrap(),
        opencode::STATE_PLUGIN
    );
    assert!(opencode.status.is_file());
    opencode.remove();
    assert!(!plugin.exists() && !opencode.status.exists());
}

/// An `OpenCode` console whose plugin wrote no line in the 31 s after the
/// operator's first input is silent; the plugin's first line alone,
/// which changes no state, is enough to show that it runs (ADR-0085 D9).
#[test]
fn an_opencode_console_with_no_line_is_silent() {
    use std::io::Write;
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let first_input = Some(now - Duration::from_secs(31));
    let source = state_source(Agent::OpenCode);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    std::fs::write(&path, b"").unwrap();
    let (mut tail, mut fold) = (Tail::new(path.clone()), Fold::new());
    let mut poll = || HookVendor::OpenCode.fold_lines(&mut fold, &tail.read_lines(), now);
    let first_line = poll().activity.then_some(now);
    assert_eq!(
        silence(source, first_input, first_line, None, now).as_deref(),
        Some(CLOCK_REASON),
        "no line at all"
    );

    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(f, r#"{{"event":"plugin.loaded","ts":"t0"}}"#).unwrap();
    f.flush().unwrap();
    let polled = poll();
    assert!(polled.transitions.is_empty(), "{polled:?}");
    let first_line = polled.activity.then_some(now);
    assert_eq!(
        silence(source, first_input, first_line, None, now),
        None,
        "the plugin's first line"
    );
}
