use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};

use super::*;

/// Whether `bytes` holds `ConPTY`'s startup cursor position request.
fn asks_for_the_cursor(bytes: &[u8]) -> bool {
    bytes
        .windows(CURSOR_POSITION_REQUEST.len())
        .any(|w| w == CURSOR_POSITION_REQUEST)
}

/// The first attach of a console launched with a notice: what it replays,
/// then what it receives live, holds the notice before the child's output.
#[tokio::test]
async fn the_notice_is_the_first_thing_an_attach_replays() {
    let mut spec = console_spec(std::env::temp_dir(), 24, 80, Some("echo CHILD-OUTPUT"));
    spec.start_notice = Some("START-NOTICE\r\n".to_owned());
    let manager = Arc::new(SessionManager::new());
    let (_id, mut first) = manager
        .spawn_attached(
            "~".to_owned(),
            "console".to_owned(),
            "console".to_owned(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn");
    let mut seen = String::from_utf8_lossy(&first.snapshot).into_owned();
    if asks_for_the_cursor(&first.snapshot) {
        first
            .write(CURSOR_POSITION_REPLY)
            .expect("the reply is written");
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while !seen.contains("CHILD-OUTPUT") {
        let next = tokio::time::timeout_at(deadline, first.rx.recv()).await;
        let Ok(next) = next else {
            panic!("no child output in 30 s: {seen:?}");
        };
        match next {
            Ok(chunk) => {
                if asks_for_the_cursor(&chunk) {
                    first
                        .write(CURSOR_POSITION_REPLY)
                        .expect("the reply is written");
                }
                seen.push_str(&String::from_utf8_lossy(&chunk));
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    let notice = seen
        .find("START-NOTICE")
        .expect("the attach shows the notice");
    let child = seen
        .find("CHILD-OUTPUT")
        .expect("the attach shows the child's output");
    assert!(notice < child, "the notice comes first: {seen:?}");
    let first_child = seen.find("CHILD").expect("the child's output is there");
    assert!(notice < first_child, "no child byte precedes the notice");
}
