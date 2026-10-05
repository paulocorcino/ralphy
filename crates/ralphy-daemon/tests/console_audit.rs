//! A console socket that starts a session writes a `console_launch` line in
//! the audit log, and a take-over writes a `console_takeover` line; a plain
//! reattach writes none (ADR-0074, amendment 2026-10-05).

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use ralphy_daemon::auth::AuthState;
use ralphy_daemon::{registry, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(dir: &std::path::Path) -> (u16, tokio::sync::watch::Sender<bool>) {
    static OVERRIDE: std::sync::Once = std::sync::Once::new();
    OVERRIDE.call_once(|| {
        std::env::set_var(
            "RALPHY_DAEMON_AGENT_OVERRIDE",
            env!("CARGO_BIN_EXE_session_test_child"),
        );
    });
    let registry_path = dir.join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("owner/workbench", &dir.to_string_lossy());
    registry::save_to(&store, &registry_path).expect("writing the registry");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        std::path::PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        AuthState::localhost(),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, tx)
}

async fn http(port: u16, method: &str, path: &str, cookie: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {cookie}\r\n\
         Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await.unwrap();
    reply
}

/// The `ralphy_device=…` pair the daemon sets on a first `/api` request.
async fn device_cookie(port: u16) -> String {
    let reply = http(port, "GET", "/api/session", "").await;
    reply
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.trim())
        .find(|value| value.starts_with("ralphy_device="))
        .unwrap_or_else(|| panic!("a device cookie in {reply}"))
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn open(port: u16, query: &str, cookie: &str) -> Result<Ws, u16> {
    let mut request = format!("ws://127.0.0.1:{port}/ws/session?{query}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    match tokio_tungstenite::connect_async(request).await {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(other) => panic!("the upgrade failed below HTTP: {other:?}"),
    }
}

/// Close the socket and wait for the daemon to answer the close, so the
/// writer slot is free before the next attach.
async fn close(mut ws: Ws) {
    ws.send(Message::Close(None)).await.unwrap();
    let _drained = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(_)) = ws.next().await {}
    })
    .await;
}

async fn launched_id(port: u16, cookie: &str) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let reply = http(port, "GET", "/api/sessions", cookie).await;
        let body = reply.split_once("\r\n\r\n").map_or("", |(_, b)| b);
        if let Ok(list) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(id) = list[0]["id"].as_u64() {
                return id;
            }
        }
        assert!(Instant::now() < deadline, "no session in {reply}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn console_lines(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let log = std::fs::read_to_string(dir.join("daemon-audit.jsonl")).unwrap_or_default();
    log.lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|line| {
            line["event"]
                .as_str()
                .is_some_and(|e| e.starts_with("console_"))
        })
        .collect()
}

#[tokio::test]
async fn a_launch_and_a_takeover_are_recorded_and_a_reattach_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = serve(dir.path()).await;
    let cookie = device_cookie(port).await;
    let device = cookie.split('.').nth(1).unwrap().to_string();

    let ws = open(
        port,
        "repo=owner%2Fworkbench&agent=claude&holder=tab-a",
        &cookie,
    )
    .await
    .expect("the launch upgrades");
    let id = launched_id(port, &cookie).await;
    close(ws).await;

    // A plain reattach by the same tab. The slot may still be closing.
    let mut reattach = open(port, &format!("id={id}&holder=tab-a"), &cookie).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while matches!(reattach, Err(409)) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        reattach = open(port, &format!("id={id}&holder=tab-a"), &cookie).await;
    }
    close(reattach.expect("the reattach upgrades")).await;

    let takeover = open(port, &format!("id={id}&takeover=1&holder=tab-b"), &cookie)
        .await
        .expect("the take-over upgrades");
    close(takeover).await;

    // A refused reattach keeps its HTTP status.
    assert!(matches!(open(port, "id=999999", &cookie).await, Err(404)));

    http(
        port,
        "POST",
        &format!("/api/sessions/close?id={id}"),
        &cookie,
    )
    .await;

    let lines = console_lines(dir.path());
    let kinds: Vec<&str> = lines.iter().map(|l| l["event"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["console_launch", "console_takeover"], "{lines:#?}");
    let launch = &lines[0];
    assert_eq!(launch["session"], id);
    assert_eq!(launch["agent"], "claude");
    assert_eq!(launch["repo"], "owner/workbench");
    assert_eq!(launch["holder"], "tab-a");
    assert_eq!(launch["device"], device.as_str());
    assert_eq!(launch["actor"], "device");
    let takeover = &lines[1];
    assert_eq!(takeover["session"], id);
    assert_eq!(takeover["holder"], "tab-b");
    assert_eq!(takeover["device"], device.as_str());
}
