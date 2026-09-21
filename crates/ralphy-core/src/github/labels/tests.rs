use super::*;

#[test]
fn parse_triage_mapping_finds_mapped_label() {
    // Two-column format: | canonical | mapped |
    let doc = "# Triage Labels\n\
                   | Canonical | Mapped |\n\
                   |-----------|--------|\n\
                   | `ready-for-agent` | `afk-ready` |\n\
                   | `other` | `other-mapped` |\n";
    assert_eq!(
        parse_triage_mapping(doc, "ready-for-agent"),
        Some("afk-ready".into())
    );
}

#[test]
fn parse_triage_mapping_returns_none_when_absent() {
    let doc = "| `other` | `other-mapped` |\n";
    assert_eq!(parse_triage_mapping(doc, "ready-for-agent"), None);
}

#[test]
fn parse_triage_mapping_returns_none_on_empty_doc() {
    assert_eq!(parse_triage_mapping("", "ready-for-agent"), None);
}

// ── label vocabulary (stage 7) ────────────────────────────────────────────

#[test]
fn normalize_color_strips_hash_and_lowercases() {
    assert_eq!(normalize_color("#0E8A16"), "0e8a16");
    assert_eq!(normalize_color("0e8a16"), "0e8a16");
    assert_eq!(normalize_color("  #FFFFFF  "), "ffffff");
}

#[test]
fn ralphy_label_specs_returns_11_names_including_triage_agent() {
    let specs = ralphy_label_specs(None);
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names.len(), 11, "expected 11 specs, got: {names:?}");
    for expected in &[
        "needs-triage",
        "needs-info",
        "ready-for-agent",
        "ready-for-human",
        "wontfix",
        "AFK",
        "HITL",
        "stop-before",
        "needs-split",
        "needs-human-review",
        "triage-agent",
    ] {
        assert!(names.contains(expected), "missing {expected} in {names:?}");
    }
}

/// Every label the runner can apply by literal name must be in the roster
/// `ralphy init` creates, or the first `add_label` fails with `not found`.
/// This is the check that was missing when `needs-split` drifted out.
#[test]
fn runner_applied_labels_are_all_in_the_init_roster() {
    let names: Vec<String> = ralphy_label_specs(None)
        .into_iter()
        .map(|s| s.name)
        .collect();
    for label in [
        crate::runner::NEEDS_SPLIT_LABEL,
        crate::runner::NEEDS_HUMAN_REVIEW_LABEL,
        crate::runner::TRIAGE_AGENT_LABEL,
        crate::runner::STOP_BEFORE_LABEL,
    ] {
        assert!(
            names.iter().any(|n| n == label),
            "runner applies `{label}` but init never creates it: {names:?}"
        );
    }
}

/// Colour is the only signal in a label list once the names wrap, so two
/// specs sharing one is a bug — `needs-split` and `stop-before` were both
/// `d93f0b` and read as the same thing at a glance.
#[test]
fn no_two_specs_share_a_colour() {
    let specs = ralphy_label_specs(None);
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for s in &specs {
        let colour = normalize_color(&s.color);
        if let Some(other) = seen.insert(colour.clone(), s.name.clone()) {
            panic!("`{}` and `{}` share colour #{colour}", other, s.name);
        }
    }
}

/// A spec that reuses a GitHub default label's colour is indistinguishable
/// from it in the picker — `needs-triage` collided with `invalid` and
/// `needs-info` with `documentation`.
#[test]
fn no_spec_collides_with_a_github_default_colour() {
    // The palette GitHub seeds every new repository with.
    let defaults = [
        ("bug", "d73a4a"),
        ("documentation", "0075ca"),
        ("duplicate", "cfd3d7"),
        ("enhancement", "a2eeef"),
        ("good first issue", "7057ff"),
        ("help wanted", "008672"),
        ("invalid", "e4e669"),
        ("question", "d876e3"),
    ];
    for s in ralphy_label_specs(None) {
        let colour = normalize_color(&s.color);
        if let Some((name, _)) = defaults.iter().find(|(_, c)| *c == colour) {
            panic!("`{}` reuses GitHub's `{name}` colour #{colour}", s.name);
        }
    }
}

/// The post-run review-debt label must never be a human gate: it lands on a
/// *closed* issue, and a human-return label would park it back out of the
/// queue as an ADR-0014 blocker.
#[test]
fn needs_human_review_is_not_a_human_return_label() {
    assert!(
        !human_return_labels(None).contains(&NEEDS_HUMAN_REVIEW_LABEL.to_string()),
        "needs-human-review must stay out of the human-gate path"
    );
}

/// Fixed-name labels are applied by literal string at the call site, so a
/// `triage-labels.md` remap must not move them out from under the runner.
#[test]
fn fixed_operational_labels_are_never_remapped() {
    let doc = "| Canonical | Mapped |\n\
                   |-----------|--------|\n\
                   | `needs-split` | `split-me` |\n\
                   | `needs-human-review` | `eyeball-it` |\n";
    let names: Vec<String> = ralphy_label_specs(Some(doc))
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(names.iter().any(|n| n == "needs-split"), "{names:?}");
    assert!(names.iter().any(|n| n == "needs-human-review"), "{names:?}");
    assert!(!names.iter().any(|n| n == "split-me"), "{names:?}");
}

#[test]
fn triage_agent_spec_is_fixed_not_remapped() {
    // Even with a doc that maps every canonical role, triage-agent stays literal.
    let doc = "| Canonical | Mapped |\n\
                   |-----------|--------|\n\
                   | `ready-for-agent` | `afk-ready` |\n\
                   | `needs-info` | `waiting` |\n";
    let specs = ralphy_label_specs(Some(doc));
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"triage-agent"),
        "triage-agent must stay fixed: {names:?}"
    );
}

#[test]
fn human_return_labels_resolves_roles_and_keeps_fixed_names() {
    let doc = "| Canonical | Mapped |\n\
                   |-----------|--------|\n\
                   | `needs-info` | `waiting-reporter` |\n";
    let got = human_return_labels(Some(doc));
    assert_eq!(
        got,
        vec![
            "ready-for-human".to_string(),
            "HITL".to_string(),
            "waiting-reporter".to_string(),
            "needs-triage".to_string(),
            "wontfix".to_string(),
            "triage-agent".to_string(),
        ],
        "role names resolve through the mapping; HITL and triage-agent stay fixed"
    );
}

#[test]
fn human_return_labels_defaults_to_canonical_without_doc() {
    let got = human_return_labels(None);
    assert!(got.contains(&"ready-for-human".to_string()));
    assert!(got.contains(&"needs-info".to_string()));
    assert!(got.contains(&"triage-agent".to_string()));
    assert!(got.contains(&"HITL".to_string()));
}

#[test]
fn ralphy_label_specs_resolves_triage_remap() {
    let doc = "| Canonical | Mapped |\n\
                   |-----------|--------|\n\
                   | `ready-for-agent` | `afk-ready` |\n";
    let specs = ralphy_label_specs(Some(doc));
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"afk-ready"),
        "expected afk-ready in {names:?}"
    );
    assert!(
        !names.contains(&"ready-for-agent"),
        "ready-for-agent should be remapped: {names:?}"
    );
}

#[test]
fn plan_label_actions_empty_existing_yields_all_create() {
    let desired = ralphy_label_specs(None);
    let actions = plan_label_actions(&desired, &[]);
    assert_eq!(actions.len(), desired.len());
    assert!(
        actions.iter().all(|a| matches!(a, LabelAction::Create(_))),
        "expected all Create, got: {actions:?}"
    );
}

#[test]
fn plan_label_actions_full_matching_existing_yields_all_skip() {
    let desired = ralphy_label_specs(None);
    // Use hash-prefixed uppercase colors to exercise normalize_color on the
    // existing side — a raw-comparison bug would produce UpdateColor here.
    let existing: Vec<(String, String)> = desired
        .iter()
        .map(|s| (s.name.clone(), format!("#{}", s.color.to_ascii_uppercase())))
        .collect();
    let actions = plan_label_actions(&desired, &existing);
    let n_create = actions
        .iter()
        .filter(|a| matches!(a, LabelAction::Create(_)))
        .count();
    let n_update = actions
        .iter()
        .filter(|a| matches!(a, LabelAction::UpdateColor { .. }))
        .count();
    let n_skip = actions
        .iter()
        .filter(|a| matches!(a, LabelAction::Skip(_)))
        .count();
    assert_eq!(n_create, 0, "expected 0 Create");
    assert_eq!(n_update, 0, "expected 0 UpdateColor");
    assert_eq!(n_skip, desired.len(), "expected every spec to Skip");
}

#[test]
fn plan_label_actions_differing_color_yields_update_no_create_for_present() {
    let desired = ralphy_label_specs(None);
    // Provide every spec as existing, but one with a wrong color.
    let mut existing: Vec<(String, String)> = desired
        .iter()
        .map(|s| (s.name.clone(), normalize_color(&s.color)))
        .collect();
    // Change AFK's color to something different.
    let afk_idx = existing.iter().position(|(n, _)| n == "AFK").unwrap();
    existing[afk_idx].1 = "aabbcc".into();

    let actions = plan_label_actions(&desired, &existing);
    let n_create = actions
        .iter()
        .filter(|a| matches!(a, LabelAction::Create(_)))
        .count();
    let n_update = actions
        .iter()
        .filter(|a| matches!(a, LabelAction::UpdateColor { .. }))
        .count();
    let n_skip = actions
        .iter()
        .filter(|a| matches!(a, LabelAction::Skip(_)))
        .count();
    assert_eq!(n_create, 0, "no Create expected for any present name");
    assert_eq!(n_update, 1, "expected exactly 1 UpdateColor");
    assert_eq!(
        n_skip,
        desired.len() - 1,
        "expected every spec but the recoloured one to Skip"
    );
    // Verify `to` carries the desired color and `from` the stale one.
    let afk_spec = desired.iter().find(|s| s.name == "AFK").unwrap();
    assert!(
        actions.iter().any(|a| matches!(
            a,
            LabelAction::UpdateColor { name, from, to }
                if name == "AFK"
                && from == "aabbcc"
                && to == &normalize_color(&afk_spec.color)
        )),
        "expected UpdateColor for AFK with correct to/from"
    );
}

#[test]
fn label_create_argv_produces_7_element_vec() {
    let spec = LabelSpec {
        name: "my-label".into(),
        color: "0e8a16".into(),
        description: "A test label".into(),
    };
    let argv = label_create_argv(&spec);
    assert_eq!(
        argv,
        vec![
            "label",
            "create",
            "my-label",
            "--color",
            "0e8a16",
            "--description",
            "A test label"
        ],
        "unexpected argv: {argv:?}"
    );
}

#[test]
fn parse_label_list_reads_name_and_color_pairs() {
    let json = r#"[{"name":"AFK","color":"f9d0c4"},{"name":"stop-before","color":"d93f0b"}]"#;
    let pairs = parse_label_list(json).unwrap();
    assert_eq!(
        pairs,
        vec![
            ("AFK".to_string(), "f9d0c4".to_string()),
            ("stop-before".to_string(), "d93f0b".to_string()),
        ]
    );
}

#[test]
fn format_label_plan_contains_names_and_summary() {
    let actions = vec![
        LabelAction::Create(LabelSpec {
            name: "new-label".into(),
            color: "ff0000".into(),
            description: "new".into(),
        }),
        LabelAction::UpdateColor {
            name: "old-label".into(),
            from: "aabbcc".into(),
            to: "112233".into(),
        },
        LabelAction::Skip("kept-label".into()),
    ];
    let output = format_label_plan(&actions);
    assert!(
        output.contains("new-label"),
        "create name missing:\n{output}"
    );
    assert!(
        output.contains("old-label"),
        "update name missing:\n{output}"
    );
    assert!(
        output.contains("1 to create"),
        "create count missing:\n{output}"
    );
    assert!(
        output.contains("1 to update"),
        "update count missing:\n{output}"
    );
    assert!(
        output.contains("1 unchanged"),
        "skip count missing:\n{output}"
    );
    assert!(
        output.contains("kept-label"),
        "skip name missing:\n{output}"
    );
}

#[test]
fn resolve_queue_labels_explicit_set_returned_verbatim() {
    let explicit = vec!["my-label".to_string(), "other-label".to_string()];
    let result = resolve_queue_labels(&explicit, Path::new("/nonexistent"));
    assert_eq!(result, explicit, "explicit set should be returned verbatim");
}

#[test]
fn resolve_queue_labels_defaults_without_triage_file() {
    let result = resolve_queue_labels(&[], Path::new("/nonexistent/repo"));
    assert_eq!(result, vec!["ready-for-agent", "AFK"]);
}

#[test]
fn resolve_queue_labels_appends_mapped_label_from_triage_file() {
    let dir = std::env::temp_dir().join(format!("ralphy-triage-{}", std::process::id()));
    let docs_dir = dir.join("docs").join("agents");
    std::fs::create_dir_all(&docs_dir).unwrap();
    let triage_content = "| Canonical | Mapped |\n\
                              |-----------|--------|\n\
                              | `ready-for-agent` | `afk-extended` |\n";
    std::fs::write(docs_dir.join("triage-labels.md"), triage_content).unwrap();

    let result = resolve_queue_labels(&[], &dir);
    assert_eq!(result, vec!["ready-for-agent", "AFK", "afk-extended"]);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn resolve_queue_labels_dedupes_mapped_label() {
    let dir = std::env::temp_dir().join(format!("ralphy-triage-dedup-{}", std::process::id()));
    let docs_dir = dir.join("docs").join("agents");
    std::fs::create_dir_all(&docs_dir).unwrap();
    // Mapping resolves to "AFK" which is already in defaults.
    let triage_content = "| `ready-for-agent` | `AFK` |\n";
    std::fs::write(docs_dir.join("triage-labels.md"), triage_content).unwrap();

    let result = resolve_queue_labels(&[], &dir);
    // "AFK" should appear only once.
    assert_eq!(result, vec!["ready-for-agent", "AFK"]);

    std::fs::remove_dir_all(&dir).ok();
}
