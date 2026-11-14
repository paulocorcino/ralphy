//! The daemon's replies carry a failure as a catalog entry (#685). A reply
//! with free error text is a site that has not moved yet. The scan reads
//! `crates/ralphy-daemon/src` outside tests and counts these patterns:
//! - a JSON key `"message"`, `"reason"` or `"error"`;
//! - a call to `failed(` in `routes/api_desk.rs`;
//! - a call to `Refusal::refuse(` or `refused(`;
//! - a `(StatusCode::X, "..."` or `(StatusCode::X, format!` body.
//!
//! Test files (`tests/`, `tests.rs`, `*_test_child.rs`) and inline
//! `#[cfg(test)] mod` blocks are not read, nor are comment lines. The count
//! may only go down: a site that moves lowers the baseline.

use super::*;

/// The sites found, measured at the integrated tip of #685.
const FREE_TEXT_SITES: usize = 186;

fn is_test_file(path: &Path) -> bool {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let in_tests = path.components().any(|c| c.as_os_str() == "tests");
    in_tests || stem == "tests" || stem.ends_with("_tests") || stem.ends_with("_test_child")
}

/// The text before an inline `#[cfg(test)] mod x {`, without comment lines.
fn production(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut code = String::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let next = lines.get(i + 1).map_or("", |l| l.trim_start());
        if trimmed.starts_with("#[cfg(test)]") && next.starts_with("mod ") && next.ends_with('{') {
            break;
        }
        if !trimmed.starts_with("//") {
            code.push_str(line);
            code.push('\n');
        }
    }
    code
}

/// The sites of one source; `desk` is true for `routes/api_desk.rs`.
fn sites(src: &str, desk: bool) -> usize {
    let key = Regex::new(r#""(?:message|reason|error)"\s*:"#).expect("fixed");
    let refuse = Regex::new(r"Refusal::refuse\(|\brefused\(").expect("fixed");
    let status = Regex::new(r#"\(StatusCode::\w+,\s*(?:"|format!)"#).expect("fixed");
    let failed = Regex::new(r"\bfailed\(").expect("fixed");
    let mut count = 0;
    for line in production(src).lines() {
        if line.contains("fn ") {
            continue;
        }
        count += key.find_iter(line).count();
        count += refuse.find_iter(line).count();
        count += status.find_iter(line).count();
        if desk {
            count += failed.find_iter(line).count();
        }
    }
    count
}

#[test]
fn the_free_text_counter_reads_each_pattern_and_skips_tests() {
    let src = concat!(
        "fn f() {
 json!({\"error\": e, \"reason\": r});
 Refusal::refuse(a, b);
 ",
        "(StatusCode::BAD_REQUEST, \"no\");
 (StatusCode::OK, format!(\"x\"));
 ",
        "// \"message\": 1
 failed(a, b);
}
#[cfg(test)]
mod t {
 ",
        "json!({\"error\": 1});
}
"
    );
    assert_eq!(sites(src, false), 5);
    assert_eq!(sites(src, true), 6);
    assert!(is_test_file(Path::new("a/tests/x.rs")));
    assert!(is_test_file(Path::new("a/dir/tests.rs")));
    assert!(!is_test_file(Path::new("a/dir/dispatch.rs")));
}

#[test]
fn failure_free_text_matches_the_baseline() {
    let src_dir = workspace_root().join("crates/ralphy-daemon/src");
    let mut files = Vec::new();
    skipped_tests::rust_sources(&src_dir, &mut files);
    let total: usize = files
        .iter()
        .filter(|f| !is_test_file(f))
        .map(|f| sites(&read(f), f.ends_with("routes/api_desk.rs")))
        .sum();
    assert_eq!(
        total, FREE_TEXT_SITES,
        "a reply moved to a failure (lower FREE_TEXT_SITES) or a new free text was added (do not)"
    );
}
