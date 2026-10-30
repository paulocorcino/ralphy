use super::*;

fn score(path: &str, name: &str, value: u32) -> Score {
    Score {
        path: path.to_string(),
        name: name.to_string(),
        value,
        marked: false,
    }
}

/// A function named `name` that scores `ifs`: one `if` after another, each 1.
fn function_of(name: &str, ifs: u32) -> String {
    let body: String = (0..ifs).map(|i| format!("if x == {i} {{}} ")).collect();
    format!("fn {name}(x: u32) {{ {body}}}\n")
}

fn errors_of(src: &str) -> Vec<String> {
    over_limit(&file_check("a.rs", src).expect("the test source parses"))
}

#[test]
fn a_rust_function_over_ten_fails_the_check() {
    let src = function_of("ten", 10) + &function_of("eleven", 11);
    assert_eq!(errors_of(&src), ["a.rs: eleven: 11, over the limit of 10"]);
}

#[test]
fn a_marked_function_over_ten_passes_the_check() {
    // The marker is above the doc comment and the attributes of `kept`.
    let kept = "// complexity-exception: one table of cases\n/// The cases.\n#[inline]\n";
    let src = format!(
        "{kept}{}{}",
        function_of("kept", 11),
        function_of("other", 11)
    );
    assert_eq!(errors_of(&src), ["a.rs: other: 11, over the limit of 10"]);
    let method = format!(
        "struct S;\nimpl S {{\n    {kept}{}}}\n",
        function_of("m", 11)
    );
    assert!(errors_of(&method).is_empty());
}

#[test]
fn a_marker_with_no_reason_does_not_count() {
    for marker in ["// complexity-exception:", "// complexity-exception:   "] {
        let src = format!("{marker}\n{}", function_of("bare", 11));
        assert_eq!(
            errors_of(&src),
            ["a.rs: bare: 11, over the limit of 10"],
            "{marker:?}"
        );
    }
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
