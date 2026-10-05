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

/// The Devices section's two reads (ADR-0074 D9), over a log that one
/// measured Android phone wrote: its device facts, then one change.
#[tokio::test]
async fn the_audit_log_reads_are_the_shared_replies() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let dir = tempfile::tempdir().unwrap();
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
    let send = |req: axum::http::Request<axum::body::Body>| {
        let app = app.clone();
        async move { app.oneshot(req).await.unwrap() }
    };
    let json_of = |res: axum::http::Response<axum::body::Body>| async move {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
    };

    let first = send(
        axum::http::Request::builder()
            .uri("/api/session")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    let cookie = first
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("ralphy_device="))
        .expect("a device cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let device = cookie.split('.').nth(1).unwrap().to_string();

    let measured: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/devices/android-chrome.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut facts = axum::http::Request::builder()
        .method("POST")
        .uri("/api/device/facts")
        .header("content-type", "application/json")
        .header("cookie", &cookie);
    for (k, v) in measured["headers"].as_object().unwrap() {
        if ![
            "host",
            "origin",
            "referer",
            "content-length",
            "content-type",
        ]
        .contains(&k.as_str())
        {
            facts = facts.header(k.as_str(), v.as_str().unwrap());
        }
    }
    let res = send(
        facts
            .body(axum::body::Body::from(measured["client"].to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), axum::http::StatusCode::NO_CONTENT);
    send(
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/sessions/close?id=7&repo=owner/repo")
            .header("cookie", &cookie)
            .header("x-real-ip", "203.0.113.10")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    send(
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/peer/command")
            .header("content-type", "application/json")
            .header("cookie", &cookie)
            .header("x-real-ip", "203.0.113.10")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "id": 1,
                    "verb": "branch.switch",
                    "payload": {"repo": "nope", "name": "main"},
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;

    let devices = json_of(
        send(
            axum::http::Request::builder()
                .uri("/api/audit/devices")
                .header("cookie", &cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(devices["devices"][0]["device"], device.as_str());
    golden::check(
        "api-audit-devices",
        devices,
        &[
            "/devices/0/device",
            "/devices/0/first_seen",
            "/devices/0/last_seen",
        ],
    );

    let events = json_of(
        send(
            axum::http::Request::builder()
                .uri(format!("/api/audit/events?device={device}"))
                .header("cookie", &cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    golden::check(
        "api-audit-events",
        events,
        &[
            "/events/0/at",
            "/events/0/device",
            "/events/1/at",
            "/events/1/device",
            "/events/2/at",
            "/events/2/device",
        ],
    );
}
