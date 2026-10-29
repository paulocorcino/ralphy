use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::*;

/// The daemon's clock in these tests.
fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
}

fn rfc3339(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A request of `kind` with id `id`, opened at `since`.
fn request(kind: Kind, id: &str, since: SystemTime) -> AttentionRequest {
    let console = matches!(
        kind,
        Kind::ConsoleWaiting | Kind::ConsoleSilent | Kind::ConsoleDone
    );
    AttentionRequest {
        id: id.to_string(),
        kind,
        vendor: "claude".to_string(),
        console: console.then(|| "7".to_string()),
        name: console.then(|| "fix login".to_string()),
        repo: Some("owner/app".to_string()),
        run: (!console).then(|| "01RUN".to_string()),
        issue: None,
        since: rfc3339(since),
        detail: Some("permission: Bash".to_string()),
        dismissable: kind.dismissable(),
    }
}

fn waiting(since: SystemTime) -> AttentionRequest {
    request(Kind::ConsoleWaiting, "console:7", since)
}

/// A transport that records each `sendMessage` body, and fails while `fail`
/// is set.
#[derive(Clone, Default)]
struct Fake {
    sent: Arc<Mutex<Vec<Value>>>,
    fail: Arc<AtomicBool>,
}

impl Transport for Fake {
    fn get(&self, method: &str) -> Result<Value> {
        bail!("the sender calls no GET, got {method}")
    }

    fn post(&self, method: &str, body: Value) -> Result<Value> {
        assert_eq!(method, "sendMessage");
        if self.fail.load(Ordering::SeqCst) {
            bail!("sendMessage request failed: the network is down");
        }
        self.sent.lock().expect("the fake's lock").push(body);
        Ok(json!({ "ok": true, "result": { "message_id": 1 } }))
    }
}

impl Fake {
    fn count(&self) -> usize {
        self.sent.lock().expect("the fake's lock").len()
    }
}

/// A config with a bot, in a temp dir.
fn config() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "token = \"123:abc\"\nchat_id = 42\n").expect("write the config");
    (dir, path)
}

/// One tick of the sender at `now`, as the attention tick runs it: the
/// messages due, their send, and what it did. The answer is the warnings.
fn tick(
    sender: &mut Sender,
    config: &Path,
    fake: &Fake,
    requests: &[AttentionRequest],
    now: SystemTime,
) -> Vec<String> {
    let messages = sender.next(requests, false, now, &Context::default());
    if messages.is_empty() {
        return Vec::new();
    }
    let fake = fake.clone();
    sender.finish(deliver(config, move |_token| fake, messages))
}

#[test]
fn a_console_request_is_sent_after_two_minutes() {
    let (_dir, path) = config();
    let fake = Fake::default();
    let mut sender = Sender::default();
    tick(&mut sender, &path, &fake, &[waiting(at(0))], at(119));
    assert_eq!(fake.count(), 0, "open 119 s: no message");

    tick(&mut sender, &path, &fake, &[waiting(at(0))], at(121));
    assert_eq!(fake.count(), 1, "open 121 s: one message");
    assert_eq!(fake.sent.lock().expect("the fake's lock")[0]["chat_id"], 42);

    let present = Sender::default().next(&[waiting(at(0))], true, at(600), &Context::default());
    assert!(present.is_empty(), "an operator at a page gets no message");
}

#[test]
fn an_answer_within_two_minutes_sends_nothing() {
    let (_dir, path) = config();
    let fake = Fake::default();
    let mut sender = Sender::default();
    for secs in (0..118).step_by(3) {
        tick(&mut sender, &path, &fake, &[waiting(at(0))], at(secs));
    }
    // The operator answered: the request left the list.
    for secs in (118..600).step_by(3) {
        tick(&mut sender, &path, &fake, &[], at(secs));
    }
    assert_eq!(fake.count(), 0);
}

#[test]
fn a_long_wait_is_one_message() {
    let (_dir, path) = config();
    let fake = Fake::default();
    let mut sender = Sender::default();
    for n in 0..3600 {
        tick(&mut sender, &path, &fake, &[waiting(at(0))], at(n * 3));
    }
    assert_eq!(fake.count(), 1, "three hours of waiting is one message");
}

/// A console's request id is always `console:<session id>`, so the second
/// wait of a console has the id of the first.
#[test]
fn a_console_that_waits_again_is_sent_again() {
    let (_dir, path) = config();
    let fake = Fake::default();
    let mut sender = Sender::default();
    tick(&mut sender, &path, &fake, &[waiting(at(0))], at(121));
    assert_eq!(fake.count(), 1, "the first wait");

    // The operator answered: the request left the list.
    tick(&mut sender, &path, &fake, &[], at(130));
    assert_eq!(fake.count(), 1);

    tick(&mut sender, &path, &fake, &[waiting(at(200))], at(300));
    assert_eq!(fake.count(), 1, "open 100 s: no message yet");
    tick(&mut sender, &path, &fake, &[waiting(at(200))], at(321));
    assert_eq!(fake.count(), 2, "the second wait, past 120 s, is sent");
}

#[test]
fn a_daemon_with_no_config_sends_nothing() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let fake = Fake::default();
    for path in [dir.path().join("config.toml"), PathBuf::new()] {
        let mut sender = Sender::default();
        let warned = tick(&mut sender, &path, &fake, &[waiting(at(0))], at(600));
        assert!(warned.is_empty(), "no config is no error: {warned:?}");
        assert_eq!(
            telegram_state::state_at(&path),
            telegram_state::TelegramState::Off
        );
    }
    assert_eq!(fake.count(), 0);
}

#[test]
fn a_failed_send_is_tried_again() {
    let (_dir, path) = config();
    let fake = Fake::default();
    fake.fail.store(true, Ordering::SeqCst);
    let mut sender = Sender::default();
    let requests = [waiting(at(0))];
    let first = tick(&mut sender, &path, &fake, &requests, at(130));
    let second = tick(&mut sender, &path, &fake, &requests, at(133));
    assert_eq!(first.len(), 1, "one warning: {first:?}");
    assert!(second.is_empty(), "the same failure is not logged again");
    assert_eq!(fake.count(), 0);

    fake.fail.store(false, Ordering::SeqCst);
    let third = tick(&mut sender, &path, &fake, &requests, at(136));
    assert!(third.is_empty());
    assert_eq!(fake.count(), 1, "the next tick sends it");
}

#[test]
fn silent_consoles_and_runs_send_nothing() {
    let requests = [
        request(Kind::ConsoleSilent, "console:7", at(0)),
        request(Kind::RunWaiting, "run:01RUN", at(0)),
    ];
    let due = due(
        &requests,
        &HashSet::new(),
        false,
        at(600),
        &Context::default(),
    );
    assert!(due.is_empty(), "got: {due:?}");
}

#[test]
fn the_message_links_to_the_console() {
    let ctx = Context {
        daemon_id: Some("01DAEMON".to_string()),
        origin: Some("https://ralphy.example".to_string()),
    };
    let requests = [
        waiting(at(0)),
        AttentionRequest {
            detail: None,
            ..request(Kind::ConsoleDone, "done:7:1", at(0))
        },
    ];
    let texts: Vec<String> = due(&requests, &HashSet::new(), false, at(600), &ctx)
        .into_iter()
        .map(|m| m.text)
        .collect();
    assert_eq!(
        texts,
        [
            "Claude console \"fix login\" in owner/app is waiting: permission: Bash\n\
             https://ralphy.example/?console=01DAEMON/7",
            "Claude console \"fix login\" in owner/app has finished its turn\n\
             https://ralphy.example/?console=01DAEMON/7",
        ]
    );

    // With no page seen yet, there is no origin and no link.
    let no_origin = Context {
        origin: None,
        ..ctx
    };
    let text = &due(&requests[..1], &HashSet::new(), false, at(600), &no_origin)[0].text;
    assert_eq!(
        text,
        "Claude console \"fix login\" in owner/app is waiting: permission: Bash"
    );
}
