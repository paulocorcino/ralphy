use std::collections::BTreeSet;

use ralphy_wording::check_values;

use super::*;

fn set(name: &str) -> &'static std::collections::BTreeMap<String, ValueWording> {
    run_file().values.get(name).expect("run.json holds the set")
}

#[test]
fn issue_status_matches_the_catalog() {
    assert_eq!(check_values::<IssueStatus>(run_file()), Ok(()));
}

#[test]
fn run_outcome_matches_the_catalog() {
    assert_eq!(check_values::<FinishOutcome>(run_file()), Ok(()));
}

#[test]
fn issue_status_keys_are_the_wire_names() {
    let statuses = [
        IssueStatus::Planning,
        IssueStatus::Executing,
        IssueStatus::Planned,
        IssueStatus::Done,
        IssueStatus::Skipped,
        IssueStatus::Blocked,
        IssueStatus::Infeasible,
        IssueStatus::NeedsSplit,
        IssueStatus::NonGreen,
        IssueStatus::Hitl,
    ];
    let wire: BTreeSet<&str> = statuses.into_iter().map(snapshot::status_wire).collect();
    assert_eq!(wire.len(), 10, "the ten statuses have ten wire names");
    let keys: BTreeSet<&str> = set("issue-status").keys().map(String::as_str).collect();
    assert_eq!(keys, wire);
}

#[test]
fn every_value_has_an_icon_and_a_distinct_fallback() {
    for name in ["issue-status", "run-outcome"] {
        let mut fallbacks = BTreeSet::new();
        for (key, wording) in set(name) {
            let icon = wording.icon.as_deref().unwrap_or_default();
            assert!(!icon.is_empty(), "{name}.{key} has no icon");
            let fallback = wording.fallback.as_deref().unwrap_or_default();
            assert!(!fallback.is_empty(), "{name}.{key} has no fallback");
            assert!(
                fallbacks.insert(fallback),
                "{name}.{key} repeats the fallback {fallback}"
            );
        }
    }
}

#[test]
fn wording_reads_the_entry_of_the_value() {
    assert_eq!(IssueStatus::NeedsSplit.wording().text, "needs split");
    assert_eq!(
        FinishOutcome::Timeout.wording().fallback.as_deref(),
        Some("[timeout]")
    );
}
