use super::*;
use crate::peer::PeerDescriptor;

fn peer(daemon_id: &str, name: &str, environment: &str) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: daemon_id.into(),
        name: name.into(),
        avatar: "🐺".into(),
        address: "127.0.0.1".into(),
        port: 7257,
        environment: environment.into(),
        token: "tok".into(),
        protocol_version: crate::peer::PEER_PROTOCOL_VERSION,
        nudge: None,
    }
}

fn store(pairs: &[(&str, &str)]) -> RegistryStore {
    let mut s = RegistryStore::default();
    for (slug, path) in pairs {
        s.upsert(slug, path);
    }
    s
}

#[test]
fn same_slug_from_two_daemons_yields_two_entries() {
    let local = store(&[("owner/repo", "C:/Dev/repo")]);
    let theirs = store(&[("owner/repo", "/home/p/repo")]);
    let d = peer("01XYZ", "wsl-box", "WSL: Ubuntu-22.04");
    let rows = aggregate(
        ("01ABC", "anvil", "Windows", &local),
        &[(&d, &PeerStatus::Reachable, Some(&theirs))],
    );

    assert_eq!(rows.len(), 2, "the same slug on two daemons is two rows");
    let by_key = |k: &str| {
        rows.iter()
            .find(|r| r.key == k)
            .unwrap_or_else(|| panic!("missing key {k}; got {:?}", rows))
    };
    assert_eq!(by_key("01ABC/owner/repo").path, "C:/Dev/repo");
    assert_eq!(by_key("01XYZ/owner/repo").path, "/home/p/repo");
    assert!(by_key("01ABC/owner/repo").local);
    assert!(!by_key("01XYZ/owner/repo").local);
}

#[test]
fn unreachable_peer_still_contributes_its_repos() {
    let local = store(&[("owner/local", "C:/Dev/local")]);
    let theirs = store(&[("owner/a", "/home/a"), ("owner/b", "/home/b")]);
    let d = peer("01XYZ", "wsl-box", "WSL: Ubuntu-22.04");

    let up = aggregate(
        ("01ABC", "anvil", "Windows", &local),
        &[(&d, &PeerStatus::Reachable, Some(&theirs))],
    );
    let down = aggregate(
        ("01ABC", "anvil", "Windows", &local),
        &[(
            &d,
            &PeerStatus::Unreachable {
                why: "connection refused".into(),
            },
            Some(&theirs),
        )],
    );

    assert_eq!(
        up.len(),
        down.len(),
        "an unreachable peer is MARKED, never dropped"
    );
    let peer_rows: Vec<&FederatedRepo> = down.iter().filter(|r| !r.local).collect();
    assert_eq!(peer_rows.len(), 2);
    for row in peer_rows {
        assert_eq!(row.peer_state, "unreachable");
        assert!(
            !row.reachable,
            "an unreachable peer's repos are not actionable"
        );
    }
    assert_eq!(
        down.iter().filter(|r| r.local).count(),
        1,
        "the local rows are untouched"
    );
}

#[test]
fn a_peer_with_no_store_contributes_no_rows() {
    let local = store(&[("owner/local", "C:/Dev/local")]);
    let d = peer("01XYZ", "wsl-box", "WSL: Ubuntu-22.04");
    let rows = aggregate(
        ("01ABC", "anvil", "Windows", &local),
        &[(&d, &PeerStatus::Unreachable { why: "down".into() }, None)],
    );
    assert_eq!(rows.len(), 1, "only the local row: {rows:?}");
}

#[test]
fn ordering_is_deterministic() {
    let local = store(&[("owner/local", "C:/Dev/local")]);
    let a_store = store(&[("owner/a", "/a")]);
    let b_store = store(&[("owner/b", "/b")]);
    let a = peer("01AAA", "alpha", "WSL: Ubuntu-22.04");
    let b = peer("01BBB", "beta", "WSL: Debian");

    let forward = aggregate(
        ("01ABC", "anvil", "Windows", &local),
        &[
            (&a, &PeerStatus::Reachable, Some(&a_store)),
            (&b, &PeerStatus::Reachable, Some(&b_store)),
        ],
    );
    let reversed = aggregate(
        ("01ABC", "anvil", "Windows", &local),
        &[
            (&b, &PeerStatus::Reachable, Some(&b_store)),
            (&a, &PeerStatus::Reachable, Some(&a_store)),
        ],
    );

    let keys =
        |rows: &[FederatedRepo]| -> Vec<String> { rows.iter().map(|r| r.key.clone()).collect() };
    assert_eq!(
        keys(&forward),
        keys(&reversed),
        "the order peers arrive in must not change the view"
    );
}

#[test]
fn store_from_repos_json_keeps_slug_and_path_and_ignores_extra_fields() {
    let body = br#"[
      {"slug":"owner/a","path":"/home/a","reachable":true,"branch":"main","dirty":false,"remote":null},
      {"slug":"owner/b","path":"/home/b","reachable":false,"branch":null,"dirty":true,"remote":null}
    ]"#;
    let store = store_from_repos_json(body).expect("a well-formed /api/repos body folds");
    assert_eq!(store.repos.len(), 2);
    assert_eq!(store.entry("owner/a").unwrap().path, "/home/a");
    assert_eq!(store.entry("owner/b").unwrap().path, "/home/b");
}

#[test]
fn store_from_repos_json_rejects_a_non_list_body() {
    assert!(store_from_repos_json(b"not json").is_none());
    assert!(store_from_repos_json(br#"{"error":"nope"}"#).is_none());
}
