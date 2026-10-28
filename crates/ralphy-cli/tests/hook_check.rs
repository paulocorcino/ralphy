//! The daemon's start check against the real `ralphy hook status`.

use std::path::Path;
use std::time::Duration;

use ralphy_proc_util::hook_form::{check, hook_for_version, HookCheck};

#[test]
fn the_start_check_passes_with_the_real_ralphy_binary() {
    let home = tempfile::tempdir().expect("temp dir");
    let exe = Path::new(env!("CARGO_BIN_EXE_ralphy"));
    let hook = hook_for_version(Some("2.1.139 (Claude Code)"), exe, &["hook", "status"]);
    let got = check(&hook, Duration::from_secs(30), Some(home.path()));
    assert_eq!(got, HookCheck::Ok);
}
