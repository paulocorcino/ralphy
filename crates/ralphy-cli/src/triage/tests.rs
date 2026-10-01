use super::*;
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use ralphy_core::DraftIssue;

use crate::runlock::LockInfo;

/// Hand-rolled unique temp dir (same idiom as `tmp_lock` in runlock.rs).
fn tmp_lock(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ralphy-triage-lock-{}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        name
    ));
    fs::create_dir_all(&dir).unwrap();
    dir.join("run.lock")
}

#[test]
fn presence_gate_defers_when_held_alive_and_if_idle() {
    let path = tmp_lock("defer");
    let info = LockInfo {
        pid: 4_000_000,
        started_at: "2026-07-02T10:00:00-03:00".into(),
    };
    fs::write(&path, serde_json::to_string(&info).unwrap()).unwrap();
    match presence_gate(&path, true, |pid| pid == 4_000_000) {
        PresenceGate::Defer(msg) => {
            assert_eq!(
                msg,
                "skipped: run in progress since 2026-07-02 10:00:00, pid 4000000"
            );
        }
        PresenceGate::Proceed { .. } => panic!("expected Defer"),
    }
}

#[test]
fn presence_gate_warns_and_proceeds_when_held_alive() {
    let path = tmp_lock("warn");
    let info = LockInfo {
        pid: 4_000_000,
        started_at: "2026-07-02T10:00:00-03:00".into(),
    };
    fs::write(&path, serde_json::to_string(&info).unwrap()).unwrap();
    match presence_gate(&path, false, |_| true) {
        PresenceGate::Proceed { warn: Some(w) } => assert_eq!(
            w,
            "a run is already active in this repo — proceeding anyway (pid 4000000)"
        ),
        other => panic!("expected Proceed with a warning, got {other:?}"),
    }
}

#[test]
fn presence_gate_takes_over_stale_lock() {
    let path = tmp_lock("stale");
    let info = LockInfo {
        pid: 4_000_001,
        started_at: "2026-07-02T10:00:00-03:00".into(),
    };
    fs::write(&path, serde_json::to_string(&info).unwrap()).unwrap();
    match presence_gate(&path, false, |_| false) {
        PresenceGate::Proceed { warn: Some(w) } => {
            assert_eq!(w, "ignoring stale run.lock (pid 4000001 not running)")
        }
        other => panic!("expected Proceed with a warning, got {other:?}"),
    }
}

#[test]
fn presence_gate_proceeds_when_free() {
    let path = tmp_lock("free");
    match presence_gate(&path, false, |_| true) {
        PresenceGate::Proceed { warn: None } => {}
        other => panic!("expected Proceed with no warning, got {other:?}"),
    }
}

#[derive(Default)]
struct RecordingTracker {
    added: RefCell<Vec<(u64, String)>>,
    removed: RefCell<Vec<(u64, String)>>,
    comments: RefCell<Vec<(u64, String)>>,
    upserts: RefCell<Vec<(u64, String, String)>>,
    created: RefCell<Vec<(String, String)>>,
}

impl IssueTracker for RecordingTracker {
    fn close(&self, _number: u64, _comment: &str) -> Result<()> {
        Ok(())
    }
    fn is_closed(&self, _number: u64) -> Result<bool> {
        Ok(true)
    }
    fn add_label(&self, number: u64, label: &str) -> Result<()> {
        self.added.borrow_mut().push((number, label.to_string()));
        Ok(())
    }
    fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        self.removed.borrow_mut().push((number, label.to_string()));
        Ok(())
    }
    fn comment(&self, number: u64, body: &str) -> Result<()> {
        self.comments.borrow_mut().push((number, body.to_string()));
        Ok(())
    }
    fn upsert_marked_comment(&self, number: u64, marker: &str, body: &str) -> Result<()> {
        self.upserts
            .borrow_mut()
            .push((number, marker.to_string(), body.to_string()));
        Ok(())
    }
    fn create_issue(&self, title: &str, body: &str, _labels: &[String]) -> Result<u64> {
        self.created
            .borrow_mut()
            .push((title.to_string(), body.to_string()));
        Ok(0)
    }
}

fn labels() -> TriageLabels {
    TriageLabels {
        queue_label: "ready-for-agent".into(),
        needs_info_label: NEEDS_INFO_LABEL.into(),
        human_label: "ready-for-human".into(),
        triage_agent_label: TRIAGE_AGENT_LABEL.into(),
        marker: CONSOLIDATED_SPEC_MARKER.into(),
        promote_marker: PROMOTE_EVIDENCE_MARKER.into(),
    }
}

#[test]
fn promote_upserts_evidence_stamp_then_swaps_labels() {
    // ADR-0027: promote records its evidence stamp via upsert (idempotent),
    // then swaps the labels.
    let body = format!("{PROMOTE_EVIDENCE_MARKER}\n## Evidence (AFK)\n- foo.rs:1");
    let draft = TriageDraft {
        items: vec![TriageItem {
            number: 12,
            verdict: TriageVerdict::Promote,
            comment: Some(body.clone()),
            draft_issue: None,
            drew_on: vec![],
        }],
    };
    let t = RecordingTracker::default();
    apply_triage(&draft, &t, &labels(), |_| true).unwrap();
    let upserts = t.upserts.borrow();
    assert_eq!(upserts.len(), 1);
    assert_eq!(upserts[0].0, 12);
    assert_eq!(upserts[0].1, PROMOTE_EVIDENCE_MARKER);
    assert!(upserts[0].2.contains("Evidence (AFK)"));
    assert_eq!(*t.removed.borrow(), vec![(12, "triage-agent".to_string())]);
    assert_eq!(*t.added.borrow(), vec![(12, "ready-for-agent".to_string())]);
    assert!(t.comments.borrow().is_empty(), "promote uses upsert");
}

#[test]
fn consolidate_upserts_marked_comment_then_swaps_labels() {
    let body = format!("{CONSOLIDATED_SPEC_MARKER}\n## Consolidated spec\n...");
    let draft = TriageDraft {
        items: vec![TriageItem {
            number: 15,
            verdict: TriageVerdict::Consolidate,
            comment: Some(body.clone()),
            draft_issue: None,
            drew_on: vec![],
        }],
    };
    let t = RecordingTracker::default();
    apply_triage(&draft, &t, &labels(), |_| true).unwrap();
    let upserts = t.upserts.borrow();
    assert_eq!(upserts.len(), 1);
    assert_eq!(upserts[0].0, 15);
    assert_eq!(upserts[0].1, CONSOLIDATED_SPEC_MARKER);
    assert!(upserts[0].2.contains("Consolidated spec"));
    // Label swap happened; no plain comment (the upsert is the comment).
    assert_eq!(*t.removed.borrow(), vec![(15, "triage-agent".to_string())]);
    assert_eq!(*t.added.borrow(), vec![(15, "ready-for-agent".to_string())]);
    assert!(t.comments.borrow().is_empty(), "consolidate uses upsert");
}

#[test]
fn bounce_never_asks_and_swaps_to_needs_info() {
    let draft = TriageDraft {
        items: vec![TriageItem {
            number: 18,
            verdict: TriageVerdict::Bounce,
            comment: Some("Missing acceptance criteria.".into()),
            draft_issue: None,
            drew_on: vec![],
        }],
    };
    let t = RecordingTracker::default();
    // `decide` panics if consulted — bounce must apply without it.
    apply_triage(&draft, &t, &labels(), |_| panic!("bounce must not ask")).unwrap();
    assert_eq!(
        *t.comments.borrow(),
        vec![(18, "Missing acceptance criteria.".to_string())]
    );
    assert_eq!(*t.removed.borrow(), vec![(18, "triage-agent".to_string())]);
    assert_eq!(*t.added.borrow(), vec![(18, "needs-info".to_string())]);
}
/// An escalate verdict posts its comment and swaps `triage-agent` for
/// `ready-for-human`, without asking for confirmation, and never creates an
/// issue — not even for a drafted follow-up: the `--yes` invariant; creation
/// lives only in the interactive `run()` path.
#[test]
fn escalate_posts_comment_and_swaps_to_ready_for_human() {
    let body = "A maintainer must decide the pricing rule; see ## Evidence.";
    let follow_up = DraftIssue {
        title: "Restricted follow-up".into(),
        body: "Closes #22".into(),
        labels: vec![],
    };
    // (case, drafted follow-up)
    let rows = [
        ("no follow-up", None),
        ("a drafted follow-up", Some(follow_up)),
    ];
    for (case, draft_issue) in rows {
        let draft = TriageDraft {
            items: vec![TriageItem {
                number: 22,
                verdict: TriageVerdict::Escalate,
                comment: Some(body.to_string()),
                draft_issue,
                drew_on: vec![],
            }],
        };
        let t = RecordingTracker::default();
        // `decide` panics if consulted — escalate must apply without it.
        apply_triage(&draft, &t, &labels(), |_| {
            panic!("{case}: escalate must not ask")
        })
        .unwrap();
        assert_eq!(*t.comments.borrow(), vec![(22, body.to_string())], "{case}");
        assert_eq!(
            *t.removed.borrow(),
            vec![(22, "triage-agent".to_string())],
            "{case}"
        );
        assert_eq!(
            *t.added.borrow(),
            vec![(22, "ready-for-human".to_string())],
            "{case}"
        );
        assert!(t.created.borrow().is_empty(), "{case}: no issue is created");
    }
}
#[test]
fn declined_confirmation_publishes_nothing() {
    let draft = TriageDraft {
        items: vec![
            TriageItem {
                number: 1,
                verdict: TriageVerdict::Promote,
                comment: Some(format!("{PROMOTE_EVIDENCE_MARKER}\nevidence")),
                draft_issue: None,
                drew_on: vec![],
            },
            TriageItem {
                number: 2,
                verdict: TriageVerdict::Consolidate,
                comment: Some(format!("{CONSOLIDATED_SPEC_MARKER}\nspec")),
                draft_issue: None,
                drew_on: vec![],
            },
        ],
    };
    let t = RecordingTracker::default();
    apply_triage(&draft, &t, &labels(), |_| false).unwrap();
    assert!(
        t.removed.borrow().is_empty(),
        "nothing published on decline"
    );
    assert!(t.added.borrow().is_empty());
    assert!(
        t.upserts.borrow().is_empty(),
        "declined promote/consolidate upsert nothing"
    );
}

/// A thread of issue `number` with one comment per `(id, association)`.
fn thread(number: u64, comments: &[(&str, &str)]) -> github::IssueThread {
    let comments: Vec<serde_json::Value> = comments
        .iter()
        .map(|(id, assoc)| {
            serde_json::json!({
                "id": id, "author": {"login": "a"}, "authorAssociation": assoc, "body": "b"
            })
        })
        .collect();
    let json = serde_json::json!({ "body": "spec", "comments": comments });
    github::parse_issue_thread(number, json.to_string().as_bytes()).unwrap()
}

fn consolidation(number: u64, drew_on: &[&str]) -> TriageDraft {
    TriageDraft {
        items: vec![TriageItem {
            number,
            verdict: TriageVerdict::Consolidate,
            comment: Some(format!("{CONSOLIDATED_SPEC_MARKER}\nspec")),
            draft_issue: None,
            drew_on: drew_on.iter().map(|s| s.to_string()).collect(),
        }],
    }
}

/// Under `--yes`, a consolidation that draws on a stranger's comment, on a
/// comment the thread does not have, or on a thread that was never read is
/// not published: it goes to a maintainer, and the comment says which
/// comments held it.
#[test]
fn yes_holds_a_consolidation_that_drew_on_an_outsider() {
    let read = || thread(30, &[("IC_own", "OWNER"), ("IC_out", "NONE")]);
    // (case, drew_on, threads, text the held comment names)
    let rows = [
        (
            "an outsider",
            vec!["IC_own", "IC_out"],
            vec![read()],
            "IC_out",
        ),
        ("an unknown id", vec!["IC_gone"], vec![read()], "IC_gone"),
        (
            "a thread not fetched",
            vec![],
            vec![github::IssueThread::not_fetched(30, "HTTP 502")],
            "could not be read",
        ),
        ("no thread at all", vec![], vec![], "could not be read"),
    ];
    for (case, drew_on, threads, named) in rows {
        let mut draft = consolidation(30, &drew_on);
        assert_eq!(
            hold_untrusted_consolidations(&mut draft, &threads),
            vec![30],
            "{case}"
        );
        let t = RecordingTracker::default();
        apply_triage(&draft, &t, &labels(), |_| true).unwrap();
        assert!(t.upserts.borrow().is_empty(), "{case}: spec published");
        assert_eq!(
            *t.added.borrow(),
            vec![(30, "ready-for-human".to_string())],
            "{case}"
        );
        let comments = t.comments.borrow();
        assert_eq!(comments.len(), 1, "{case}");
        assert!(comments[0].1.contains(named), "{case}: {}", comments[0].1);
        assert!(
            !comments[0].1.contains("IC_own"),
            "{case}: {}",
            comments[0].1
        );
    }
}

/// The control: a consolidation that draws only on trusted comments, or only
/// on the body, is published as it was drafted.
#[test]
fn yes_publishes_a_consolidation_that_drew_on_collaborators() {
    let threads = [thread(31, &[("IC_own", "OWNER"), ("IC_out", "NONE")])];
    for drew_on in [vec!["IC_own"], vec![]] {
        let mut draft = consolidation(31, &drew_on);
        assert!(hold_untrusted_consolidations(&mut draft, &threads).is_empty());
        let t = RecordingTracker::default();
        apply_triage(&draft, &t, &labels(), |_| true).unwrap();
        assert_eq!(t.upserts.borrow().len(), 1, "{drew_on:?}");
        assert_eq!(*t.added.borrow(), vec![(31, "ready-for-agent".to_string())]);
    }
}

/// Interactive triage asks the operator, who reads the preview: the hold runs
/// only in the `--yes` branch of `run`.
#[test]
fn interactive_triage_does_not_hold() {
    let src = include_str!("../triage.rs");
    let calls: Vec<usize> = src
        .match_indices("hold_untrusted_consolidations(&mut draft")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(calls.len(), 1);
    let before = &src[..calls[0]];
    let branch = before.rfind("if args.yes {").expect("no --yes branch");
    assert!(
        !before[branch..].contains('}'),
        "the hold is called outside the `--yes` branch"
    );
}
