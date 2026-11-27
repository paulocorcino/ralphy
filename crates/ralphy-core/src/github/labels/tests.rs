use super::*;

#[test]
fn parse_triage_mapping_reads_the_mapped_column() {
    // Two-column format: | canonical | mapped |
    let doc = "# Triage Labels\n\
                   | Canonical | Mapped |\n\
                   |-----------|--------|\n\
                   | `ready-for-agent` | `afk-ready` |\n\
                   | `other` | `other-mapped` |\n";
    // (case, document, expected mapping of `ready-for-agent`)
    let rows = [
        ("mapped label", doc, Some("afk-ready")),
        ("role absent", "| `other` | `other-mapped` |\n", None),
        ("empty document", "", None),
    ];
    for (case, doc, want) in rows {
        assert_eq!(
            parse_triage_mapping(doc, "ready-for-agent").as_deref(),
            want,
            "{case}"
        );
    }
}

// ── label vocabulary (stage 7) ────────────────────────────────────────────

#[test]
fn normalize_color_strips_hash_and_lowercases() {
    assert_eq!(normalize_color("#0E8A16"), "0e8a16");
    assert_eq!(normalize_color("0e8a16"), "0e8a16");
    assert_eq!(normalize_color("  #FFFFFF  "), "ffffff");
}

#[test]
fn ralphy_label_specs_include_every_role_and_triage_agent() {
    let specs = ralphy_label_specs(None);
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
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
        NEEDS_SPLIT_LABEL,
        NEEDS_HUMAN_REVIEW_LABEL,
        TRIAGE_AGENT_LABEL,
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

#[test]
fn human_return_labels_resolves_roles_and_keeps_fixed_names() {
    let doc = "| Canonical | Mapped |\n\
                   |-----------|--------|\n\
                   | `needs-info` | `waiting-reporter` |\n";
    let got = human_return_labels(Some(doc));
    assert_eq!(
        got,
        vec![
            "ready-for-human".to_owned(),
            "HITL".to_owned(),
            "waiting-reporter".to_owned(),
            "needs-triage".to_owned(),
            "wontfix".to_owned(),
            "triage-agent".to_owned(),
        ],
        "role names resolve through the mapping; HITL and triage-agent stay fixed"
    );
}

/// A `triage-labels.md` remap renames a role label; the fixed-name labels are
/// applied by literal string at the call site, so a remap must not move them.
#[test]
fn ralphy_label_specs_remap_roles_and_keep_fixed_names() {
    let head = "| Canonical | Mapped |\n|-----------|--------|\n";
    // (case, mapping rows, names present, names absent)
    let rows: [(&str, &str, &[&str], &[&str]); 3] = [
        (
            "role remapped",
            "| `ready-for-agent` | `afk-ready` |\n",
            &["afk-ready"],
            &["ready-for-agent"],
        ),
        (
            "operational labels fixed",
            "| `needs-split` | `split-me` |\n| `needs-human-review` | `eyeball-it` |\n",
            &["needs-split", "needs-human-review"],
            &["split-me", "eyeball-it"],
        ),
        (
            "triage-agent fixed",
            "| `ready-for-agent` | `afk-ready` |\n| `needs-info` | `waiting` |\n",
            &["triage-agent"],
            &[],
        ),
    ];
    for (case, mapping, present, absent) in rows {
        let doc = format!("{head}{mapping}");
        let names: Vec<String> = ralphy_label_specs(Some(&doc))
            .into_iter()
            .map(|s| s.name)
            .collect();
        for name in present {
            assert!(
                names.iter().any(|n| n == name),
                "{case}: {name} in {names:?}"
            );
        }
        for name in absent {
            assert!(
                !names.iter().any(|n| n == name),
                "{case}: no {name} in {names:?}"
            );
        }
    }
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
            ("AFK".to_owned(), "f9d0c4".to_owned()),
            ("stop-before".to_owned(), "d93f0b".to_owned()),
        ]
    );
}

/// Each desired spec becomes one action: absent is created, a matching colour
/// (compared after `#` and case are normalized) is skipped, another is updated.
#[test]
fn plan_label_actions_create_skip_or_recolour_each_spec() {
    type Existing = fn(&LabelSpec) -> Option<String>;
    let desired = ralphy_label_specs(None);
    let rows: [(&str, Existing); 3] = [
        ("empty repo", |_| None),
        ("every colour matches", |s| {
            Some(format!("#{}", s.color.to_ascii_uppercase()))
        }),
        ("AFK recoloured", |s| {
            Some(if s.name == "AFK" {
                "aabbcc".to_owned()
            } else {
                s.color.clone()
            })
        }),
    ];
    for (case, existing_of) in rows {
        let existing: Vec<(String, String)> = desired
            .iter()
            .filter_map(|s| existing_of(s).map(|c| (s.name.clone(), c)))
            .collect();
        let want: Vec<LabelAction> = desired
            .iter()
            .map(|s| match existing_of(s) {
                None => LabelAction::Create(s.clone()),
                Some(c) if c == "aabbcc" => LabelAction::UpdateColor {
                    name: s.name.clone(),
                    from: "aabbcc".into(),
                    to: s.color.clone(),
                },
                Some(_) => LabelAction::Skip(s.name.clone()),
            })
            .collect();
        assert_eq!(plan_label_actions(&desired, &existing), want, "{case}");
    }
}

#[test]
fn format_label_plan_lists_each_action_and_the_counts() {
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
    assert_eq!(
        format_label_plan(&actions),
        "  create  new-label\n  update  old-label (aabbcc → 112233)\n  skip    kept-label\n\
         labels: 1 to create, 1 to update, 1 unchanged\n"
    );
}

/// An explicit set wins verbatim; otherwise the defaults, plus the mapped
/// `ready-for-agent` label from `docs/agents/triage-labels.md` when it is new.
#[test]
fn resolve_queue_labels_explicit_or_defaults_plus_mapping() {
    // (case, explicit labels, triage file text, expected labels)
    type Row<'a> = (&'a str, &'a [&'a str], Option<&'a str>, &'a [&'a str]);
    let rows: [Row<'_>; 4] = [
        (
            "explicit set",
            &["my-label", "other-label"],
            None,
            &["my-label", "other-label"],
        ),
        ("no triage file", &[], None, &["ready-for-agent", "AFK"]),
        (
            "mapped label appended",
            &[],
            Some(concat!(
                "| Canonical | Mapped |\n|-----------|--------|\n",
                "| `ready-for-agent` | `afk-extended` |\n",
            )),
            &["ready-for-agent", "AFK", "afk-extended"],
        ),
        (
            "mapped label deduped",
            &[],
            Some("| `ready-for-agent` | `AFK` |\n"),
            &["ready-for-agent", "AFK"],
        ),
    ];
    for (case, explicit, triage, want) in rows {
        let dir = tempfile::tempdir().unwrap();
        if let Some(text) = triage {
            let docs = dir.path().join("docs").join("agents");
            std::fs::create_dir_all(&docs).unwrap();
            std::fs::write(docs.join("triage-labels.md"), text).unwrap();
        }
        let explicit: Vec<String> = explicit.iter().map(|l| (*l).to_owned()).collect();
        assert_eq!(resolve_queue_labels(&explicit, dir.path()), *want, "{case}");
    }
}
