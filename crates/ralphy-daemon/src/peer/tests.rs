use super::client::{classify_address, classify_unreachable, PeerStatus};
use super::*;

fn descriptor_toml(daemon_id: &str, port: u16) -> String {
    format!(
        r#"
daemon_id = "{daemon_id}"
name = "anvil"
avatar = "🐙"
address = "127.0.0.1"
port = {port}
environment = "WSL: Ubuntu-22.04"
token = "deadbeef"
protocol_version = {PEER_PROTOCOL_VERSION}
"#
    )
}

#[test]
fn fold_of_empty_store_is_empty() {
    let (accepted, rejected) = fold(&[]);
    assert!(accepted.is_empty());
    assert!(rejected.is_empty());
}

#[test]
fn fold_degrades_one_bad_record_not_the_daemon() {
    let records = vec![
        ("01AAA.toml".to_string(), descriptor_toml("01AAA", 7257)),
        ("01BBB.toml".to_string(), "not toml at all".to_string()),
        ("01CCC.toml".to_string(), descriptor_toml("01CCC", 7258)),
    ];
    let (accepted, rejected) = fold(&records);
    assert_eq!(accepted.len(), 2, "the good records survive: {accepted:?}");
    assert_eq!(rejected.len(), 1, "exactly one rejection: {rejected:?}");
    assert!(
        matches!(&rejected[0], PeerReject::Malformed { file, .. } if file == "01BBB.toml"),
        "got: {:?}",
        rejected[0]
    );
    let ids: Vec<&str> = accepted.iter().map(|d| d.daemon_id.as_str()).collect();
    assert_eq!(ids, vec!["01AAA", "01CCC"]);
}

#[test]
fn fold_keeps_unknown_fields() {
    let text = format!("{}\nfuture_field = \"x\"\n", descriptor_toml("01AAA", 7257));
    let (accepted, rejected) = fold(&[("01AAA.toml".to_string(), text)]);
    assert_eq!(
        accepted.len(),
        1,
        "a newer peer's extra field must not be fatal: {rejected:?}"
    );
    assert!(rejected.is_empty());
}

#[test]
fn fold_rejects_incompatible_version() {
    let text = descriptor_toml("01AAA", 7257).replace(
        &format!("protocol_version = {PEER_PROTOCOL_VERSION}"),
        "protocol_version = 999",
    );
    let (accepted, rejected) = fold(&[("01AAA.toml".to_string(), text)]);
    assert!(
        accepted.is_empty(),
        "an incompatible peer is NOT usable: {accepted:?}"
    );
    assert!(
        matches!(
            &rejected[0],
            PeerReject::IncompatibleVersion { theirs: 999, .. }
        ),
        "got: {:?}",
        rejected[0]
    );
    assert_eq!(
        rejected[0].version_mismatch_for("01AAA"),
        Some(("WSL: Ubuntu-22.04", 999))
    );
}

#[test]
fn fold_first_duplicate_identity_wins() {
    let records = vec![
        ("a.toml".to_string(), descriptor_toml("01SAME", 7257)),
        ("b.toml".to_string(), descriptor_toml("01SAME", 9999)),
    ];
    let (accepted, rejected) = fold(&records);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].port, 7257, "the FIRST file by name wins");
    assert!(
        matches!(&rejected[0], PeerReject::DuplicateIdentity { file, daemon_id }
            if file == "b.toml" && daemon_id == "01SAME"),
        "got: {:?}",
        rejected[0]
    );
}

#[test]
fn read_store_of_missing_dir_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (accepted, rejected) = read_store(&dir.path().join("no-such-peers"));
    assert!(accepted.is_empty());
    assert!(rejected.is_empty());
}

#[test]
fn read_store_sorts_by_file_name_and_skips_non_toml() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("b.toml"), descriptor_toml("01BBB", 2)).unwrap();
    std::fs::write(dir.path().join("a.toml"), descriptor_toml("01AAA", 1)).unwrap();
    std::fs::write(dir.path().join("README.md"), "not a descriptor").unwrap();
    let (accepted, rejected) = read_store(dir.path());
    let ids: Vec<&str> = accepted.iter().map(|d| d.daemon_id.as_str()).collect();
    assert_eq!(ids, vec!["01AAA", "01BBB"], "sorted by file name");
    assert!(
        rejected.is_empty(),
        "a non-toml file is skipped, not rejected"
    );
}

#[test]
fn environment_label_names_the_distro() {
    assert_eq!(
        environment_label(Some("Ubuntu-22.04"), "linux"),
        "WSL: Ubuntu-22.04"
    );
    assert_eq!(environment_label(None, "windows"), "Windows");
    assert_eq!(environment_label(None, "linux"), "Linux");
    assert_eq!(environment_label(None, "macos"), "macOS");
    assert_eq!(environment_label(None, "freebsd"), "freebsd");
}

#[test]
fn writer_emits_every_announced_field() {
    let dir = tempfile::tempdir().unwrap();
    let d = PeerDescriptor {
        daemon_id: "01WRITE".into(),
        name: "anvil".into(),
        avatar: "🐙".into(),
        address: "127.0.0.1".into(),
        port: 7443,
        environment: "WSL: Ubuntu-22.04".into(),
        token: "tok-abc".into(),
        protocol_version: PEER_PROTOCOL_VERSION,
        nudge: Some(NudgeSpec {
            distro: "Ubuntu-22.04".into(),
            unit: "ralphy-daemon.service".into(),
        }),
    };
    let path = write_descriptor(dir.path(), &d).unwrap();
    assert!(path.ends_with("01WRITE.toml"));
    assert_eq!(
        path.parent().unwrap(),
        dir.path().join("peers"),
        "the descriptor lands in <store>/peers/"
    );

    let back: PeerDescriptor = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(back.daemon_id, "01WRITE");
    assert_eq!(back.name, "anvil");
    assert_eq!(back.avatar, "🐙");
    assert_eq!(back.address, "127.0.0.1");
    assert_eq!(back.port, 7443, "the BOUND port is announced");
    assert_eq!(back.environment, "WSL: Ubuntu-22.04");
    assert_eq!(back.token, "tok-abc");
    assert_eq!(back.protocol_version, PEER_PROTOCOL_VERSION);
    assert_eq!(back.nudge.unwrap().unit, "ralphy-daemon.service");
}

#[test]
fn classify_refuses_non_loopback_address() {
    assert!(
        matches!(
            classify_address("10.0.0.5"),
            Some(PeerStatus::Refused { .. })
        ),
        "a routable address must never be dialled"
    );
    assert!(matches!(
        classify_address("not-an-ip"),
        Some(PeerStatus::Refused { .. })
    ));
    assert_eq!(classify_address("127.0.0.1"), None);
    assert_eq!(classify_address("::1"), None);
    // A refusal must NAME the address, so the operator can find the descriptor
    // that carries it — "refused" alone is not a diagnosis.
    let why = classify_address("10.0.0.5")
        .unwrap()
        .diagnosis("WSL: Ubuntu");
    assert!(
        why.contains("10.0.0.5") && why.contains("loopback"),
        "got: {why}"
    );
}

/// The two opposite situations one `unreachable` used to cover. A stopped distro
/// is the ordinary case and must not read as a fault; a running distro whose
/// daemon is silent must say so, because the operator's next act differs.
#[test]
fn a_failed_dial_separates_a_stopped_distro_from_a_dead_daemon() {
    let stopped = classify_unreachable(Some("Ubuntu-22.04"), Some(false), "refused".into());
    assert_eq!(stopped.state(), "asleep");
    let asleep = stopped.diagnosis("WSL: Ubuntu-22.04");
    assert!(asleep.contains("Ubuntu-22.04"), "got: {asleep}");

    let dead = classify_unreachable(Some("Ubuntu-22.04"), Some(true), "refused".into());
    assert_eq!(dead.state(), "unreachable");
    let why = dead.diagnosis("WSL: Ubuntu-22.04");
    assert!(
        why.contains("is running, so its daemon is not"),
        "a running distro must be named as such: {why}"
    );
    assert!(
        why.contains("refused"),
        "the underlying dial error must survive: {why}"
    );
}

/// A host that could not be asked, and a peer with no distro to ask about, must
/// both land on the diagnosis that assumes nothing. Inventing "asleep" from an
/// unanswered question is the same bug this change exists to remove.
#[test]
fn an_unasked_host_invents_no_verdict() {
    for status in [
        classify_unreachable(Some("Ubuntu-22.04"), None, "refused".into()),
        classify_unreachable(None, None, "refused".into()),
        // A non-WSL peer: liveness is unknowable, whatever the host answered.
        classify_unreachable(None, Some(false), "refused".into()),
    ] {
        assert_eq!(status.state(), "unreachable", "got: {status:?}");
        assert_eq!(
            status,
            PeerStatus::Unreachable {
                why: "refused".into()
            },
            "the dial error must be carried through untouched"
        );
    }
}

#[test]
fn diagnosis_always_names_the_environment() {
    let env = "WSL: Ubuntu-22.04";
    for status in [
        PeerStatus::Reachable,
        PeerStatus::Unauthorized,
        PeerStatus::VersionMismatch {
            theirs: 999,
            ours: 1,
        },
        PeerStatus::Asleep {
            distro: "Ubuntu-22.04".into(),
        },
        PeerStatus::Unreachable { why: "boom".into() },
        PeerStatus::Refused { why: "boom".into() },
    ] {
        let d = status.diagnosis(env);
        assert!(
            d.contains(env),
            "{:?} diagnosis lost the environment: {d}",
            status
        );
    }
}
