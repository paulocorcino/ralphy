//! The host verbs on `/ws/command` (ADR-0036 amendment 2026-09-29): served with
//! no `repo`, refused on a bad value before anything spawns, and never answered
//! for a peer. The spawned exe is `command_test_child`, which echoes its argv.

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, Request};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::router;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

fn command(id: u64, verb: &str, payload: serde_json::Value) -> Message {
    Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id,
            verb: verb.to_string(),
            payload,
        }))
        .into(),
    )
}

fn app(registry_path: std::path::PathBuf) -> axum::Router {
    let (tx, rx) = tokio::sync::watch::channel(false);
    std::mem::forget(tx);
    router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    )
}

/// Every reply frame to one command, up to a terminal `exited` or `error`.
async fn replies(port: u16, verb: &str, payload: serde_json::Value) -> Vec<serde_json::Value> {
    replies_with(port, verb, payload, &[]).await
}

/// [`replies`], with extra headers on the upgrade request.
async fn replies_with(
    port: u16,
    verb: &str,
    payload: serde_json::Value,
    headers: &[(&str, &str)],
) -> Vec<serde_json::Value> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let url = format!("ws://127.0.0.1:{port}/ws/command");
    let mut request = url.into_client_request().expect("a websocket request");
    for (name, value) in headers {
        request.headers_mut().insert(
            header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    let (mut ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connecting to /ws/command");
    ws.send(command(1, verb, payload)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut frames = Vec::new();
        while let Some(msg) = ws.next().await {
            let Ok(Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Command(reply)) = protocol::decode(&bytes) {
                let status = reply.payload["status"].as_str().unwrap_or("").to_string();
                frames.push(reply.payload);
                if status == "exited" || status == "error" {
                    break;
                }
            }
        }
        frames
    })
    .await
    .expect("a terminal frame within 10s")
}

// One test: the exe override is process-global (the pattern of `command_ws.rs`).
#[tokio::test]
async fn host_verbs_run_locally_with_no_repo() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    std::env::set_var(
        "RALPHY_EXE_OVERRIDE",
        env!("CARGO_BIN_EXE_command_test_child"),
    );
    std::env::set_var("RALPHY_TEST_READ_STDIN", "1");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let served = app(registry_path.clone());
    tokio::spawn(async move {
        axum::serve(listener, served).await.unwrap();
    });

    // (a) No `repo`, no registry: the check still runs, streamed.
    let frames = replies(
        port,
        "host.check",
        serde_json::json!({"destination": "svrapp"}),
    )
    .await;
    let statuses: Vec<&str> = frames
        .iter()
        .map(|f| f["status"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(statuses.first(), Some(&"spawned"), "{frames:?}");
    assert_eq!(statuses.last(), Some(&"exited"), "{frames:?}");
    let output: String = frames.iter().filter_map(|f| f["chunk"].as_str()).collect();
    assert!(
        output.contains("dispatch-argv: host check svrapp --json"),
        "{output:?}"
    );
    assert_eq!(frames.last().unwrap()["code"], 0, "{frames:?}");
    assert!(
        !format!("{frames:?}").contains("unknown repo"),
        "{frames:?}"
    );

    // (b) An option in the destination: refused, nothing spawned.
    let frames = replies(
        port,
        "host.add",
        serde_json::json!({"destination": "-oProxyCommand=x"}),
    )
    .await;
    assert_eq!(
        frames,
        [serde_json::json!({"status": "error", "message": "invalid host options"})]
    );

    // (b2) A Mutate answers ok on exit 0; a Query whose last line is not
    // JSON is an error that carries the output (the test child prints text).
    let frames = replies(
        port,
        "host.trust",
        serde_json::json!({"destination": "svrapp", "fingerprint": format!("SHA256:{}", "A".repeat(43))}),
    )
    .await;
    assert_eq!(frames, [serde_json::json!({"status": "ok"})]);
    let frames = replies(port, "host.aliases", serde_json::json!({})).await;
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0]["status"], "error");
    assert!(
        frames[0]["message"]
            .as_str()
            .unwrap_or("")
            .contains("dispatch-argv: host aliases"),
        "{frames:?}"
    );

    // (b3) A password goes to the child's standard input, never its argv, when
    // the request comes from this computer or over https.
    let with_password = serde_json::json!({"destination": "svrapp", "password": "s3 cret"});
    for headers in [
        &[][..],
        &[
            ("x-forwarded-proto", "https"),
            ("x-forwarded-for", "10.0.0.9"),
        ][..],
    ] {
        let frames = replies_with(port, "host.add", with_password.clone(), headers).await;
        let output: String = frames.iter().filter_map(|f| f["chunk"].as_str()).collect();
        assert!(
            output.contains("dispatch-argv: host add svrapp --json --password-stdin\n")
                || output.contains("dispatch-argv: host add svrapp --json --password-stdin\r\n"),
            "{headers:?}: {output:?}"
        );
        assert!(
            output.contains("dispatch-stdin: s3 cret"),
            "{headers:?}: {output:?}"
        );
    }
    // Over plain http through a front, the password is refused and nothing
    // spawns.
    let frames = replies_with(
        port,
        "host.add",
        with_password,
        &[("x-forwarded-for", "10.0.0.9")],
    )
    .await;
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0]["status"], "error");
    assert!(
        frames[0]["message"]
            .as_str()
            .unwrap_or("")
            .contains("https"),
        "{frames:?}"
    );

    // (c) A peer asking this daemon for a host verb is refused.
    let body = serde_json::json!({"id": 1, "verb": "host.aliases", "payload": {}});
    let resp = app(registry_path)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/peer/command")
                .header(header::HOST, format!("127.0.0.1:{port}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("the router must answer");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let reply: serde_json::Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&bytes)));
    assert_eq!(
        reply["message"],
        "a host command runs only on the daemon you are connected to"
    );
}
