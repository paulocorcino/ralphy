use super::*;
use std::sync::OnceLock;

// ---- the Watcher, end to end through `tracing` (ADR-0059 §3) --------
//
// A process-global capturing layer, installed once: the Watcher emits from
// ITS OWN thread, so a thread-local subscriber would never see it. Every
// `agent state` event lands in one shared sink; each test picks its own
// events out by the `since` it wrote into the lines.

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
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while mine(&sink, &tag).len() < 2 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
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
