use super::*;

fn reply(id: u64, status: &str) -> Vec<u8> {
    protocol::encode(&Frame::Command(Command {
        id,
        verb: "run".to_string(),
        payload: serde_json::json!({ "status": status }),
    }))
}

#[test]
fn only_the_exit_or_error_of_the_same_command_ends_a_peer_relay() {
    assert!(is_terminal_reply(&reply(3, "exited"), 3));
    assert!(is_terminal_reply(&reply(3, "error"), 3));
    assert!(
        !is_terminal_reply(&reply(4, "exited"), 3),
        "the exit of another command"
    );
    assert!(!is_terminal_reply(&reply(4, "error"), 3));
    assert!(!is_terminal_reply(&reply(3, "spawned"), 3));
    assert!(!is_terminal_reply(&reply(3, "output"), 3));
    assert!(!is_terminal_reply(&[0xFF, 0x00], 3), "undecodable bytes");
}

fn ctx(dir: &Path) -> CommandCtx {
    CommandCtx {
        registry_path: dir.join("registry.json"),
        peers_dir: dir.join("peers"),
        daemon_id: Some("here".to_string()),
        sessions: Arc::new(session::SessionManager::new()),
        secret_ok: true,
    }
}

fn nobody() -> SocketAudit {
    SocketAudit::of(Arc::new(crate::audit::Audit::in_dir(None)), None)
}

fn command(verb: &str) -> Command {
    Command {
        id: 7,
        verb: verb.to_string(),
        payload: serde_json::json!({ "repo": "demo", "agent": "claude" }),
    }
}

#[tokio::test]
async fn a_spawn_verb_is_answered_as_a_stream() {
    let dir = tempfile::tempdir().expect("a temp dir is writable");
    let answer = answer_one(&ctx(dir.path()), &command("run"), &nobody()).await;
    assert!(
        matches!(answer, Answer::Stream(dispatch::Verb::Run)),
        "{answer:?}"
    );
    let entries = std::fs::read_dir(dir.path())
        .expect("the temp dir reads")
        .count();
    assert_eq!(entries, 0, "nothing was written or spawned");
}

#[tokio::test]
async fn an_unknown_verb_is_answered_with_one_error_reply() {
    let dir = tempfile::tempdir().expect("a temp dir is writable");
    let answer = answer_one(&ctx(dir.path()), &command("no.such.verb"), &nobody()).await;
    match answer {
        Answer::Reply(reply) => assert_eq!(
            reply,
            serde_json::json!({ "status": "error", "message": "unknown verb" })
        ),
        Answer::Stream(verb) => panic!("streamed {verb:?}"),
    }
}

// ADR-0084 D2: the scroll probe is not a registry verb, so a build without
// the feature answers it as any name it does not know.
#[cfg(not(feature = "scroll-probe"))]
#[tokio::test]
async fn a_build_without_the_probe_answers_unknown_verb() {
    let dir = tempfile::tempdir().expect("a temp dir is writable");
    let mut cmd = command("probe.scroll");
    cmd.payload = serde_json::json!({ "records": [] });
    match answer_one(&ctx(dir.path()), &cmd, &nobody()).await {
        Answer::Reply(reply) => assert_eq!(
            reply,
            serde_json::json!({ "status": "error", "message": "unknown verb" })
        ),
        Answer::Stream(verb) => panic!("streamed {verb:?}"),
    }
}

// ADR-0084 D1: a release, a release candidate and CI never build the probe.
#[test]
fn no_workflow_turns_on_the_scroll_probe() {
    let workflows = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows");
    let mut read = 0;
    for entry in std::fs::read_dir(&workflows).expect("the workflows folder reads") {
        let path = entry.expect("a workflow entry").path();
        let text = std::fs::read_to_string(&path).expect("a workflow reads");
        assert!(
            !text.contains("scroll-probe"),
            "{} names the scroll probe",
            path.display()
        );
        read += 1;
    }
    assert!(read > 0, "no workflow was read");
}
