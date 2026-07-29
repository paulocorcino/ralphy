use super::*;
use crate::dispatch;
use crate::peer::PEER_PROTOCOL_VERSION;

const LOCAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const PEER_A_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const PEER_B_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";

fn peer(id: &str, environment: &str) -> PeerDescriptor {
    PeerDescriptor {
        daemon_id: id.to_string(),
        name: environment.to_string(),
        avatar: String::new(),
        address: "127.0.0.1".to_string(),
        port: 7441,
        environment: environment.to_string(),
        token: "token".to_string(),
        protocol_version: PEER_PROTOCOL_VERSION,
        nudge: None,
    }
}

#[test]
fn repo_refs_route_by_a_parsed_daemon_identity() {
    let peers = [
        peer(PEER_A_ID, "WSL: Ubuntu-22.04"),
        peer(PEER_B_ID, "Linux"),
    ];

    assert_eq!(
        route("owner/repo", LOCAL_ID, &peers),
        Route::Local { slug: "owner/repo" }
    );
    assert_eq!(
        route("path-deadbeef", LOCAL_ID, &peers),
        Route::Local {
            slug: "path-deadbeef"
        }
    );
    assert_eq!(
        route(&format!("{LOCAL_ID}/owner/repo"), LOCAL_ID, &peers),
        Route::Local { slug: "owner/repo" }
    );
    assert_eq!(
        route(&format!("{PEER_A_ID}/owner/repo"), LOCAL_ID, &peers),
        Route::Peer {
            peer: &peers[0],
            slug: "owner/repo"
        }
    );
    assert_eq!(
        route(&format!("{PEER_B_ID}/owner/repo"), LOCAL_ID, &peers),
        Route::Peer {
            peer: &peers[1],
            slug: "owner/repo"
        }
    );
    assert_eq!(
        route("01ARZ3NDEKTSV4RRFFQ69G5FAY/owner/repo", LOCAL_ID, &peers),
        Route::UnknownDaemon {
            daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY"
        }
    );
    assert_eq!(route("", LOCAL_ID, &peers), Route::Local { slug: "" });
    assert_eq!(
        route("01PEERA/repo", LOCAL_ID, &peers),
        Route::Local {
            slug: "01PEERA/repo"
        }
    );
}

#[test]
fn routing_is_independent_of_the_command_verb() {
    let peers = [peer(PEER_A_ID, "WSL: Ubuntu-22.04")];
    for _verb in dispatch::Verb::ALL {
        assert_eq!(
            route(&format!("{PEER_A_ID}/owner/repo"), LOCAL_ID, &peers),
            Route::Peer {
                peer: &peers[0],
                slug: "owner/repo"
            }
        );
    }
}

#[test]
fn unreachable_diagnosis_names_the_environment() {
    let peer = peer(PEER_A_ID, "WSL: Ubuntu-22.04");
    assert!(peer_unreachable(&peer, "connection refused").contains(&peer.environment));
}
