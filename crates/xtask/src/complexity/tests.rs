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
fn a_listed_function_over_the_ceiling_fails() {
    let current = table([score("a.rs", "run", 51), score("a.rs", "load", 50)].iter());
    assert_eq!(
        ceiling_errors(50, &current),
        ["a.rs: run: 51 is over the ceiling 50"]
    );
    assert!(ceiling_errors(51, &current).is_empty());
}

#[test]
fn the_baseline_file_keeps_its_ceiling_through_a_round_trip() {
    let baseline = Baseline {
        ceiling: 30,
        functions: table([score("a.rs", "run", 12)].iter()),
    };
    let text = to_json(&baseline).expect("serialize");
    assert_eq!(parse_baseline(&text).expect("parse"), baseline);
}

#[test]
fn a_blessed_baseline_keeps_the_ceiling_it_read_and_never_raises_it() {
    assert_eq!(ceiling_of(None).expect("no file"), CEILING);
    let old_format = r#"{ "a.rs": { "run": [12] } }"#;
    assert_eq!(ceiling_of(Some(old_format)).expect("old"), CEILING);
    let lowered = r#"{ "ceiling": 30, "functions": {} }"#;
    assert_eq!(ceiling_of(Some(lowered)).expect("lowered"), 30);
    assert!(ceiling_of(Some("not json")).is_err());
}

#[test]
fn a_ceiling_error_is_never_told_to_bless() {
    let bless =
        "run `cargo run -p xtask -- complexity --bless` to record a function that went down";
    let split = "split a function that grew along a responsibility it already has";
    let ceiling =
        "a function over the ceiling can only be split: --bless does not raise the ceiling";
    assert_eq!(fix_advice(true, false), format!("{split}; {bless}"));
    assert_eq!(fix_advice(false, true), format!("{split}; {ceiling}"));
    assert_eq!(
        fix_advice(true, true),
        format!("{split}; {bless}; {ceiling}")
    );
}

#[test]
fn a_baseline_in_the_old_format_says_to_bless_it() {
    let old_format = r#"{ "a.rs": { "run": [12] } }"#;
    let error = parse_baseline(old_format).expect_err("no ceiling key");
    assert!(
        format!("{error:#}").contains("run `cargo run -p xtask -- complexity --bless` to migrate"),
        "{error:#}"
    );
}
