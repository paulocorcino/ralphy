use std::collections::{HashMap, HashSet};

use super::*;

#[derive(Default)]
struct Tracker {
    closed: HashSet<u64>,
    children: HashMap<u64, Vec<u64>>,
    labels: HashMap<u64, Vec<String>>,
}

impl IssueTracker for Tracker {
    fn close(&self, _number: u64, _comment: &str) -> Result<()> {
        panic!("the blocked-by gate never closes an issue")
    }
    fn is_closed(&self, number: u64) -> Result<bool> {
        Ok(self.closed.contains(&number))
    }
    fn create_issue(&self, _title: &str, _body: &str, _labels: &[String]) -> Result<u64> {
        panic!("the blocked-by gate never creates an issue")
    }
    fn open_children(&self, number: u64) -> Result<Vec<u64>> {
        Ok(self.children.get(&number).cloned().unwrap_or_default())
    }
    fn issue_labels(&self, number: u64) -> Result<Vec<String>> {
        Ok(self.labels.get(&number).cloned().unwrap_or_default())
    }
}

fn blocked_issue(number: u64, blocker: u64) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        body: format!("## Blocked by\n- #{blocker}\n"),
        labels: Vec::new(),
        comments: Vec::new(),
    }
}

type SplitCase = (&'static str, Vec<u64>, Vec<u64>, Vec<u64>);

#[test]
fn a_closed_blocker_blocks_on_its_open_children_except_the_issue_itself() {
    // (case, open children of the closed blocker #3, expected open, expected closed)
    let rows: [SplitCase; 4] = [
        ("open children", vec![16, 17], vec![16, 17], vec![]),
        ("no open children", vec![], vec![], vec![3]),
        (
            "the issue among the children",
            vec![4, 16],
            vec![16],
            vec![],
        ),
        ("the issue the only child", vec![4], vec![], vec![3]),
    ];
    for (case, children, open, closed) in rows {
        let tracker = Tracker {
            closed: HashSet::from([3]),
            children: HashMap::from([(3, children)]),
            ..Default::default()
        };
        let got = open_blockers(&blocked_issue(4, 3), &tracker).unwrap();
        assert_eq!(got.open, open, "{case}: open blockers");
        assert_eq!(got.closed, closed, "{case}: closed blockers");
    }
}

#[test]
fn an_issue_never_blocks_itself() {
    let got = open_blockers(&blocked_issue(5, 5), &Tracker::default()).unwrap();
    assert!(got.open.is_empty(), "a self-ref is dropped");
    assert!(got.closed.is_empty(), "a self-ref is not a handoff source");
}

#[test]
fn only_a_human_gate_label_makes_an_open_blocker_a_human_blocker() {
    // (blocker labels, expected human blockers)
    let rows: [(&[&str], Vec<u64>); 4] = [
        (&["ready-for-human"], vec![2]),
        (&["HITL"], vec![2]),
        (&["ready-for-agent"], vec![]),
        (&[], vec![]),
    ];
    for (labels, human) in rows {
        let tracker = Tracker {
            labels: HashMap::from([(2, labels.iter().map(ToString::to_string).collect())]),
            ..Default::default()
        };
        let got = open_blockers(&blocked_issue(5, 2), &tracker).unwrap();
        assert_eq!(
            got.open,
            vec![2],
            "{labels:?}: the open blocker still blocks"
        );
        assert_eq!(got.human, human, "{labels:?}: human blockers");
    }
}
