use super::*;

#[test]
fn a_403_is_a_legible_rejection() {
    assert_eq!(
        http_verdict(403, false),
        Some(PeerStatus::Unauthorized { tunnel: false })
    );
    assert_eq!(
        http_verdict(403, true),
        Some(PeerStatus::Unauthorized { tunnel: true })
    );
}

#[test]
fn a_status_that_is_not_200_or_a_refusal_is_unreachable() {
    assert_eq!(
        http_verdict(500, false),
        Some(PeerStatus::Unreachable {
            why: "peer answered HTTP 500 to the handshake".to_string()
        })
    );
    assert_eq!(http_verdict(200, false), None);
}

#[test]
fn a_hello_without_a_protocol_version_is_not_reachable() {
    let hello = serde_json::json!({"daemon_id": "x"});
    assert_eq!(
        protocol_verdict(Some(&hello)),
        PeerStatus::Unreachable {
            why: "the handshake answered without a protocol_version".to_string()
        }
    );
    assert_eq!(
        protocol_verdict(None),
        PeerStatus::Unreachable {
            why: "the handshake answered without a protocol_version".to_string()
        }
    );
}
