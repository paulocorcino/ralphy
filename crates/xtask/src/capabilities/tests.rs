use std::path::Path;
use std::process::Command;

use super::rules::{line_findings, lock_crates, path_findings};
use super::*;

fn caps(path: &str, text: &str) -> Vec<Capability> {
    line_findings(path, 1, text)
        .0
        .into_iter()
        .map(|f| f.cap)
        .collect()
}

fn file(path: &str) -> FileDiff {
    FileDiff {
        path: path.to_string(),
        ..FileDiff::default()
    }
}

#[test]
fn parse_diff_keeps_added_lines_with_their_numbers() {
    let diff = "\
diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -3,0 +4,2 @@ fn x() {
+let a = 1;
+let b = 2;
@@ -10 +12 @@
-old
+new
diff --git a/icon.png b/icon.png
Binary files a/icon.png and b/icon.png differ
";
    let files = parse_diff(diff);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "src/a.rs");
    assert_eq!(
        files[0].added,
        vec![
            (4, "let a = 1;".to_string()),
            (5, "let b = 2;".to_string()),
            (12, "new".to_string())
        ]
    );
    assert_eq!(files[0].removed, ["old"]);
    assert!(files[1].binary);
    assert_eq!(files[1].path, "icon.png");
}

#[test]
fn parse_diff_reads_a_triple_plus_inside_a_hunk_as_an_added_line() {
    let diff = "\
diff --git a/a.md b/a.md
--- a/a.md
+++ b/a.md
@@ -0,0 +1,2 @@
+++ counter
+--- rule
";
    let files = parse_diff(diff);
    assert_eq!(
        files[0].added,
        [(1, "++ counter".to_string()), (2, "--- rule".to_string())]
    );
}

#[test]
fn hunk_start_reads_the_new_side() {
    assert_eq!(hunk_start("-3,2 +4,5 @@ fn x"), Some(4));
    assert_eq!(hunk_start("-10 +12 @@"), Some(12));
    assert_eq!(hunk_start("garbage"), None);
}

#[test]
fn rust_lines_that_add_a_power_are_found() {
    assert_eq!(
        caps("a.rs", "let r = ureq::get(url);"),
        [Capability::Network]
    );
    assert_eq!(caps("a.rs", "Command::new(\"sh\")"), [Capability::Process]);
    assert_eq!(caps("a.rs", "unsafe { ptr.read() }"), [Capability::Unsafe]);
    assert_eq!(
        caps("a.rs", "std::env::var(\"GH_TOKEN\")"),
        [Capability::Credential]
    );
    assert_eq!(
        caps("a.rs", "let p = home.join(\".ssh/id_rsa\");"),
        [Capability::Credential]
    );
}

#[test]
fn ordinary_lines_are_not_found() {
    assert!(caps("a.rs", "let unsafe_count = 0; // not unsafe code").is_empty());
    assert!(caps("a.rs", "std::env::var(\"HOME\")").is_empty());
    assert!(caps("a.md", "Command::new is described in the docs").is_empty());
}

#[test]
fn javascript_dynamic_code_is_found() {
    assert_eq!(
        caps("ui/app.js", "eval(payload)"),
        [Capability::DynamicCode]
    );
    assert_eq!(
        caps("ui/app.js", "const f = new Function(src);"),
        [Capability::DynamicCode]
    );
    assert_eq!(caps("ui/index.html", "atob(x)"), [Capability::DynamicCode]);
    assert!(caps("a.rs", "eval(payload)").is_empty());
}

#[test]
fn encoded_and_minified_text_is_found() {
    let blob = "QUJD".repeat(25);
    assert_eq!(
        caps("a.rs", &format!("const X: &str = \"{blob}\";")),
        [Capability::Encoded]
    );
    let long = "a+b;".repeat(300);
    assert_eq!(caps("ui/app.js", &long), [Capability::Obfuscated]);
    assert!(caps("notes.md", &"word ".repeat(300)).is_empty());
    // A long string of prose in Rust holds more spaces than minified code.
    let prose = format!(
        "const RULE: &str = \"{}\";",
        "one rule, `code`. ".repeat(80)
    );
    assert!(caps("a.rs", &prose).is_empty());
}

#[test]
fn vendored_code_is_reported_once_by_path_not_by_line() {
    assert!(caps("ui/vendor/lib.min.js", "eval(x)").is_empty());
    let found: Vec<_> = path_findings(&file("crates/d/assets/ui/vendor/lib.min.js"))
        .into_iter()
        .map(|f| f.cap)
        .collect();
    assert_eq!(found, [Capability::Vendored]);
}

#[test]
fn sensitive_paths_are_found() {
    let cap = |p: &str| -> Vec<Capability> {
        path_findings(&file(p)).into_iter().map(|f| f.cap).collect()
    };
    assert_eq!(cap(".github/workflows/ci.yml"), [Capability::Workflow]);
    assert_eq!(cap("crates/x/build.rs"), [Capability::BuildScript]);
    assert_eq!(cap("assets/prompts/execute.md"), [Capability::AgentText]);
    assert_eq!(cap("deny.toml"), [Capability::SecurityPolicy]);
    assert_eq!(
        cap("crates/xtask/src/capabilities/rules.rs"),
        [Capability::SecurityPolicy]
    );
    assert!(cap("crates/x/src/lib.rs").is_empty());
    assert_eq!(cap("web/package-lock.json"), [Capability::Dependency]);
}

#[test]
fn lockfile_lines_are_not_read_one_by_one() {
    let integrity = format!("\"integrity\": \"sha512-{}\"", "QUJD".repeat(25));
    assert!(caps("web/package-lock.json", &integrity).is_empty());
    let (_, hosts) = line_findings("web/package-lock.json", 1, "https://registry.npmjs.org/x");
    assert!(hosts.is_empty());
}

#[test]
fn url_hosts_are_collected() {
    let (_, hosts) = line_findings(
        "a.rs",
        1,
        "get(\"https://Evil.example.net/x\") and wss://a.io/",
    );
    assert_eq!(hosts, ["evil.example.net", "a.io"]);
}

#[test]
fn lock_crates_reads_package_names() {
    let lock = "[[package]]\nname = \"anyhow\"\nversion = \"1\"\n\n[[package]]\nname = \"ureq\"\n";
    let names: Vec<_> = lock_crates(lock).into_iter().collect();
    assert_eq!(names, ["anyhow", "ureq"]);
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .expect("git is installed on every machine that runs the xtask tests");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn scan_reports_new_hosts_and_crates_but_not_known_ones() {
    let tmp = tempfile::tempdir().expect("a temp dir can be created");
    let dir = tmp.path();
    run_git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("Cargo.lock"), "[[package]]\nname = \"anyhow\"\n").expect("write lock");
    std::fs::write(dir.join("a.rs"), "// https://known.example.com\n").expect("write a.rs");
    std::fs::write(dir.join("old.rs"), "    Command::new(\"git\")\n").expect("write old.rs");
    run_git(dir, &["add", "."]);
    run_git(dir, &["commit", "-q", "-m", "base"]);
    run_git(dir, &["checkout", "-q", "-b", "pr"]);
    std::fs::write(
        dir.join("Cargo.lock"),
        "[[package]]\nname = \"anyhow\"\n\n[[package]]\nname = \"sneaky\"\n",
    )
    .expect("write lock");
    std::fs::write(
        dir.join("a.rs"),
        "// https://known.example.com\nlet u = \"https://known.example.com/x\";\nlet v = \"https://new.example.org/y\";\n",
    )
    .expect("write a.rs");
    // The subprocess moves to a new file with a new indent: not a new capability.
    std::fs::remove_file(dir.join("old.rs")).expect("remove old.rs");
    std::fs::write(
        dir.join("new.rs"),
        "fn f() {\n        Command::new(\"git\")\n}\n",
    )
    .expect("write new.rs");
    run_git(dir, &["add", "-A"]);
    run_git(dir, &["commit", "-q", "-m", "pr"]);

    let found = scan(dir, "main", "pr").expect("scan runs");
    let details: Vec<_> = found.iter().map(|f| (f.cap, f.detail.as_str())).collect();
    assert_eq!(
        details,
        [
            (
                Capability::Network,
                "a URL host the repository did not name before: `new.example.org`"
            ),
            (
                Capability::Dependency,
                "adds the crate `sneaky` to Cargo.lock"
            ),
        ]
    );
    assert_eq!(found[0].line, Some(3));
}
