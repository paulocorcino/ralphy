//! The tests of the OpenCode map and of a run's watcher.

use super::*;

use ralphy_adapter_support::agent_state::Fold;
use serde_json::Value;

/// The shared mapping fixture: every row is a sequence of status lines
/// and the state after each one. The daemon pins ITS fold against the
/// same file (ADR-0059 §5), so the two cannot drift.
const MAPPING: &str = include_str!("../../tests/fixtures/agent_state_mapping.json");

// The daemon has the same test against the same fixture (ADR-0059 §5).
#[test]
fn fold_matches_the_shared_mapping_fixture() {
    let rows: Vec<Value> = serde_json::from_str(MAPPING).unwrap();
    assert!(rows.len() >= 10, "the fixture covers the whole table");
    for row in rows {
        let why = row["why"].as_str().unwrap_or_default();
        let mut fold = Fold::new();
        let lines = row["lines"].as_array().expect("lines");
        let states = row["states"].as_array().expect("states");
        assert_eq!(lines.len(), states.len(), "{why}");
        for (i, (line, want)) in lines.iter().zip(states).enumerate() {
            let text = serde_json::to_string(line).unwrap();
            let parsed = StatusLine::parse(&text).expect("a status line");
            let got = fold.apply(&parsed, opencode_signal).map(|o| {
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

// ---- a run, end to end through `tracing` (ADR-0059 §3) -------------
//
// The watcher emits from its own thread, so the capture is a
// process-global subscriber: nextest runs each test in a process of its
// own, and this is the only test of the crate that installs one.
use std::sync::{Arc, Mutex};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

type Sink = Arc<Mutex<Vec<(String, String)>>>;

/// Keeps the `state` and `detail` of each `agent state` event.
struct Capture(Sink);

// The Codex adapter has the same capture layer for its own run test; a
// test helper shared by two adapter crates would need a crate of its own.
impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        #[derive(Default)]
        struct Fields {
            message: String,
            state: String,
            detail: String,
        }
        impl tracing::field::Visit for Fields {
            fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
                match f.name() {
                    "state" => self.state = v.into(),
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
            self.0
                .lock()
                .expect("sink")
                .push((fields.state, fields.detail));
        }
    }
}

/// A run whose child (the run hook, standing in for `opencode run` and
/// its plugin) appends a `permission.asked` line to the run's status
/// file emits one `agent state` event: `waiting`, with the permission.
/// The plugin file is in the run dir before the child starts.
#[test]
fn an_opencode_run_emits_the_agent_state_of_its_plugin() {
    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(Capture(sink.clone())),
    )
    .expect("this test installs the one global subscriber of its process");
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let ws = ralphy_core::Workspace::new(&repo);
    let run_dir = temp.path().join("run");
    let status_file = run_dir.join(STATUS_FILE);
    let plugin_file = run_dir.join(PLUGIN_FILE);
    let agent = crate::OpenCodeAgent::new(None, run_dir).with_run_hook(move || {
        use std::io::Write;
        anyhow::ensure!(plugin_file.is_file(), "the plugin is written first");
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&status_file)?;
        writeln!(
            f,
            r#"{{"event":"permission.asked","session_id":"ses_1","tool_name":"bash","tool_use_id":"per_1","ts":"t1"}}"#
        )?;
        Ok(ralphy_adapter_support::HeadlessRun {
            stdout: String::new(),
            log: String::new(),
            exited_cleanly: true,
            timed_out: false,
            idle_killed: false,
            stopped: false,
            exit_code: Some(0),
        })
    });
    let plan = ralphy_core::Plan {
        path: ws.plan_path(),
        open_steps: 1,
        recommended_model: None,
        usage: ralphy_core::Usage::default(),
        session_id: None,
    };
    ralphy_core::Agent::execute(&agent, &plan, &ws).expect("the execute ends");
    let got = sink.lock().expect("sink").clone();
    assert_eq!(
        got,
        [("waiting".to_string(), "permission: bash".to_string())]
    );
}
