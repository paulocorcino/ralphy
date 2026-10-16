use std::collections::HashMap;

use super::*;
use crate::fleet::{PeerRepoRow, PeerRepoStore};
use crate::routes::PeerMemory;
use http_body_util::BodyExt;

/// A cache that holds one peer which re-keyed `path-abc` to `owner/repo`.
fn cache_with_a_renamed_peer_repo() -> PeerRepoCache {
    let repos: PeerRepoStore = [(
        "owner/repo".to_string(),
        PeerRepoRow {
            path: "/home/repo".into(),
            reachable: true,
            branch: None,
            dirty: false,
            remote: None,
            former_slugs: vec!["path-abc".into()],
            root: None,
        },
    )]
    .into();
    let memory = PeerMemory {
        repos: Some(repos),
        ..PeerMemory::default()
    };
    Arc::new(std::sync::Mutex::new(HashMap::from([(
        "peer1".to_string(),
        memory,
    )])))
}

/// A desk saved before a peer's project was re-keyed names `<peer>/<old>`; the
/// host serves it under `<peer>/<new>` from the peer's own rename fact, so the
/// window finds its project again (ADR-0036 amendment 2026-10-15).
#[tokio::test]
async fn the_desk_follows_a_rename_that_happened_on_a_peer() {
    let dir = tempfile::tempdir().unwrap();
    let desk_path = dir.path().join("desk.toml");
    let mut stale = desk::DeskStore::default();
    stale.windows.push(desk::DeskRecord {
        id: "w1".into(),
        repo: "peer1/path-abc".into(),
        daemon_id: Some("peer1".into()),
        rect: desk::DeskRect {
            left: 1.0,
            top: 1.0,
            width: 300.0,
            height: 200.0,
        },
        ..desk::DeskRecord::default()
    });
    desk::save_to(&stale, &desk_path).unwrap();

    let res = desk_get_route(
        desk_path,
        dir.path().join("repos.toml"),
        cache_with_a_renamed_peer_repo(),
    )
    .await;
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        body.contains(r#""repo":"peer1/owner/repo""#) && !body.contains("path-abc"),
        "the record follows the peer's key: {body}"
    );

    let cold = PeerRepoCache::default();
    let aliases = former_slug_aliases(&dir.path().join("repos.toml"), &cold);
    assert!(aliases.is_empty(), "a cold cache maps nothing: {aliases:?}");
}
