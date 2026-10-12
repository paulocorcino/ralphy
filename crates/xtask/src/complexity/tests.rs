use super::*;

fn score(path: &str, name: &str, value: u32) -> Score {
    Score {
        path: path.to_string(),
        name: name.to_string(),
        value,
    }
}

#[test]
fn oxlint_scores_name_an_anonymous_function_and_refuse_an_unknown_message() {
    let report = serde_json::json!({ "diagnostics": [
        { "code": "eslint(complexity)", "filename": "ui/a.ts",
          "message": "function `paint` has a complexity of 14. Maximum allowed is 6." },
        { "code": "eslint(complexity)", "filename": "ui/a.ts",
          "message": "async function `load` has a complexity of 8. Maximum allowed is 6." },
        { "code": "eslint(complexity)", "filename": "ui/b.ts",
          "message": "function has a complexity of 11. Maximum allowed is 6." }
    ]});
    let scores = parse_oxlint(&report.to_string()).expect("parse");
    assert_eq!(
        scores,
        [
            score("ui/a.ts", "paint", 14),
            score("ui/a.ts", "load", 8),
            score("ui/b.ts", ANONYMOUS, 11)
        ]
    );
    let changed = serde_json::json!({ "diagnostics": [
        { "code": "eslint(complexity)", "filename": "ui/a.ts",
          "message": "method `paint` is too complex (14)" }
    ]});
    assert!(parse_oxlint(&changed.to_string()).is_err());
}

#[test]
fn a_new_function_over_the_limit_fails() {
    let baseline = table([score("a.rs", "run", 12)].iter());
    let current = table([score("a.rs", "run", 12), score("a.rs", "load", 11)].iter());
    assert_eq!(
        compare(&baseline, &current),
        ["a.rs: load: 11, over the limit of 10"]
    );
}

#[test]
fn a_listed_function_may_not_grow_and_a_lower_value_updates_the_baseline() {
    let baseline = table([score("a.rs", "run", 12)].iter());
    let same = table([score("a.rs", "run", 12)].iter());
    assert!(compare(&baseline, &same).is_empty());
    let grown = table([score("a.rs", "run", 13)].iter());
    assert_eq!(compare(&baseline, &grown), ["a.rs: run: 12 -> 13"]);
    let lower = table([score("a.rs", "run", 11)].iter());
    assert_eq!(
        compare(&baseline, &lower),
        ["a.rs: run: 12 -> 11, update the baseline"]
    );
    let gone = Table::new();
    assert_eq!(
        compare(&baseline, &gone),
        ["a.rs: run: 12 -> 10 or less, update the baseline"]
    );
}

#[test]
fn functions_of_one_name_are_compared_highest_first() {
    let baseline = table([score("a.ts", ANONYMOUS, 20), score("a.ts", ANONYMOUS, 12)].iter());
    // The same two values, found in the other order.
    let moved = table([score("a.ts", ANONYMOUS, 12), score("a.ts", ANONYMOUS, 20)].iter());
    assert!(compare(&baseline, &moved).is_empty());
    let third = table(
        [
            score("a.ts", ANONYMOUS, 12),
            score("a.ts", ANONYMOUS, 20),
            score("a.ts", ANONYMOUS, 11),
        ]
        .iter(),
    );
    assert_eq!(
        compare(&baseline, &third),
        ["a.ts: (anonymous): 11, over the limit of 10"]
    );
}

#[test]
fn the_near_list_holds_seven_to_ten_in_the_changed_files() {
    let scores = [
        score("a.rs", "six", 6),
        score("a.rs", "seven", 7),
        score("a.rs", "ten", 10),
        score("a.rs", "eleven", 11),
        score("b.rs", "eight", 8),
    ];
    let files = BTreeSet::from(["a.rs".to_string()]);
    let names: Vec<&str> = near_limit(&scores, &files)
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["seven", "ten"]);
}

#[test]
fn ci_installs_the_pinned_oxlint() {
    let ci =
        std::fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).expect("read ci.yml");
    assert!(
        ci.contains(&format!("OXLINT_VERSION: \"{OXLINT_VERSION}\"")),
        "ci.yml must pin oxlint {OXLINT_VERSION}"
    );
}
