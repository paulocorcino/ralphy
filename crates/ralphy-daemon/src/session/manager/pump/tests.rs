use std::time::Duration;

use ralphy_pty::{CURSOR_POSITION_REPLY, CURSOR_POSITION_REQUEST};

use super::*;
use crate::session::console_spec;

/// A child that exits on its own while the manager that spawned it is gone:
/// no list is left to remove it from, and the writer is still told the
/// session ended.
#[tokio::test]
async fn a_child_that_outlives_its_manager_still_ends_its_writer() {
    let manager = Arc::new(SessionManager::new());
    let spec = console_spec(std::env::temp_dir(), 24, 80, Some("exit 0"));
    let (_id, mut writer) = manager
        .spawn_attached(
            "~".to_string(),
            "console".to_string(),
            "console".to_string(),
            None,
            None,
            None,
            spec,
        )
        .expect("the platform shell must spawn — the free console depends on it");
    // The test runtime has one thread, so the pump has not run yet: the
    // manager is gone before the pump can see the child exit.
    drop(manager);
    let ended = tokio::time::timeout(Duration::from_secs(30), async {
        while writer.evict.reason().is_none() {
            // ConPTY asks for the cursor position before the child runs.
            let chunk = tokio::time::timeout(Duration::from_millis(50), writer.rx.recv()).await;
            if let Ok(Ok(chunk)) = chunk {
                if chunk
                    .windows(CURSOR_POSITION_REQUEST.len())
                    .any(|w| w == CURSOR_POSITION_REQUEST)
                {
                    writer
                        .write(CURSOR_POSITION_REPLY)
                        .expect("the reply is written");
                }
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the writer was never told the session ended");
    assert_eq!(writer.evict.reason(), Some(EndReason::ChildExited));
}
