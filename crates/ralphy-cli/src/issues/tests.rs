use super::*;
use ralphy_core::{Issue, Usage, CONSOLIDATED_SPEC_MARKER, STOP_BEFORE_LABEL};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Serializes the tests that set the process-global `RALPHY_USAGE_DIR`.
static USAGE_LOCK: Mutex<()> = Mutex::new(());

/// A read-only tracker: read methods answer from scripted state; every
/// mutating method bumps `mutations` (asserted to stay `0` — criterion #6).
#[derive(Default)]
struct FakeTracker {
    open: HashSet<u64>,
    comments: HashMap<u64, Vec<String>>,
    mutations: RefCell<usize>,
}

impl IssueTracker for FakeTracker {
    fn close(&self, _n: u64, _c: &str) -> Result<()> {
        *self.mutations.borrow_mut() += 1;
        Ok(())
    }
    fn write_evidence(&self, _n: u64, _b: &str, _v: &[ralphy_core::Verdict]) -> Result<()> {
        *self.mutations.borrow_mut() += 1;
        Ok(())
    }
    fn comment(&self, _n: u64, _b: &str) -> Result<()> {
        *self.mutations.borrow_mut() += 1;
        Ok(())
    }
    fn add_label(&self, _n: u64, _l: &str) -> Result<()> {
        *self.mutations.borrow_mut() += 1;
        Ok(())
    }
    fn remove_label(&self, _n: u64, _l: &str) -> Result<()> {
        *self.mutations.borrow_mut() += 1;
        Ok(())
    }
    fn create_issue(&self, _t: &str, _b: &str, _l: &[String]) -> Result<u64> {
        *self.mutations.borrow_mut() += 1;
        Ok(0)
    }
    fn upsert_marked_comment(&self, _n: u64, _m: &str, _b: &str) -> Result<()> {
        *self.mutations.borrow_mut() += 1;
        Ok(())
    }
    fn is_closed(&self, number: u64) -> Result<bool> {
        Ok(!self.open.contains(&number))
    }
    fn issue_comments(&self, number: u64) -> Result<Vec<String>> {
        Ok(self.comments.get(&number).cloned().unwrap_or_default())
    }
}

fn issue(number: u64, labels: &[&str], body: &str) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: body.to_string(),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        comments: Vec::new(),
    }
}

fn human() -> Vec<String> {
    ["needs-info", "wontfix"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn render_json_emits_full_key_set_and_fields_selects_subset() {
    let queue = vec![issue(7, &["queue"], "")];
    let tr = FakeTracker::default();
    let view = resolve_queue_view(&queue, &[], &human(), &tr).unwrap();

    // Full JSON: every issue carries exactly the seven contract keys.
    let json = render_json(&view, None).unwrap();
    let val: Value = serde_json::from_str(&json).unwrap();
    let obj = val[0].as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = vec![
        "number",
        "title",
        "labels",
        "queue_status",
        "skip_reason",
        "blocked_by",
        "position",
    ];
    expected.sort_unstable();
    assert_eq!(keys, expected);
    assert_eq!(obj["queue_status"], "eligible");

    // `--fields number,queue_status` yields ONLY those two keys.
    let fields = vec!["number".to_string(), "queue_status".to_string()];
    let json = render_json(&view, Some(&fields)).unwrap();
    let val: Value = serde_json::from_str(&json).unwrap();
    let obj = val[0].as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["number", "queue_status"]);
}

#[test]
fn render_text_shows_a_row_for_each_status() {
    let queue = vec![
        issue(1, &[STOP_BEFORE_LABEL], ""),
        issue(2, &["needs-info"], ""),
        issue(3, &[], "## Blocked by\n- #99\n"),
        issue(4, &[], ""),
    ];
    let mut tr = FakeTracker::default();
    tr.open.insert(99);
    let view = resolve_queue_view(&queue, &[], &human(), &tr).unwrap();
    let text = render_text(&view, None);
    // One line per issue, each carrying its status word and reason cell.
    assert!(
        text.contains("#1") && text.contains("stop_before"),
        "{text}"
    );
    assert!(
        text.contains("#2") && text.contains("skipped") && text.contains("needs-info"),
        "{text}"
    );
    assert!(
        text.contains("#3") && text.contains("blocked") && text.contains("by #99"),
        "{text}"
    );
    assert!(
        text.contains("#4") && text.contains("eligible") && text.contains("pos 1"),
        "{text}"
    );
    assert_eq!(text.lines().count(), 4, "one row per issue: {text}");
}

#[test]
fn render_text_empty_queue_names_active_filter() {
    let empty = QueueView {
        count: 0,
        order: vec![],
        stop_before: None,
        issues: vec![],
    };
    // Unfiltered: the plain notice.
    assert_eq!(render_text(&empty, None), "No open issues in the queue.");
    // Filtered: the notice names the assignee.
    let filtered = render_text(&empty, Some("@me"));
    assert!(filtered.contains("@me"), "got: {filtered}");
    assert!(filtered.contains("assigned to"), "got: {filtered}");
}

#[test]
fn show_view_json_carries_body_spec_labels_judgment_and_history() {
    let _g = USAGE_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("ralphy-issues-show-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("RALPHY_USAGE_DIR", &dir);

    // Seed the ledger: two rows for #7 (plan + execute) and an unrelated #8 row.
    let slug = "o/r";
    let rec = |issue: u64, phase: &str, out: u64| ralphy_core::ledger::LedgerRecord {
        project: slug.to_string(),
        actor_email: "t@example.com".into(),
        actor_name: "T".into(),
        ralphy_version: "0.0.0".into(),
        issue,
        phase: phase.to_string(),
        agent: "scripted".into(),
        model: "claude-opus-4".into(),
        session_id: None,
        outcome: "ok".into(),
        tokens: Usage {
            input: 100,
            output: out,
            cache_read: 0,
            cache_creation: 0,
            model: None,
        },
        ts: "2026-07-03T10:00:00Z".into(),
    };
    ralphy_core::ledger::append(&rec(7, "plan", 10)).unwrap();
    ralphy_core::ledger::append(&rec(7, "execute", 20)).unwrap();
    ralphy_core::ledger::append(&rec(8, "plan", 5)).unwrap();

    let history = issue_history(slug, 7);
    assert_eq!(history.len(), 2, "only #7's two rows");

    let issue = issue(7, &["queue"], "the issue body");
    let comments = vec![format!(
        "{CONSOLIDATED_SPEC_MARKER}\n## Consolidated spec\nthe real spec\n"
    )];
    let tr = FakeTracker::default();
    let view = show_view(&issue, &comments, &history, &human(), &tr).unwrap();
    let json = render_show_json(&view, None).unwrap();
    let val: Value = serde_json::from_str(&json).unwrap();

    assert_eq!(val["number"], 7);
    assert_eq!(val["body"], "the issue body");
    assert_eq!(val["labels"], serde_json::json!(["queue"]));
    assert_eq!(val["queue_status"], "eligible");
    // The detail view carries no list-relative `position`.
    assert!(
        val.get("position").is_none(),
        "show detail must not carry a position: {val}"
    );
    assert!(
        val["consolidated_spec"]
            .as_str()
            .unwrap()
            .contains("the real spec"),
        "consolidated_spec surfaced: {val}"
    );
    let hist = val["history"].as_array().unwrap();
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0]["phase"], "plan");
    assert_eq!(hist[0]["tokens"], 110); // 100 input + 10 output
    assert_eq!(hist[1]["phase"], "execute");
    assert_eq!(hist[1]["tokens"], 120);

    std::env::remove_var("RALPHY_USAGE_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn show_view_json_includes_comments() {
    let issue = issue(7, &["queue"], "the issue body");
    let comments = vec!["a comment".to_string()];
    let tr = FakeTracker::default();
    let view = show_view(&issue, &comments, &[], &human(), &tr).unwrap();
    let json = render_show_json(&view, None).unwrap();
    let val: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(val["body"], "the issue body");
    assert_eq!(val["comments"], serde_json::json!(["a comment"]));
}

#[test]
fn render_board_json_folds_whole_tracker_with_union_and_graph_order() {
    use ralphy_core::blocked::sort_queue_in_graph;
    use ralphy_core::github::BoardIssue;

    // Board row factory: a whole-tracker BoardIssue with the given state,
    // assignees, blocked_by, and (closed) reason.
    let bi =
        |number: u64, state: &str, assignees: &[&str], blocked_by: &[u64], reason: Option<&str>| {
            BoardIssue {
                number,
                title: format!("issue {number}"),
                state: state.to_string(),
                reason: reason.map(str::to_string),
                labels: vec!["ready-for-agent".to_string()],
                assignees: assignees.iter().map(|s| s.to_string()).collect(),
                blocked_by: blocked_by.to_vec(),
                created: "2026-07-01T00:00:00Z".to_string(),
                updated: "2026-07-02T00:00:00Z".to_string(),
            }
        };

    // A ready chain (#7 blocked by #8) → the graph order the fold must preserve.
    let ready_q = vec![issue(7, &[], "## Blocked by\n- #8\n"), issue(8, &[], "")];
    let ready_sorted = sort_queue_in_graph(ready_q.clone(), &ready_q);
    let ready_order: Vec<u64> = ready_sorted.iter().map(|i| i.number).collect();
    assert_eq!(ready_order, vec![8, 7], "#8 (root) precedes #7");

    // Whole open tracker: the two ready issues + a backlog issue + one assigned to
    // someone-else + one assigned to `octo`. One recent-closed row.
    let open = vec![
        bi(7, "open", &[], &[8], None),
        bi(8, "open", &[], &[], None),
        bi(30, "open", &[], &[], None),
        bi(31, "open", &["someone-else"], &[], None),
        bi(32, "open", &["octo"], &[], None),
    ];
    let closed = vec![bi(99, "closed", &[], &[], Some("not_planned"))];
    let repo_labels = vec![("ready-for-agent".to_string(), "0e8a16".to_string())];

    let nums = |val: &Value| -> Vec<u64> {
        val["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["number"].as_u64().unwrap())
            .collect()
    };

    // (a)+(b) default (login=None): Ready subset first in graph order, then the
    // remaining UNASSIGNED open (#30), then closed (#99). The someone-else (#31)
    // and octo (#32) rows are dropped — only empty-assignees rows survive.
    let json = render_board_json(&ready_order, &open, &closed, None, &repo_labels).unwrap();
    let val: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        nums(&val),
        vec![8, 7, 30, 99],
        "default hides assigned: {val}"
    );

    // (d) the closed row carries state + lowercased reason.
    let closed_row = val["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["number"] == 99)
        .unwrap();
    assert_eq!(closed_row["state"], "closed");
    assert_eq!(closed_row["reason"], "not_planned");

    // (e) labels[] carries the repo {name,color} vocabulary.
    assert!(
        val["labels"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({"name":"ready-for-agent","color":"0e8a16"})),
        "labels must carry the repo vocabulary with colors: {val}"
    );

    // (c) a configured login keeps unassigned OR that-login (#32 survives), while
    // an issue assigned only to someone-else (#31) stays dropped.
    let json2 =
        render_board_json(&ready_order, &open, &closed, Some("octo"), &repo_labels).unwrap();
    let val2: Value = serde_json::from_str(&json2).unwrap();
    let n2 = nums(&val2);
    assert!(
        n2.contains(&32),
        "octo-assigned survives under login: {val2}"
    );
    assert!(!n2.contains(&31), "someone-else stays dropped: {val2}");
    assert_eq!(n2, vec![8, 7, 30, 32, 99]);
}

#[test]
fn push_without_events_url_errors_naming_events_url() {
    // Criterion: `--push` with no `events.url` configured fails with a clear
    // message naming `events.url`. Point the events store at an empty temp dir
    // and drive `push_snapshot` directly (no `gh` needed).
    let _g = crate::events::config::ENV_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("ralphy-issues-push-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("RALPHY_EVENTS_DIR", &dir);

    let queue = vec![issue(7, &["queue"], "")];
    let tr = FakeTracker::default();
    let view = resolve_queue_view(&queue, &[], &human(), &tr).unwrap();

    let err = push_snapshot(std::path::Path::new("."), &view, None).unwrap_err();
    assert!(
        err.to_string().contains("events.url"),
        "error must name events.url: {err}"
    );

    std::env::remove_var("RALPHY_EVENTS_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_and_show_never_mutate_the_tracker() {
    // Criterion #6: the surface is read-only. Drive both the list resolution
    // (over a blocked issue, exercising is_closed/open_children/issue_labels)
    // and the show view (exercising issue_comments), then assert zero mutations.
    let mut tr = FakeTracker::default();
    tr.open.insert(99);
    tr.comments.insert(7, vec!["a comment".into()]);

    let queue = vec![
        issue(3, &[], "## Blocked by\n- #99\n"),
        issue(7, &["queue"], ""),
    ];
    let _ = resolve_queue_view(&queue, &[], &human(), &tr).unwrap();

    let issue = issue(7, &["queue"], "body");
    let comments = tr.issue_comments(7).unwrap();
    let _ = show_view(&issue, &comments, &[], &human(), &tr).unwrap();

    assert_eq!(
        *tr.mutations.borrow(),
        0,
        "the query surface must never mutate the tracker"
    );
}
