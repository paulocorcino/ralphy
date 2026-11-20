use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::{from_peer, BytesQuery};
use crate::peer::client::StreamDeadlines;
use crate::peer::{self, PeerDescriptor, PEER_PROTOCOL_VERSION};
use crate::{auth, epoch, registry, router, StorePaths};

const PEER_ID: &str = "01J9ZQ0000000000000000PEER";
const TOKEN: &str = "peer-token";

/// A PNG of 64 bytes: the signature, then padding.
fn png() -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(64, 7);
    bytes
}

/// A store folder whose registry holds the repo `demo` with `a.png`.
fn store_with_demo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).expect("the repo folder");
    std::fs::write(repo.join("a.png"), png()).expect("the image");
    let registry_path = dir.path().join("repos.toml");
    let mut store = registry::RegistryStore::default();
    store.upsert("demo", &repo.to_string_lossy());
    registry::save_to(&store, &registry_path).expect("the registry is saved");
    (dir, registry_path)
}

fn router_on(registry_path: &Path, auth: Arc<auth::AuthState>) -> Router {
    let (_tx, shutdown) = tokio::sync::watch::channel(false);
    router(
        None,
        registry_path.to_path_buf(),
        PathBuf::from("does-not-exist"),
        StorePaths::default(),
        std::time::Instant::now(),
        shutdown,
        auth,
    )
}

fn bearer_auth() -> Arc<auth::AuthState> {
    auth::AuthState::fixed(
        auth::AuthPolicy::Bearer(TOKEN.to_owned()),
        epoch::SessionEpoch::in_memory_detached(),
    )
}

/// Serve `app` on a loopback port and return the port.
async fn serve(app: Router) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let port = listener.local_addr().expect("the bound address").port();
    tokio::spawn(async move { axum::serve(listener, app).await });
    port
}

fn descriptor(port: u16) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: PEER_ID.to_owned(),
        name: "peer".to_owned(),
        avatar: "x".to_owned(),
        address: "127.0.0.1".to_owned(),
        port,
        environment: "WSL: Ubuntu".to_owned(),
        os: String::new(),
        token: TOKEN.to_owned(),
        protocol_version: PEER_PROTOCOL_VERSION,
        tunnel: None,
        nudge: None,
    }
}

fn header_of(resp: &Response, name: header::HeaderName) -> Option<&str> {
    resp.headers().get(name).and_then(|v| v.to_str().ok())
}

#[tokio::test]
async fn a_peer_range_comes_back_as_206() {
    let (_peer_dir, peer_registry) = store_with_demo();
    let port = serve(router_on(&peer_registry, bearer_auth())).await;
    let local = tempfile::tempdir().expect("a temp dir");
    let local_registry = local.path().join("repos.toml");
    peer::write_descriptor(local.path(), &descriptor(port)).expect("the peer is announced");

    let uri = format!("/api/file/bytes?repo={PEER_ID}/demo&path=a.png");
    let resp = router_on(&local_registry, auth::AuthState::localhost())
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("range", "bytes=2-5")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("the router answers");

    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        header_of(&resp, header::CONTENT_RANGE),
        Some("bytes 2-5/64")
    );
    assert_eq!(header_of(&resp, header::CONTENT_TYPE), Some("image/png"));
    assert_eq!(header_of(&resp, header::CACHE_CONTROL), Some("no-store"));
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("the body");
    assert_eq!(body.as_ref(), png().get(2..6).expect("four bytes"));
}

#[tokio::test]
async fn the_peer_route_needs_the_peer_key() {
    let (_dir, registry_path) = store_with_demo();
    let ask = |key: Option<&str>| {
        let mut req = Request::builder().uri("/api/peer/file/bytes?repo=demo&path=a.png");
        if let Some(key) = key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        router_on(&registry_path, bearer_auth())
            .oneshot(req.body(Body::empty()).expect("a request"))
    };
    let refused = ask(None).await.expect("the router answers");
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    let served = ask(Some(TOKEN)).await.expect("the router answers");
    assert_eq!(served.status(), StatusCode::OK);
}

/// A fake peer whose file bytes route sends what `chunks` receives, and
/// stays open until the sender is dropped.
async fn fake_peer(chunks: tokio::sync::mpsc::Receiver<Bytes>) -> PeerDescriptor {
    let body = Arc::new(std::sync::Mutex::new(Some(chunks)));
    let app = Router::new().route(
        "/api/peer/file/bytes",
        axum::routing::get(move || {
            let rx = body.lock().expect("the body lock").take();
            async move {
                let rx = rx.expect("one request");
                let stream = futures_util::stream::unfold(rx, |mut rx| async move {
                    rx.recv()
                        .await
                        .map(|chunk| (Ok::<_, std::io::Error>(chunk), rx))
                });
                Body::from_stream(stream)
            }
        }),
    );
    descriptor(serve(app).await)
}

fn query() -> BytesQuery {
    BytesQuery {
        repo: format!("{PEER_ID}/demo"),
        path: "a.mp4".to_owned(),
        checkout: None,
    }
}

const SHORT: StreamDeadlines = StreamDeadlines {
    first_byte: Duration::from_secs(5),
    silence: Duration::from_millis(300),
};

#[tokio::test]
async fn the_body_is_passed_on_as_it_arrives() {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let peer = fake_peer(rx).await;
    tx.send(Bytes::from_static(b"first")).await.expect("sent");

    let resp = from_peer(&peer, "demo", &query(), &HeaderMap::new(), SHORT).await;
    let mut body = resp.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .expect("the first chunk comes before the last is sent")
        .expect("a chunk")
        .expect("no error");
    assert_eq!(first.as_ref(), b"first");

    tx.send(Bytes::from_static(b"last")).await.expect("sent");
    drop(tx);
    let tail = Body::from_stream(body).collect().await.expect("the rest");
    assert_eq!(tail.to_bytes().as_ref(), b"last");
}

#[tokio::test]
async fn a_silent_peer_ends_the_answer() {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let peer = fake_peer(rx).await;
    tx.send(Bytes::from_static(b"first")).await.expect("sent");

    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-"));
    let resp = from_peer(&peer, "demo", &query(), &headers, SHORT).await;
    let ended = tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(resp.into_body(), usize::MAX),
    )
    .await
    .expect("the answer ends before the test deadline");
    assert!(
        ended.is_err(),
        "a silent peer is an error, not a whole body"
    );
    drop(tx);
}
