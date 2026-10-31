use super::*;

#[tokio::test]
async fn peer_spawn_verbs_stream_from_the_owning_daemon() {
    let peer_store = tempfile::tempdir().unwrap();
    let peer_repo = tempfile::tempdir().unwrap();
    std::fs::write(peer_repo.path().join("note.txt"), "peer-side").unwrap();
    let png = b"\x89PNG\r\n\x1a\npeer-fixture";
    std::fs::write(peer_repo.path().join("pixel.png"), png).unwrap();
    let mut large_png = vec![0_u8; ralphy_daemon::tree::MAX_IMAGE_BYTES as usize];
    large_png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    std::fs::write(peer_repo.path().join("large.png"), &large_png).unwrap();
    let runid = "01TESTRUNIDTESTRUNIDTE";
    let runstate = peer_repo.path().join(".ralphy").join("runstate");
    std::fs::create_dir_all(&runstate).unwrap();
    std::fs::write(
        runstate.join(format!("{runid}.json")),
        serde_json::to_vec(&serde_json::json!({
            "v": 1,
            "runid": runid,
            "pid": std::process::id(),
            "started_at": "2026-07-29T10:00:00-03:00"
        }))
        .unwrap(),
    )
    .unwrap();
    let peer_registry = peer_store.path().join("repos.toml");
    save_registry(&peer_registry, peer_repo.path());
    let (peer_port, _peer_task) = serve(
        identity(B_ID, "peer"),
        peer_registry,
        AuthState::fixed(
            AuthPolicy::Bearer("peer-tok".to_string()),
            SessionEpoch::in_memory_detached(),
        ),
    )
    .await;

    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    std::fs::write(local_repo.path().join("note.txt"), "local-side").unwrap();
    std::fs::write(local_repo.path().join("local-only.txt"), "local").unwrap();
    let local_registry = local_store.path().join("repos.toml");
    save_registry(&local_registry, local_repo.path());
    peer::write_descriptor(local_store.path(), &descriptor(B_ID, peer_port)).unwrap();
    let closed_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    peer::write_descriptor(local_store.path(), &descriptor(DEAD_ID, closed_port)).unwrap();
    let (local_port, _local_task) = serve(
        identity(A_ID, "local"),
        local_registry,
        AuthState::localhost(),
    )
    .await;

    let read = ask(
        local_port,
        1,
        "file.read",
        serde_json::json!({
            "repo": format!("{B_ID}/{SLUG}"),
            "path": "note.txt"
        }),
    )
    .await;
    assert_eq!(read["content"], "peer-side", "got {read}");
    assert_ne!(
        read["content"], "local-side",
        "the local registry must never answer for a peer ref"
    );

    let tree = ask(
        local_port,
        4,
        "tree.list",
        serde_json::json!({ "repo": format!("{B_ID}/{SLUG}"), "path": "" }),
    )
    .await;
    let names: Vec<&str> = tree["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(names.contains(&"note.txt"), "got {tree}");
    assert!(
        !names.contains(&"local-only.txt"),
        "a peer tree must not come from the local collision: {tree}"
    );

    let image = ask(
        local_port,
        5,
        "file.image",
        serde_json::json!({
            "repo": format!("{B_ID}/{SLUG}"),
            "path": "pixel.png"
        }),
    )
    .await;
    assert_eq!(image["mediaType"], "image/png", "got {image}");
    assert_eq!(
        data_encoding::BASE64
            .decode(image["base64"].as_str().unwrap().as_bytes())
            .unwrap(),
        png
    );
    let large_image = ask(
        local_port,
        14,
        "file.image",
        serde_json::json!({
            "repo": format!("{B_ID}/{SLUG}"),
            "path": "large.png"
        }),
    )
    .await;
    let decoded_large = data_encoding::BASE64
        .decode(large_image["base64"].as_str().unwrap().as_bytes())
        .unwrap();
    assert_eq!(decoded_large.len(), large_png.len());
    assert_eq!(&decoded_large[..8], b"\x89PNG\r\n\x1a\n");

    let runs = ask(
        local_port,
        6,
        "runs.list",
        serde_json::json!({ "repo": format!("{B_ID}/{SLUG}") }),
    )
    .await;
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1, "got {runs}");
    assert_eq!(runs["runs"][0]["runid"], runid, "got {runs}");

    let peer_ref = format!("{B_ID}/{SLUG}");
    let created = ask(
        local_port,
        7,
        "file.create",
        serde_json::json!({ "repo": peer_ref, "path": "new.txt", "dir": false }),
    )
    .await;
    assert_eq!(created["status"], "ok", "got {created}");
    assert!(peer_repo.path().join("new.txt").is_file());
    assert!(!local_repo.path().join("new.txt").exists());

    let written = ask(
        local_port,
        8,
        "file.write",
        serde_json::json!({
            "repo": peer_ref,
            "path": "new.txt",
            "content": "written-through-the-peer"
        }),
    )
    .await;
    assert_eq!(written["status"], "ok", "got {written}");
    assert_eq!(
        std::fs::read_to_string(peer_repo.path().join("new.txt")).unwrap(),
        "written-through-the-peer"
    );

    let renamed = ask(
        local_port,
        9,
        "file.rename",
        serde_json::json!({
            "repo": peer_ref,
            "path": "new.txt",
            "to": "renamed.txt"
        }),
    )
    .await;
    assert_eq!(renamed["status"], "ok", "got {renamed}");
    assert!(peer_repo.path().join("renamed.txt").is_file());
    assert!(!local_repo.path().join("renamed.txt").exists());

    let deleted = ask(
        local_port,
        10,
        "file.delete",
        serde_json::json!({ "repo": peer_ref, "path": "renamed.txt" }),
    )
    .await;
    assert_eq!(deleted["status"], "ok", "got {deleted}");
    assert!(!peer_repo.path().join("renamed.txt").exists());

    let escape_name = format!("escape-{B_ID}.txt");
    let escaped = ask(
        local_port,
        11,
        "file.write",
        serde_json::json!({
            "repo": peer_ref,
            "path": format!("../{escape_name}"),
            "content": "escaped"
        }),
    )
    .await;
    assert_eq!(escaped["status"], "error", "got {escaped}");
    assert_eq!(escaped["reason"], "refused", "got {escaped}");
    assert!(!peer_repo
        .path()
        .parent()
        .unwrap()
        .join(&escape_name)
        .exists());
    assert!(!local_repo
        .path()
        .parent()
        .unwrap()
        .join(&escape_name)
        .exists());

    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_EXIT_CODE", "1");
    for (id, verb, payload, argv) in [
        (
            20,
            "changes.list",
            serde_json::json!({ "repo": peer_ref }),
            "changes list --format json",
        ),
        (
            21,
            "changes.stage",
            serde_json::json!({ "repo": peer_ref, "paths": ["a.txt"] }),
            "changes stage --path=a.txt",
        ),
        (
            22,
            "changes.unstage",
            serde_json::json!({ "repo": peer_ref, "paths": ["a.txt"] }),
            "changes unstage --path=a.txt",
        ),
        (
            23,
            "changes.discard",
            serde_json::json!({ "repo": peer_ref, "paths": ["a.txt"] }),
            "changes discard --path=a.txt",
        ),
        (
            24,
            "changes.commit",
            serde_json::json!({ "repo": peer_ref, "message": "peer commit" }),
            "changes commit --message=peer commit",
        ),
    ] {
        let reply = ask(local_port, id, verb, payload).await;
        let message = reply["message"]
            .as_str()
            .unwrap_or_else(|| panic!("{verb} did not relay child output: {reply}"));
        assert!(
            ran_in(message, peer_repo.path()),
            "{verb} must execute in the peer repo: {message}"
        );
        assert!(
            !message.contains(&local_repo.path().display().to_string()),
            "{verb} must never execute in the local collision: {message}"
        );
        assert!(
            message.contains(argv),
            "{verb} must carry its own argv: {message}"
        );
    }
    let config_refused = ask(
        local_port,
        25,
        "config.set",
        serde_json::json!({
            "repo": peer_ref,
            "key": "verify.command",
            "value": "echo widened"
        }),
    )
    .await;
    assert_eq!(
        config_refused["message"], "invalid mutation options",
        "the owning daemon must retain the remote config boundary: {config_refused}"
    );

    std::env::set_var("RALPHY_TEST_EXIT_CODE", "0");
    let env_dump = peer_store.path().join("peer-command-env.txt");
    std::env::set_var("RALPHY_TEST_ENV_DUMP", &env_dump);
    for (id, verb, payload) in [
        (
            12,
            "run",
            serde_json::json!({
                "repo": format!("{B_ID}/{SLUG}"),
                "agent": "codex",
                "branchMode": "current"
            }),
        ),
        (
            13,
            "triage",
            serde_json::json!({ "repo": format!("{B_ID}/{SLUG}") }),
        ),
        (
            14,
            "push",
            serde_json::json!({ "repo": format!("{B_ID}/{SLUG}") }),
        ),
    ] {
        let replies = ask_all(local_port, id, verb, payload).await;
        let statuses = replies
            .iter()
            .filter_map(|reply| reply.payload["status"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(statuses.first(), Some(&"spawned"), "{verb}: {statuses:?}");
        assert_eq!(statuses.last(), Some(&"exited"), "{verb}: {statuses:?}");
        assert!(
            statuses[1..statuses.len() - 1]
                .iter()
                .all(|status| *status == "output"),
            "{verb}: {statuses:?}"
        );
        let output = replies
            .iter()
            .filter_map(|reply| reply.payload["chunk"].as_str())
            .collect::<String>();
        assert!(
            output.contains("dispatch-stdout-marker"),
            "{verb}: {output}"
        );
        assert!(
            output.contains("dispatch-stderr-marker"),
            "{verb}: {output}"
        );
        assert!(
            ran_in(&output, peer_repo.path()),
            "{verb} must execute in the peer repo: {output}"
        );
        assert!(
            !output.contains(&local_repo.path().display().to_string()),
            "{verb} must never execute in the local collision: {output}"
        );
        assert_eq!(
            replies.last().unwrap().payload["code"],
            0,
            "{verb}: {replies:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&env_dump).unwrap(),
            format!(
                "RALPHY_DAEMON_TOKEN=ABSENT\nRALPHY_DAEMON_ID={B_ID}\n\
                 RALPHY_TELEGRAM_TOKEN=ABSENT\nRALPHY_EVENTS_TOKEN=ABSENT"
            ),
            "{verb} must carry the owning daemon identity"
        );
    }
    std::env::remove_var("RALPHY_TEST_ENV_DUMP");

    let unreachable = ask(
        local_port,
        15,
        "file.read",
        serde_json::json!({
            "repo": format!("{DEAD_ID}/{SLUG}"),
            "path": "note.txt"
        }),
    )
    .await;
    let message = unreachable["message"].as_str().unwrap();
    assert!(message.contains(ENV), "got {unreachable}");
    assert!(
        !message.contains("serde") && !message.contains("decod"),
        "transport failure must not surface as a decoding error: {unreachable}"
    );

    // The switcher's pre-flight (#411/#412): a `tree.list` that names a
    // checkout is proxied with the name INTACT — only `repo` is rewritten to
    // the owner's slug — so the owning daemon resolves the worktree against
    // its own tree, and its `unknown checkout` for a vanished one comes back
    // verbatim, which is the placeholder's one trigger.
    let wt = peer_repo
        .path()
        .join(".ralphy")
        .join("worktrees")
        .join("wt-a");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(wt.join(".git"), "gitdir: elsewhere/.git/worktrees/wt-a\n").unwrap();
    std::fs::write(wt.join("only-in-wt.txt"), "wt").unwrap();
    let in_wt = ask(
        local_port,
        16,
        "tree.list",
        serde_json::json!({ "repo": peer_ref, "path": "", "checkout": "wt-a" }),
    )
    .await;
    let names: Vec<&str> = in_wt["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("got {in_wt}"))
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(
        names.contains(&"only-in-wt.txt") && !names.contains(&"note.txt"),
        "the checkout must be resolved on the peer's tree: {in_wt}"
    );
    let gone = ask(
        local_port,
        17,
        "tree.list",
        serde_json::json!({ "repo": peer_ref, "path": "", "checkout": "wt-gone" }),
    )
    .await;
    assert_eq!(gone["status"], "error", "got {gone}");
    assert_eq!(
        gone["message"],
        ralphy_daemon::checkout::UNKNOWN,
        "the peer's refusal must come back verbatim: {gone}"
    );
}

#[tokio::test]
async fn a_peer_that_ends_a_spawn_before_its_last_frame_is_one_error_reply() {
    // A stand-in peer: its handshake passes, and its page socket answers the
    // open with `channel.closed {reason: refused}` and no frame of the run.
    let opens = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = opens.clone();
    let stand_in = axum::Router::new()
        .route(
            "/api/peer/hello",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "daemon_id": B_ID,
                    "protocol_version": PEER_PROTOCOL_VERSION,
                }))
            }),
        )
        .route(
            "/ws",
            axum::routing::get(move |ws: axum::extract::ws::WebSocketUpgrade| {
                let seen = seen.clone();
                async move { ws.on_upgrade(move |socket| refuse_the_open(socket, seen)) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let peer_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, stand_in).await.unwrap();
    });

    let local_store = tempfile::tempdir().unwrap();
    let local_repo = tempfile::tempdir().unwrap();
    let local_registry = local_store.path().join("repos.toml");
    save_registry(&local_registry, local_repo.path());
    peer::write_descriptor(local_store.path(), &descriptor(B_ID, peer_port)).unwrap();
    let (local_port, _local_task) = serve(
        identity(A_ID, "local"),
        local_registry,
        AuthState::localhost(),
    )
    .await;

    let payload = serde_json::json!({ "repo": format!("{B_ID}/{SLUG}") });
    let replies = ask_until_the_end(local_port, 30, "triage", payload).await;
    assert_eq!(replies.len(), 1, "exactly one reply: {replies:?}");
    assert_eq!(replies[0].payload["status"], "error", "got {replies:?}");
    let message = replies[0].payload["message"].as_str().unwrap_or_default();
    assert!(message.contains(ENV), "the error names the peer: {message}");

    let opens = opens.lock().unwrap().clone();
    assert_eq!(opens.len(), 1, "the peer got one open: {opens:?}");
    assert_eq!(opens[0]["kind"], "spawn", "got {opens:?}");
    assert_eq!(opens[0]["command"]["verb"], "triage", "got {opens:?}");
    assert_eq!(
        opens[0]["command"]["payload"]["repo"], SLUG,
        "the peer gets its own slug: {opens:?}"
    );
}

/// Read the first frame of `socket` as a `channel.open`, keep its payload in
/// `seen`, and end that channel as refused.
async fn refuse_the_open(
    mut socket: axum::extract::ws::WebSocket,
    seen: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
) {
    use axum::extract::ws::Message as AxumMessage;
    let Some(Ok(AxumMessage::Binary(bytes))) = socket.recv().await else {
        return;
    };
    let Ok(Frame::Command(open)) = protocol::decode(&bytes) else {
        return;
    };
    assert_eq!(open.verb, "channel.open", "the first frame opens a channel");
    let channel = open.payload["channel"].clone();
    seen.lock()
        .expect("the stand-in peer's record of opens")
        .push(open.payload);
    let closed = Frame::Command(Command {
        id: 0,
        verb: "channel.closed".to_string(),
        payload: serde_json::json!({ "channel": channel, "reason": "refused" }),
    });
    socket
        .send(AxumMessage::Binary(protocol::encode(&closed).into()))
        .await
        .expect("the stand-in peer sends channel.closed");
    // Hold the socket until the local daemon closes it.
    while let Some(Ok(_)) = socket.recv().await {}
}
