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
        PresenceGate::Proceed { warn: Some(_) } => {}
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
        PresenceGate::Proceed { warn: Some(_) } => {}
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

#[test]
fn escalate_posts_comment_and_swaps_to_ready_for_human() {
    let body = "A maintainer must decide the pricing rule; see ## Evidence.";
    let draft = TriageDraft {
        items: vec![TriageItem {
            number: 22,
            verdict: TriageVerdict::Escalate,
            comment: Some(body.to_string()),
            draft_issue: None,
        }],
    };
    let t = RecordingTracker::default();
    apply_triage(&draft, &t, &labels(), |_| true).unwrap();
    assert_eq!(*t.comments.borrow(), vec![(22, body.to_string())]);
    assert_eq!(*t.removed.borrow(), vec![(22, "triage-agent".to_string())]);
    assert_eq!(*t.added.borrow(), vec![(22, "ready-for-human".to_string())]);
    assert!(
        t.created.borrow().is_empty(),
        "apply_triage never creates an issue"
    );
}

#[test]
fn escalate_never_asks_confirmation() {
    let draft = TriageDraft {
        items: vec![TriageItem {
            number: 23,
            verdict: TriageVerdict::Escalate,
            comment: Some("A maintainer owes a decision.".into()),
            draft_issue: None,
        }],
    };
    let t = RecordingTracker::default();
    // `decide` panics if consulted — escalate must apply without it.
    apply_triage(&draft, &t, &labels(), |_| panic!("escalate must not ask")).unwrap();
    assert_eq!(*t.added.borrow(), vec![(23, "ready-for-human".to_string())]);
}

#[test]
fn yes_mode_escalate_creates_no_issues() {
    // The `--yes` invariant: `apply_triage` over an escalate item that
    // carries a drafted follow-up never creates an issue — creation lives
    // only in the interactive `run()` path.
    let draft = TriageDraft {
        items: vec![TriageItem {
            number: 24,
            verdict: TriageVerdict::Escalate,
            comment: Some("A maintainer owes a decision.".into()),
            draft_issue: Some(DraftIssue {
                title: "Restricted follow-up".into(),
                body: "Closes #24".into(),
                labels: vec![],
            }),
        }],
    };
    let t = RecordingTracker::default();
    apply_triage(&draft, &t, &labels(), |_| true).unwrap();
    assert!(
        t.created.borrow().is_empty(),
        "--yes escalate must never create an issue"
    );
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
            },
            TriageItem {
                number: 2,
                verdict: TriageVerdict::Consolidate,
                comment: Some(format!("{CONSOLIDATED_SPEC_MARKER}\nspec")),
                draft_issue: None,
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

#[test]
fn retriage_edits_existing_marked_comment() {
    // Idempotence lives behind `upsert_marked_comment`; this asserts the CLI
    // routes a consolidation through the upsert (never a plain `comment`), so a
    // re-triage edits the marked comment rather than stacking a second one.
    let draft = TriageDraft {
        items: vec![TriageItem {
            number: 7,
            verdict: TriageVerdict::Consolidate,
            comment: Some(format!("{CONSOLIDATED_SPEC_MARKER}\nv2 spec")),
            draft_issue: None,
        }],
    };
    let t = RecordingTracker::default();
    apply_triage(&draft, &t, &labels(), |_| true).unwrap();
    assert_eq!(t.upserts.borrow().len(), 1, "exactly one upsert");
    assert!(
        t.comments.borrow().is_empty(),
        "consolidation never posts a plain comment"
    );
}
