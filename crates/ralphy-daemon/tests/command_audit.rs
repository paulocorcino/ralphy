//! A verb that changes state, sent on a `spawn` channel of the page socket,
//! writes a `command` line in the audit log with the device of the browser
//! that asked (ADR-0074 D12).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ralphy_daemon::auth::AuthState;
use ralphy_daemon::protocol::{self, Command, Frame};
use ralphy_daemon::router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

#[path = "support/page.rs"]
pub mod page;

async fn serve(registry_path: PathBuf) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test port binds");
    let port = listener
        .local_addr()
        .expect("the test port has an address")
        .port();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let app = router(
        None,
        registry_path,
        PathBuf::from("does-not-exist"),
        ralphy_daemon::StorePaths::default(),
        Instant::now(),
        rx,
        AuthState::localhost(),
    );
    // As `serve.rs` serves the TCP listener: with the connection's address.
    tokio::spawn(async move {
        let _shutdown = tx;
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("the test server runs");
    });
    port
}

/// The `ralphy_device=…` pair the daemon sets on a first `/api` request.
async fn device_cookie(port: u16) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the test socket connects");
    stream
        .write_all(
            format!(
                "GET /api/session HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .expect("the request is written");
    let mut reply = String::new();
    stream
        .read_to_string(&mut reply)
        .await
        .expect("the reply is read");
    reply
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value.trim())
        .find(|value| value.starts_with("ralphy_device="))
        .unwrap_or_else(|| panic!("a device cookie in {reply}"))
        .split(';')
        .next()
        .expect("the reply sets a device cookie")
        .to_string()
}

#[tokio::test]
async fn a_command_that_changes_state_is_recorded_with_its_device() {
    let dir = tempfile::tempdir().unwrap();
    let port = serve(dir.path().join("repos.toml")).await;
    let cookie = device_cookie(port).await;
    let device = cookie.split('.').nth(1).unwrap().to_string();

    let mut request = page::url(port).into_client_request().unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let ws = page::connect_request(request).await;
    let mut channel = page::Channel::spawn_on(ws);
    channel
        .send(Message::Binary(
            protocol::encode(&Frame::Command(Command {
                id: 1,
                verb: "branch.switch".to_string(),
                payload: serde_json::json!({"repo": "nope", "name": "main"}),
            }))
            .into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(Ok(_)) = channel.next().await {}
    })
    .await
    .expect("the server closes the socket");

    let log = std::fs::read_to_string(dir.path().join("daemon-audit.jsonl")).unwrap();
    let commands: Vec<serde_json::Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|line: &serde_json::Value| line["event"] == "command")
        .collect();
    assert_eq!(commands.len(), 1, "{log}");
    assert_eq!(commands[0]["verb"], "branch.switch");
    assert_eq!(commands[0]["repo"], "nope");
    assert_eq!(commands[0]["device"], device.as_str());
    assert_eq!(commands[0]["actor"], "device");
    assert_eq!(
        commands[0]["ip"], "127.0.0.1",
        "a direct connection with no front is recorded with its own address"
    );
}
