use super::*;

#[test]
fn parse_plan_steps_maps_the_three_markers() {
    let md = "# Plan\n\n## Steps\n- [ ] open one\n- [x] done two\n- [!] noticed three\n\
              - not a step\nprose line\n  - [X] indented checked\n";
    assert_eq!(
        parse_plan_steps(md),
        vec![
            ("open one".to_string(), "open"),
            ("done two".to_string(), "checked"),
            ("noticed three".to_string(), "noticed"),
            ("indented checked".to_string(), "checked"),
        ]
    );
    // The serialized wire form parses back to a JSON array of {text,status}.
    let json = plan_steps_json("- [ ] a\n- [x] b\n");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        parsed,
        serde_json::json!([
            {"text": "a", "status": "open"},
            {"text": "b", "status": "checked"},
        ])
    );
}
