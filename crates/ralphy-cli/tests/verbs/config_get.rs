//! `ralphy config get` end to end: a `gemini` section stored by an older version
//! is ignored, with one warning on stderr in text mode.

use super::support::run_git;

#[test]
fn config_get_warns_once_on_stderr_about_a_stored_gemini_section() {
    let repo = tempfile::tempdir().expect("creating a temporary directory");
    run_git(repo.path(), &["init", "--quiet"]);
    let ralphy_dir = repo.path().join(".ralphy");
    std::fs::create_dir_all(&ralphy_dir).expect("creating a directory");
    std::fs::write(
        ralphy_dir.join("settings.json"),
        r#"{"gemini":{"plan_model":"gemini-2.5-pro"}}"#,
    )
    .expect("writing a file");

    let out = crate::support::ralphy_command()
        .args(["config", "get"])
        .current_dir(repo.path())
        .output()
        .expect("spawning ralphy");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    assert_eq!(stderr.matches("warning:").count(), 1, "{stderr}");
    assert!(stderr.contains("antigravity.plan_model"), "{stderr}");
    assert!(stderr.contains("antigravity.exec_model"), "{stderr}");
}
