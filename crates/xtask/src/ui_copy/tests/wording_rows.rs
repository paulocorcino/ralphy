use super::*;

fn catalog(texts: &[(&str, &str, &str)]) -> String {
    let values: Vec<String> = texts
        .iter()
        .map(|(key, text, fallback)| {
            format!(
                r#"      "{key}": {{
        "text": {text:?},
        "icon": "x",
        "fallback": {fallback:?},
        "note": "Writer note that cites ADR-0065 and runs on and on and on and on and on."
      }}"#
            )
        })
        .collect();
    format!(
        "{{\n  \"domain\": \"run\",\n  \"values\": {{\n    \"issue-status\": \
         {{\n{}\n    }}\n  }}\n}}\n",
        values.join(",\n")
    )
}

fn wording_found(src: &str) -> Vec<Found> {
    let mut found = Vec::new();
    wording::scan(src, &mut found).expect("the inline catalog parses");
    found
}

/// The rule ids the real rules report for the rows of an inline catalog.
fn broken(src: &str) -> Vec<String> {
    let rules = check::load(&asset_pins::repo_root()).expect("the real rules load");
    let rows: Vec<Row> = wording_found(src)
        .into_iter()
        .map(|f| row_of("wording/run.json", f, &[]))
        .collect();
    check::check(&rows, &rules)
        .violations
        .into_iter()
        .map(|v| v.rule)
        .collect()
}

#[test]
fn every_value_text_is_a_row() {
    let src = catalog(&[
        ("done", "done", "[ok]"),
        ("needs_split", "needs split", "[split]"),
    ]);
    let found = wording_found(&src);
    let got: Vec<(&str, &str, usize)> = found
        .iter()
        .map(|f| (f.kind.label(), f.text.as_str(), f.line))
        .collect();
    assert_eq!(
        got,
        vec![
            ("wording:value", "done", 6),
            ("wording:value", "[ok]", 8),
            ("wording:value", "needs split", 12),
            ("wording:value", "[split]", 14),
        ]
    );
}

#[test]
fn a_catalog_text_citing_an_adr_fails() {
    let rules = broken(&catalog(&[("done", "see ADR-0065", "[ok]")]));
    assert_eq!(rules, vec!["banned:ADR".to_owned()]);
}

#[test]
fn a_long_value_text_fails() {
    let long = "one two three four five six seven eight nine ten eleven twelve thirteen \
                fourteen fifteen sixteen seventeen eighteen nineteen twenty twentyone";
    assert_eq!(
        broken(&catalog(&[("done", long, "[ok]")])),
        vec!["sentence-length".to_owned()]
    );
}
