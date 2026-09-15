use std::fs;
use std::path::Path;
use std::time::Duration;

use super::*;

fn seed(root: &Path) {
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::create_dir_all(root.join("build")).unwrap();
    fs::create_dir_all(root.join(".ralphy/runs")).unwrap();
    fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    fs::create_dir_all(root.join(".git/objects")).unwrap();
    fs::write(root.join(".gitignore"), "build/\n.ralphy/\n").unwrap();
    fs::write(
        root.join("src/main.rs"),
        "fn main() { println!(\"Task one\"); }\n",
    )
    .unwrap();
    fs::write(root.join("src/deep/task_list.md"), "task, TASK, Task.\n").unwrap();
    fs::write(root.join("build/out.js"), "task task task\n").unwrap();
    fs::write(root.join(".ralphy/plan.md"), "# plan\nthe task ahead\n").unwrap();
    fs::write(root.join("node_modules/pkg/index.js"), "task\n").unwrap();
    fs::write(root.join(".git/objects/blob"), "task\n").unwrap();
    fs::write(root.join("logo.bin"), b"task\0task").unwrap();
    fs::write(root.join("README.md"), "nothing here\n").unwrap();
}

fn paths<H>(reply: &SearchReply<H>, f: impl Fn(&H) -> &str) -> Vec<&str> {
    reply.hits.iter().map(f).collect()
}

#[test]
fn find_matches_names_with_the_trees_policy() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let reply = find(dir.path(), "TASK", &SearchBudget::default()).unwrap();
    // Gitignored (`build/`, `.ralphy/`) entries are found — the tree shows them —
    // while the noise dirs are never walked. Rel paths are `/`-joined on every
    // host, which is what `relPath` in the workbench produces.
    assert_eq!(
        paths(&reply, |h| h.path.as_str()),
        vec!["src/deep/task_list.md"]
    );
    assert!(!reply.truncated);
    // NEGATIVE CONTROL: `node_modules/pkg` would have matched by name.
    let reply = find(dir.path(), "pkg", &SearchBudget::default()).unwrap();
    assert!(reply.hits.is_empty(), "{:?}", reply.hits);
}

#[test]
fn find_lists_directories_first_and_marks_them() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("zeta_x")).unwrap();
    fs::write(dir.path().join("alpha_x.txt"), "").unwrap();
    let reply = find(dir.path(), "_x", &SearchBudget::default()).unwrap();
    // Dirs first even when the alphabet says otherwise — the tree's own order.
    assert_eq!(
        reply.hits,
        vec![
            FindHit {
                path: "zeta_x".into(),
                dir: true
            },
            FindHit {
                path: "alpha_x.txt".into(),
                dir: false
            },
        ]
    );
}

#[test]
fn find_refuses_a_query_shorter_than_the_floor() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let reply = find(dir.path(), "t", &SearchBudget::default()).unwrap();
    assert!(reply.hits.is_empty());
    let reply = find(dir.path(), "  ", &SearchBudget::default()).unwrap();
    assert!(reply.hits.is_empty());
}

#[test]
fn find_stops_at_the_hit_cap_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let budget = SearchBudget {
        max_hits: 1,
        deadline: Duration::from_secs(5),
    };
    let reply = find(dir.path(), "md", &budget).unwrap();
    assert_eq!(reply.hits.len(), 1);
    assert!(reply.truncated);
}

#[test]
fn find_stops_at_the_deadline_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let budget = SearchBudget {
        max_hits: 200,
        deadline: Duration::ZERO,
    };
    let reply = find(dir.path(), "task", &budget).unwrap();
    assert!(reply.hits.is_empty());
    assert!(reply.truncated);
}

#[test]
fn grep_respects_gitignore_but_always_searches_ralphy() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let reply = grep(dir.path(), "task", &SearchBudget::default()).unwrap();
    let hits: Vec<(&str, u32)> = reply
        .hits
        .iter()
        .map(|h| (h.path.as_str(), h.count))
        .collect();
    // `build/out.js` is ignored → absent; `.ralphy/plan.md` is ignored too →
    // present, the one exception; the binary is skipped; the count is
    // occurrences, case-insensitive, not lines.
    assert_eq!(
        hits,
        vec![
            (".ralphy/plan.md", 1),
            ("src/deep/task_list.md", 3),
            ("src/main.rs", 1),
        ]
    );
    assert!(!reply.truncated);
}

#[test]
fn grep_is_literal_not_a_pattern() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    // A regex would read `.` as "any char" and match every file; the literal
    // matches only the one line that has "Task." in it.
    let reply = grep(dir.path(), "task.", &SearchBudget::default()).unwrap();
    assert_eq!(
        paths(&reply, |h| h.path.as_str()),
        vec!["src/deep/task_list.md"]
    );
    let reply = grep(dir.path(), "main() {", &SearchBudget::default()).unwrap();
    assert_eq!(paths(&reply, |h| h.path.as_str()), vec!["src/main.rs"]);
}

#[test]
fn grep_skips_an_oversized_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("small.txt"), "needle\n").unwrap();
    let big = "needle".repeat((MAX_READ_BYTES as usize / 6) + 1);
    fs::write(dir.path().join("big.txt"), big).unwrap();
    let reply = grep(dir.path(), "needle", &SearchBudget::default()).unwrap();
    assert_eq!(paths(&reply, |h| h.path.as_str()), vec!["small.txt"]);
}

#[test]
fn grep_stops_at_the_hit_cap_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let budget = SearchBudget {
        max_hits: 1,
        deadline: Duration::from_secs(5),
    };
    let reply = grep(dir.path(), "task", &budget).unwrap();
    assert_eq!(reply.hits.len(), 1);
    assert!(reply.truncated);
}

#[test]
fn grep_without_a_ralphy_dir_is_the_main_walk_alone() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
    let reply = grep(dir.path(), "needle", &SearchBudget::default()).unwrap();
    assert_eq!(paths(&reply, |h| h.path.as_str()), vec!["a.txt"]);
}

#[test]
fn searches_refuse_a_missing_root() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    assert!(find(&missing, "task", &SearchBudget::default()).is_err());
    assert!(grep(&missing, "task", &SearchBudget::default()).is_err());
}
