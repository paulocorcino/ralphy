//! A scratch workspace for the config tests.

use super::*;
use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};

static N: AtomicU32 = AtomicU32::new(0);

pub(super) fn tmp_ws(name: &str) -> (Workspace, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "ralphy-config-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        name
    ));
    fs::create_dir_all(&dir).unwrap();
    let ws = Workspace::new(&dir);
    (ws, dir)
}
