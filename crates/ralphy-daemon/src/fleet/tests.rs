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

/// A peer's repos as that peer would serve them: reachable, on no named branch
/// unless a test says otherwise.
fn peer_store(pairs: &[(&str, &str)]) -> PeerRepoStore {
    pairs
        .iter()
        .map(|(slug, path)| {
            (
                (*slug).to_string(),
                PeerRepoRow {
                    path: (*path).to_string(),
                    reachable: true,
                    branch: None,
                },
            )
        })
        .collect()
}

#[test]
fn same_slug_from_two_daemons_yields_two_entries() {
    let local = store(&[("owner/repo", "C:/Dev/repo")]);
    let theirs = peer_store(&[("owner/repo", "/home/p/repo")]);
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
    let theirs = peer_store(&[("owner/a", "/home/a"), ("owner/b", "/home/b")]);
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
    let a_store = peer_store(&[("owner/a", "/a")]);
    let b_store = peer_store(&[("owner/b", "/b")]);
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
fn store_from_repos_json_keeps_the_peers_own_verdict_and_ignores_extra_fields() {
    let body = br#"[
      {"slug":"owner/a","path":"/home/a","reachable":true,"branch":"main","dirty":false,"remote":null},
      {"slug":"owner/b","path":"/home/b","reachable":false,"branch":null,"dirty":true,"remote":null}
    ]"#;
    let store = store_from_repos_json(body).expect("a well-formed /api/repos body folds");
    assert_eq!(store.len(), 2);
    let a = store.get("owner/a").expect("owner/a folds");
    assert_eq!(a.path, "/home/a");
    assert!(a.reachable);
    assert_eq!(a.branch.as_deref(), Some("main"));
    // The row this fold used to lose: a peer repo whose directory is gone.
    let b = store.get("owner/b").expect("owner/b folds");
    assert!(!b.reachable, "the peer said its own path is not there");
    assert_eq!(b.branch, None);
}

/// A peer repo whose path is gone is listed as gone, on a live peer.
///
/// Found in the #353 capstone: the fold kept only `slug -> path`, so the row's
/// `reachable` was re-derived from whether the *peer daemon* answered — and a
/// registered repo whose directory had been deleted inside the distro was shown
/// as healthy. This daemon must not stat a peer path to check (ADR-0052 §1), so
/// carrying the owner's verdict through is the only way to be right.
#[test]
fn a_live_peers_missing_repo_is_not_reachable() {
    let local = store(&[("owner/local", "C:/Dev/local")]);
    let mut theirs = peer_store(&[("owner/here", "/home/here"), ("owner/gone", "/home/gone")]);
    theirs
        .get_mut("owner/gone")
        .expect("fixture has owner/gone")
        .reachable = false;
    let d = peer("01XYZ", "wsl-box", "WSL: Ubuntu-22.04");

    let rows = aggregate(
        ("01ABC", "anvil", "Windows", &local),
        &[(&d, &PeerStatus::Reachable, Some(&theirs))],
    );
    let by_key = |k: &str| {
        rows.iter()
            .find(|r| r.key == k)
            .unwrap_or_else(|| panic!("missing key {k}; got {rows:?}"))
    };
    assert!(by_key("01XYZ/owner/here").reachable);
    assert!(
        !by_key("01XYZ/owner/gone").reachable,
        "a live peer's missing repo must not read as healthy"
    );
    assert_eq!(
        by_key("01XYZ/owner/gone").peer_state,
        "reachable",
        "the PEER is up; only its repo is gone — the two verdicts are distinct"
    );
}

/// A peer row carries the branch its owner reported.
#[test]
fn a_peer_row_carries_the_owners_branch() {
    let local = store(&[("owner/local", "C:/Dev/local")]);
    let mut theirs = peer_store(&[("owner/a", "/home/a")]);
    theirs
        .get_mut("owner/a")
        .expect("fixture has owner/a")
        .branch = Some("ralphy/init".to_string());
    let d = peer("01XYZ", "wsl-box", "WSL: Ubuntu-22.04");

    let rows = aggregate(
        ("01ABC", "anvil", "Windows", &local),
        &[(&d, &PeerStatus::Reachable, Some(&theirs))],
    );
    let row = rows
        .iter()
        .find(|r| r.key == "01XYZ/owner/a")
        .expect("the peer row is present");
    assert_eq!(row.branch.as_deref(), Some("ralphy/init"));
}

#[test]
fn store_from_repos_json_rejects_a_non_list_body() {
    assert!(store_from_repos_json(b"not json").is_none());
    assert!(store_from_repos_json(br#"{"error":"nope"}"#).is_none());
}
