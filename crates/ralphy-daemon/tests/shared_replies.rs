//! Replies the daemon itself produces, written to the shared fixtures the UI
//! tests fold (ADR-0070 Compliance). Query replies that the CLI produces are
//! written by the CLI tests; the daemon owns only the refusals it makes before
//! any child is spawned.

#[path = "support/golden.rs"]
mod golden;

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::{registry, router};
use tokio_tungstenite::tungstenite::Message;

/// One command round-trip on its own socket, returning the reply payload.
async fn ask(port: u16, id: u64, verb: &str, payload: serde_json::Value) -> serde_json::Value {
    let url = format!("ws://127.0.0.1:{port}/ws/command");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting to /ws/command");

    ws.send(Message::Binary(
        protocol::encode(&Frame::Command(Command {
            id,
            verb: verb.to_string(),
            payload,
        }))
        .into(),
    ))
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            let bytes = match msg.unwrap() {
                Message::Binary(b) => b,
                Message::Close(_) => break,
                _ => continue,
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.id == id {
                    return Some(cmd.payload);
                }
            }
        }
        None
    })
    .await
    .expect("a reply must arrive within 10s")
    .expect("a reply on the requesting id")
}

#[tokio::test]
async fn project_remove_of_an_unknown_repo_is_the_shared_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    registry::save_to(&registry::RegistryStore::default(), &registry_path).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let reply = ask(
        port,
        1,
        "project.remove",
        serde_json::json!({ "repo": "nope/gone", "slug": "nope/gone" }),
    )
    .await;
    assert_eq!(
        reply,
        serde_json::json!({ "status": "error", "message": "unknown repo" })
    );
    golden::check("project.remove--unknown-repo", reply, &[]);
}

/// The desk history list (ADR-0050 amendment 2026-10-04): three versions with
/// fixed ids, so the reply has no volatile field.
#[tokio::test]
async fn the_desk_history_list_is_the_shared_reply() {
    use http_body_util::BodyExt;
    use ralphy_daemon::desk::{self, history};
    use tower::ServiceExt;

    let dir = tempfile::tempdir().unwrap();
    let history_dir = dir.path().join("desk-history");
    let window = |left: f64| desk::DeskRecord {
        id: "w1".into(),
        repo: "owner/repo".into(),
        agent: "claude".into(),
        kind: "agent".into(),
        rect: desk::DeskRect {
            left,
            top: 20.0,
            width: 640.0,
            height: 480.0,
        },
        ..Default::default()
    };
    let one = desk::DeskStore {
        windows: vec![window(10.0)],
        ..Default::default()
    };
    let two = desk::DeskStore {
        windows: vec![
            window(10.0),
            desk::DeskRecord {
                id: "w2".into(),
                ..window(700.0)
            },
        ],
        fences: vec![desk::DeskFence {
            id: "f1".into(),
            name: "backend".into(),
            rect: window(0.0).rect,
            ..Default::default()
        }],
        ..Default::default()
    };
    history::append(
        &history_dir,
        &one,
        history::Reason::Change,
        1_790_000_000_000,
    )
    .unwrap();
    history::append(
        &history_dir,
        &two,
        history::Reason::BeforeRestore,
        1_790_000_100_000,
    )
    .unwrap();
    history::append(
        &history_dir,
        &one,
        history::Reason::Restore,
        1_790_000_100_000,
    )
    .unwrap();

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        dir.path().join("repos.toml"),
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        ralphy_daemon::auth::AuthState::localhost(),
    );
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/desk/history")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let reply: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reply.as_array().map(Vec::len), Some(3));
    golden::check("api-desk-history", reply, &[]);
}
