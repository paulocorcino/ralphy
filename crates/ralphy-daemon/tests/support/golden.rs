//! Shared replies (ADR-0070 Compliance): a test that produces a real reply
//! compares it with `crates/ralphy-daemon/ui-tests/fixtures/<name>.json`, the
//! file the UI tests feed to the real JS folds. `UPDATE_GOLDEN=1` writes the
//! file instead. Included by path from the daemon and the CLI tests, so it
//! needs nothing beyond `serde_json` (and `tempfile` in its own tests).

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The fixtures directory. Both including crates are siblings under `crates/`.
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ralphy-daemon/ui-tests/fixtures")
}

/// Compare `reply` with the shared file `<name>.json`, after each JSON pointer
/// in `volatile` is replaced by the marker `"<last segment>"`.
#[allow(dead_code)] // each including test binary uses one of the two entry points
pub fn check(name: &str, reply: Value, volatile: &[&str]) {
    check_in(&fixtures_dir(), name, reply, volatile);
}

fn check_in(dir: &Path, name: &str, mut reply: Value, volatile: &[&str]) {
    for pointer in volatile {
        let marker = format!("<{}>", pointer.rsplit('/').next().unwrap_or(pointer));
        let slot = reply
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("volatile pointer {pointer} is not in the {name} reply"));
        *slot = Value::String(marker);
    }
    let path = dir.join(format!("{name}.json"));
    if std::env::var_os("UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(dir).expect("creating the fixtures directory");
        let text = serde_json::to_string_pretty(&reply).expect("a Value always serializes");
        std::fs::write(&path, text + "\n")
            .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
        return;
    }
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("no shared reply {name}: run the test with UPDATE_GOLDEN=1"));
    let golden: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
    assert_eq!(
        reply, golden,
        "the {name} reply differs from {}; if the change is intended, run the test with UPDATE_GOLDEN=1",
        path.display()
    );
}

#[cfg(test)]
mod golden_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_reply_that_differs_from_its_file_panics() {
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.json"), r#"{"a":1}"#).unwrap();
        check_in(dir.path(), "t", json!({"a": 1}), &[]);
        let differs = std::panic::catch_unwind(|| check_in(dir.path(), "t", json!({"a": 2}), &[]));
        assert!(differs.is_err(), "a differing reply must fail the test");
        let missing = std::panic::catch_unwind(|| check_in(dir.path(), "none", json!({}), &[]));
        assert!(missing.is_err(), "a missing file must fail the test");
    }

    #[test]
    fn a_volatile_pointer_becomes_its_marker() {
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("t.json"),
            r#"{"a":{"sha":"<sha>"},"list":[{"at":"<at>"}]}"#,
        )
        .unwrap();
        check_in(
            dir.path(),
            "t",
            json!({"a": {"sha": "abc"}, "list": [{"at": "2026"}]}),
            &["/a/sha", "/list/0/at"],
        );
        let stale = std::panic::catch_unwind(|| check_in(dir.path(), "t", json!({}), &["/a/sha"]));
        assert!(
            stale.is_err(),
            "a pointer that resolves to nothing must fail"
        );
    }
}
