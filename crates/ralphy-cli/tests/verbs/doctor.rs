//! `ralphy doctor` end to end: on a computer with none of the tools, the real
//! binary prints its report, exits 1, and prints no second error line under
//! it. The install scripts end on this report.

fn doctor(args: &[&str]) -> (Option<i32>, String, String) {
    let empty = tempfile::tempdir().expect("an empty folder");
    let out = crate::support::ralphy_command()
        .arg("doctor")
        .args(args)
        // No tool on PATH, and a home with no `~/.local/bin` to fall back on.
        .env("PATH", empty.path())
        .env("HOME", empty.path())
        .env("USERPROFILE", empty.path())
        // A scratch store: the Telegram config doctor reads is the empty
        // store's, and no daemon of the developer answers. On Windows the
        // default config folder does not follow `USERPROFILE`.
        .env("RALPHY_DAEMON_DIR", empty.path())
        .output()
        .expect("running ralphy doctor");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_missing_tool_exits_1_under_one_report() {
    let (code, stdout, stderr) = doctor(&[]);
    assert_eq!(code, Some(1), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("[miss] git:"), "{stdout}");
    assert!(stdout.contains("required tool(s) missing"), "{stdout}");
    assert!(!stderr.contains("Error:"), "a second error line: {stderr}");
}

#[test]
fn the_json_report_exits_1_too() {
    let (code, stdout, stderr) = doctor(&["--json"]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    let report: serde_json::Value = serde_json::from_str(stdout.trim()).expect("one JSON line");
    let git = &report["tools"][0];
    assert_eq!(git["tool"], "git");
    assert_eq!(git["ok"], false);
    assert!(!stderr.contains("Error:"), "{stderr}");
}
