use std::time::Duration;

use super::*;

/// A daemon with no listener bound has nothing to serve: it does not wait
/// for the stop signal.
#[tokio::test]
async fn with_no_listener_the_serving_ends_at_once() {
    let (_stop_tx, stop) = tokio::sync::watch::channel(false);
    let served = tokio::time::timeout(
        Duration::from_secs(5),
        serve_listeners(
            None,
            #[cfg(unix)]
            None,
            axum::Router::new(),
            stop,
        ),
    )
    .await;
    assert!(
        matches!(served, Ok(Ok(()))),
        "serving with no listener returns Ok at once: {served:?}"
    );
}

/// The stop of the real process: the listeners stop, and `run` drops the
/// runtime right after, which would cut the last frames of a page socket
/// (#834). The stop waits for the open page socket, so a console's
/// `session-end {reason: daemon-shutdown}` can leave. The daemon runs on a
/// runtime of its own, dropped as `run` drops it.
#[tokio::test(flavor = "multi_thread")]
async fn the_process_stop_waits_for_an_open_page_socket() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    use crate::protocol::{self, Command, Frame};

    let dir = tempfile::tempdir().unwrap();
    let registry_path = dir.path().join("repos.toml");
    let mut store = crate::registry::RegistryStore::default();
    store.upsert("owner/workbench", &dir.path().to_string_lossy());
    crate::registry::save_to(&store, &registry_path).unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop_tx, stop) = tokio::sync::watch::channel(false);
    let daemon = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let served = runtime.block_on(async move {
            let (sockets, drain) = crate::mux::tracked();
            let app = crate::router_tracking(
                None,
                registry_path,
                std::path::PathBuf::from("does-not-exist"),
                crate::StorePaths::default(),
                std::time::Instant::now(),
                stop.clone(),
                crate::auth::AuthState::localhost(),
                sockets,
            );
            let tcp = tokio::net::TcpListener::from_std(listener).unwrap();
            serve_until_stopped(
                Some(tcp),
                #[cfg(unix)]
                None,
                app,
                stop,
                drain,
            )
            .await
        });
        drop(runtime);
        served
    });

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
        .await
        .unwrap();
    let open = Frame::Command(Command {
        id: 0,
        verb: "channel.open".to_string(),
        payload: serde_json::json!({
            "channel": 1, "kind": "console", "repo": "owner/workbench", "console": 1,
        }),
    });
    ws.send(Message::Binary(protocol::encode(&open).into()))
        .await
        .unwrap();

    // The verbs of the inner frames on channel 1, until `session-end`.
    let mut verbs = Vec::new();
    let mut inner = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(Ok(message)) = ws.next().await {
            let Message::Binary(bytes) = message else {
                continue;
            };
            let Ok(Frame::Channel {
                channel: 1,
                more,
                piece,
            }) = protocol::decode(&bytes)
            else {
                continue;
            };
            inner.extend_from_slice(&piece);
            if more {
                continue;
            }
            if let Ok(Frame::Command(c)) = protocol::decode(&std::mem::take(&mut inner)) {
                if c.verb == "session-open" {
                    stop_tx.send(true).unwrap();
                }
                let reason = c.payload["reason"].as_str().map(str::to_string);
                verbs.push((c.verb, reason));
                if verbs.last().is_some_and(|(verb, _)| verb == "session-end") {
                    break;
                }
            }
        }
    })
    .await;
    assert!(read.is_ok(), "the console ends after the stop: {verbs:?}");
    assert_eq!(
        verbs.last(),
        Some(&(
            "session-end".to_string(),
            Some("daemon-shutdown".to_string())
        )),
        "the console tells the page the daemon stopped"
    );
    // The page has not closed its socket: the process waits for it, up to
    // the bound, instead of dropping the runtime under it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !daemon.is_finished(),
        "the daemon's stop waits for an open page socket"
    );
    drop(ws);
    let joined = tokio::task::spawn_blocking(move || daemon.join());
    let served = tokio::time::timeout(SOCKETS_WAIT, joined)
        .await
        .expect("the daemon stops within the bound")
        .unwrap();
    served.unwrap().unwrap();
}
