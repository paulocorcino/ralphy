//! The fold of the peer store records: what it accepts and what it rejects.

use super::*;

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
fn fold_keeps_a_tunnel_host_whose_saved_version_is_old() {
    let text = descriptor_toml("01AAA", 7257).replace(
        &format!("protocol_version = {PEER_PROTOCOL_VERSION}"),
        "protocol_version = 3",
    ) + "[tunnel]\ndestination = \"user@host\"\npeer_port = 7257\nlocal_port = 7257\n";
    let (accepted, rejected) = fold(&[("01AAA.toml".to_string(), text)]);
    assert!(
        rejected.is_empty(),
        "the host stays in the list: {rejected:?}"
    );
    assert_eq!(accepted.len(), 1);
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

fn tunnel_toml(port: u16, local_port: u16) -> String {
    format!(
        "{}\n[tunnel]\ndestination = \"svrapp\"\npeer_port = 7257\nlocal_port = {local_port}\n",
        descriptor_toml("01TUN", port)
    )
}

fn socket_tunnel_toml(socket: &str) -> String {
    format!(
        "{}
[tunnel]
destination = \"svrapp\"
peer_socket = \"{socket}\"
local_port = 7401
",
        descriptor_toml("01TUN", 7401)
    )
}

#[test]
fn fold_reads_a_tunnel_to_a_socket() {
    let text = socket_tunnel_toml("/home/ralphy2/.ralphy/daemon.sock");
    let (accepted, rejected) = fold(&[("01TUN.toml".to_string(), text)]);
    assert!(rejected.is_empty(), "got: {rejected:?}");
    let tunnel = accepted[0].tunnel.clone().unwrap();
    assert_eq!(tunnel.peer_port, 0);
    assert_eq!(
        tunnel.peer_socket.as_deref(),
        Some("/home/ralphy2/.ralphy/daemon.sock")
    );
    let written = toml::to_string_pretty(&accepted[0]).unwrap();
    assert!(!written.contains("peer_port"), "got: {written}");
}

#[test]
fn fold_rejects_a_tunnel_with_neither_port_nor_socket() {
    let text = tunnel_toml(7401, 7401).replace("peer_port = 7257\n", "");
    let (accepted, rejected) = fold(&[("01TUN.toml".to_string(), text)]);
    assert!(accepted.is_empty(), "got: {accepted:?}");
    assert!(
        matches!(&rejected[0], PeerReject::Malformed { why, .. }
            if why.contains("its tunnel needs a destination")),
        "got: {:?}",
        rejected[0]
    );
}

#[test]
fn fold_rejects_a_relative_socket() {
    let alone = socket_tunnel_toml("daemon.sock");
    let with_port = alone.replace("local_port = 7401", "peer_port = 7257\nlocal_port = 7401");
    for text in [alone, with_port] {
        let (accepted, rejected) = fold(&[("01TUN.toml".to_string(), text.clone())]);
        assert!(accepted.is_empty(), "{text}\ngot: {accepted:?}");
        assert!(
            matches!(&rejected[0], PeerReject::Malformed { .. }),
            "got: {:?}",
            rejected[0]
        );
    }
}

/// A descriptor written before sockets existed has `peer_port` and no
/// `peer_socket`.
#[test]
fn a_descriptor_with_peer_port_still_reads() {
    let (accepted, rejected) = fold(&[("01TUN.toml".to_string(), tunnel_toml(7401, 7401))]);
    assert!(rejected.is_empty(), "got: {rejected:?}");
    let tunnel = accepted[0].tunnel.clone().unwrap();
    assert_eq!((tunnel.peer_port, tunnel.peer_socket), (7257, None));
}

#[test]
fn fold_reads_a_tunnel_section() {
    let (accepted, rejected) = fold(&[("01TUN.toml".to_string(), tunnel_toml(7401, 7401))]);
    assert!(rejected.is_empty(), "got: {rejected:?}");
    assert_eq!(
        accepted[0].tunnel,
        Some(TunnelSpec {
            destination: "svrapp".into(),
            peer_port: 7257,
            peer_socket: None,
            local_port: 7401,
            identity_file: None,
        })
    );
}

/// An older daemon's descriptor has no tunnel, and writing one back must not
/// add an empty table: the file stays byte-compatible with protocol 3 readers.
#[test]
fn fold_without_a_tunnel_writes_no_tunnel_table() {
    let (accepted, rejected) = fold(&[("01AAA.toml".to_string(), descriptor_toml("01AAA", 7257))]);
    assert!(rejected.is_empty(), "got: {rejected:?}");
    assert_eq!(accepted[0].tunnel, None);
    let text = toml::to_string_pretty(&accepted[0]).unwrap();
    assert!(!text.contains("tunnel"), "got: {text}");
}

#[test]
fn fold_rejects_a_tunnel_whose_local_port_is_not_its_port() {
    let (accepted, rejected) = fold(&[("01TUN.toml".to_string(), tunnel_toml(7402, 7401))]);
    assert!(accepted.is_empty(), "got: {accepted:?}");
    assert!(
        matches!(&rejected[0], PeerReject::Malformed { why, .. }
            if why.contains("its tunnel local port 7401 is not its port 7402")),
        "got: {:?}",
        rejected[0]
    );
}

#[test]
fn fold_rejects_a_tunnel_with_no_destination_or_a_port_0() {
    let no_destination =
        tunnel_toml(7401, 7401).replace(r#"destination = "svrapp""#, r#"destination = " ""#);
    let no_peer_port = tunnel_toml(7401, 7401).replace("peer_port = 7257", "peer_port = 0");
    for text in [no_destination, no_peer_port] {
        let (accepted, rejected) = fold(&[("01TUN.toml".to_string(), text)]);
        assert!(accepted.is_empty(), "got: {accepted:?}");
        assert!(
            matches!(&rejected[0], PeerReject::Malformed { why, .. }
                if why.contains("its tunnel needs a destination")),
            "got: {:?}",
            rejected[0]
        );
    }
}
