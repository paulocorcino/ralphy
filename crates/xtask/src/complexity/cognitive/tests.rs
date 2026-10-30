use super::*;

fn scores(src: &str) -> Vec<(String, u32)> {
    let found = file_scores(src).expect("the test source parses");
    found.into_iter().map(|f| (f.name, f.value)).collect()
}

fn one(src: &str) -> u32 {
    let found = scores(src);
    assert_eq!(found.len(), 1, "one function in {src}");
    found[0].1
}

#[test]
fn a_structure_costs_its_nesting_and_else_costs_one() {
    let src = r#"
        fn f(x: Option<u32>, v: &[u32]) -> u32 {
            let mut n = 0;
            for a in v {                 // +1
                if *a > 1 {              // +2, nested once
                    n += 1;
                } else if *a == 0 {      // +1
                    n += 2;
                } else {                 // +1
                    n += 3;
                }
            }
            match x {                    // +1
                Some(y) => { while n < y { n += 1; } } // +2, nested once
                None => {}
            }
            n
        }
    "#;
    assert_eq!(one(src), 8);
}

#[test]
fn each_run_of_one_logical_operator_costs_one() {
    assert_eq!(
        one("fn g(a: bool, b: bool, c: bool) -> bool { a && b && c }"),
        1
    );
    assert_eq!(
        one("fn h(a: bool, b: bool, c: bool) -> bool { a && b || c }"),
        2
    );
    assert_eq!(
        one("fn i(a: bool, b: bool, c: bool) -> bool { (a || b) && c }"),
        2
    );
}

#[test]
fn a_closure_nests_and_a_label_or_let_else_costs_one() {
    let src = r#"
        fn k(v: Vec<Option<u32>>) -> u32 {
            let Some(first) = v.first().copied().flatten() else { return 0 }; // +1
            let add = |x: u32| if x > 1 { x } else { 0 };                      // +2, +1
            'outer: loop {                                                     // +1
                break 'outer;                                                  // +1
            }
            add(first)
        }
    "#;
    assert_eq!(one(src), 6);
}

#[test]
fn macro_arguments_and_select_arms_are_scored() {
    let fmt = r#"fn m(a: bool) -> String { format!("{}", if a { 1 } else { 2 }) }"#;
    assert_eq!(one(fmt), 2);
    let select = r#"
        async fn s(mut rx: Rx) {
            tokio::select! {                         // +1
                biased;
                m = rx.recv() => { if m.is_some() {} } // +2, nested once
                else => {}
            }
        }
    "#;
    assert_eq!(one(select), 3);
}

#[test]
fn methods_and_inner_functions_are_scored_and_test_code_is_not() {
    let src = r#"
        struct S;
        impl S { fn a(&self) -> u32 { if true { 1 } else { 2 } } }
        trait T { fn b(&self) { if true {} } fn c(&self); }
        fn outer() { fn inner() { if true {} } }
        #[cfg(test)]
        mod tests { fn t() { if true {} } }
        #[test]
        fn u() { if true {} }
    "#;
    let found = scores(src);
    let names: Vec<(&str, u32)> = found.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    assert_eq!(names, [("a", 2), ("b", 1), ("outer", 0), ("inner", 1)]);
}

#[test]
fn production_files_follow_the_declared_modules() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    let files = [
        (
            "src/lib.rs",
            "mod a;\n#[cfg(test)]\nmod tests;\nmod b { mod c; }\n#[path = \"x/y.rs\"]\nmod p;\n",
        ),
        ("src/a.rs", "mod d;\n"),
        ("src/a/d.rs", ""),
        ("src/tests.rs", ""),
        ("src/b/c.rs", ""),
        ("src/x/y.rs", ""),
        ("src/orphan.rs", ""),
        ("src/bin/tool.rs", ""),
        ("src/bin/tool_test_child.rs", ""),
    ];
    for (rel, text) in files {
        let path = root.join(rel);
        let parent = path.parent().expect("a relative path with a parent");
        std::fs::create_dir_all(parent).expect("create the parent");
        std::fs::write(&path, text).expect("write the file");
    }
    let found: Vec<String> = production_files(root)
        .expect("walk")
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        found,
        [
            "src/a/d.rs",
            "src/a.rs",
            "src/b/c.rs",
            "src/bin/tool.rs",
            "src/lib.rs",
            "src/x/y.rs"
        ]
    );
}
