use super::*;

fn reply(id: u64, status: &str) -> Vec<u8> {
    protocol::encode(&Frame::Command(Command {
        id,
        verb: "run".to_string(),
        payload: serde_json::json!({ "status": status }),
    }))
}

#[test]
fn only_the_exit_or_error_of_the_same_command_ends_a_peer_relay() {
    assert!(is_terminal_reply(&reply(3, "exited"), 3));
    assert!(is_terminal_reply(&reply(3, "error"), 3));
    assert!(
        !is_terminal_reply(&reply(4, "exited"), 3),
        "the exit of another command"
    );
    assert!(!is_terminal_reply(&reply(4, "error"), 3));
    assert!(!is_terminal_reply(&reply(3, "spawned"), 3));
    assert!(!is_terminal_reply(&reply(3, "output"), 3));
    assert!(!is_terminal_reply(&[0xFF, 0x00], 3), "undecodable bytes");
}
