//! `ralphy install --print-path`: the install scripts ask the binary where it
//! goes, so the default folder is decided in one place. It changes nothing.

use std::process::Command;

#[test]
fn print_path_names_the_binary_and_writes_nothing() {
    let scratch = tempfile::tempdir().expect("scratch");
    let dir = scratch.path().join("bin");
    let out = Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args(["install", "--print-path", "--dir"])
        .arg(&dir)
        .output()
        .expect("running ralphy install --print-path");
    assert!(out.status.success(), "{out:?}");
    let name = if cfg!(windows) {
        "ralphy.exe"
    } else {
        "ralphy"
    };
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        dir.join(name).display().to_string()
    );
    assert!(!dir.exists(), "--print-path created the folder");
}
