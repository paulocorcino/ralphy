//! End-to-end coverage for `ralphy blob read` (issue #311): drives the real
//! `ralphy` binary against an isolated temp git repo. The JSON shapes asserted
//! here are the wire contract the daemon's `blob.read` verb relays to the
//! workbench's diff tab, and the escape case is the real-containment boundary —
//! the daemon validates the path by shape, this process stands in the repo.

use std::path::Path;
use std::process::{Command, Output};

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run_git(root, &["init", "--quiet"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    dir
}

fn run_git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .expect("spawning git");
    assert!(status.success(), "git {args:?} failed");
}

fn blob_read(repo: &Path, path: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ralphy"))
        .args([
            "blob",
            "read",
            "--revision",
            "head",
            "--path",
            path,
            "--format",
            "json",
            "--repo",
            &repo.to_string_lossy(),
        ])
        .output()
        .expect("spawning ralphy")
}

#[test]
fn a_committed_file_reads_back_present_with_its_content() {
    let repo = init_repo();

    let out = blob_read(repo.path(), "README.md");
    assert!(out.status.success(), "blob read must succeed");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end_matches(['\r', '\n']),
        r#"{"status":"present","content":"hello\n"}"#
    );
}

#[test]
fn an_uncommitted_path_reads_back_absent() {
    let repo = init_repo();
    std::fs::write(repo.path().join("brand-new.txt"), "brand new line\n").unwrap();

    let out = blob_read(repo.path(), "brand-new.txt");
    assert!(
        out.status.success(),
        "absence is an answer, not a failure: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end_matches(['\r', '\n']),
        r#"{"status":"absent"}"#
    );
}

#[test]
fn a_committed_binary_is_refused_with_the_viewers_vocabulary() {
    let repo = init_repo();
    std::fs::write(repo.path().join("logo.png"), [0x89, b'P', 0x00, 0x01]).unwrap();
    run_git(repo.path(), &["add", "."]);
    run_git(repo.path(), &["commit", "--quiet", "-m", "add logo"]);

    let out = blob_read(repo.path(), "logo.png");
    assert!(out.status.success(), "a refusal is an answer, not an error");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end_matches(['\r', '\n']),
        r#"{"status":"refused","reason":"binary"}"#
    );
}

#[test]
fn a_path_leaving_the_repo_is_refused_before_any_read() {
    let repo = init_repo();
    // A real sibling file, so a leak would have something to leak.
    std::fs::write(repo.path().parent().unwrap().join("escape.txt"), "secret\n").unwrap();

    // One vector per arm of the guard — a single `..` case would leave the
    // absolute / rooted / drive-prefix arms deletable with the suite still green.
    for path in [
        "../escape.txt",
        "src/../../escape.txt",
        "/etc/passwd",
        "C:\\Windows\\win.ini",
        "..\\escape.txt",
        ".",
    ] {
        let out = blob_read(repo.path(), path);
        assert!(
            !out.status.success(),
            "{path:?} must exit non-zero, got stdout {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(
            out.stdout.is_empty(),
            "{path:?}: no JSON may reach stdout on a refusal: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("path escapes the repo"),
            "{path:?}: stderr must name the refusal: {stderr:?}"
        );
    }
}

/// Without `--format`, the command prints the raw bytes — the shape a human at a
/// terminal gets, and the branch the JSON tests never touch.
#[test]
fn without_format_the_raw_bytes_go_to_stdout() {
    let repo = init_repo();
    std::fs::write(repo.path().join("brand-new.txt"), "loose\n").unwrap();
    std::fs::write(repo.path().join("logo.png"), [0x89, b'P', 0x00, 0x01]).unwrap();
    run_git(repo.path(), &["add", "logo.png"]);
    run_git(repo.path(), &["commit", "--quiet", "-m", "add logo"]);

    let plain = |path: &str| {
        Command::new(env!("CARGO_BIN_EXE_ralphy"))
            .args([
                "blob",
                "read",
                "--revision",
                "head",
                "--path",
                path,
                "--repo",
                &repo.path().to_string_lossy(),
            ])
            .output()
            .expect("spawning ralphy")
    };

    let present = plain("README.md");
    assert!(present.status.success());
    assert_eq!(
        String::from_utf8_lossy(&present.stdout),
        "hello\n",
        "the bytes go out verbatim, with no added newline"
    );

    let absent = plain("brand-new.txt");
    assert!(absent.status.success(), "absence is not a failure");
    assert!(
        absent.stdout.is_empty(),
        "absent prints nothing: {:?}",
        String::from_utf8_lossy(&absent.stdout)
    );

    let binary = plain("logo.png");
    assert!(!binary.status.success(), "a refusal exits non-zero here");
    assert!(
        String::from_utf8_lossy(&binary.stderr).contains("binary"),
        "stderr names the refusal: {:?}",
        String::from_utf8_lossy(&binary.stderr)
    );
}
