//! Repo-label vocabulary: the canonical Ralphy label set, planning/applying
//! label actions against a repository, and the queue/human-return label sets.

use std::path::Path;

use anyhow::{Context, Result};

use crate::github::client::gh_output;
use crate::runner::TRIAGE_AGENT_LABEL;

use super::client::gh;

/// Parse a `docs/agents/triage-labels.md` table row. Scans `doc` for
/// `|`-delimited rows, strips backticks, trims each cell, and returns cell[2]
/// when cell[1] == `canonical`. Ports `Resolve-TriageLabels`'s row parsing.
pub fn parse_triage_mapping(doc: &str, canonical: &str) -> Option<String> {
    for line in doc.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .split('|')
            .map(|c| c.trim().trim_matches('`').trim())
            .collect();
        // After split on '|', a row like `| a | b |` yields
        // ["", "a", "b", ""] — cell[1] and cell[2] are the first and
        // second data columns. Skip separator rows (|---|---|).
        let is_separator = |s: &str| s.trim_matches(['-', ':', ' ']).is_empty() && !s.is_empty();
        if cells.len() >= 4 && cells[1] == canonical && !is_separator(cells[2]) {
            let mapped = cells[2].to_string();
            if !mapped.is_empty() {
                return Some(mapped);
            }
        }
    }
    None
}

/// A label to maintain on the GitHub repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelSpec {
    pub name: String,
    pub color: String,
    pub description: String,
}

/// Strip a leading `#`, trim whitespace, and lowercase — produces the
/// 6-hex lowercase form `gh label list --json color` returns.
fn normalize_color(c: &str) -> String {
    c.trim().trim_start_matches('#').to_ascii_lowercase()
}

/// The 8 canonical Ralphy labels, with triage-role names resolved through
/// `triage_doc` when provided.  Each canonical triage role is looked up via
/// `parse_triage_mapping`; if absent in the doc the canonical name is kept.
/// Fixed-name specs (`AFK`, `HITL`, `stop-before`) are appended after the five
/// triage roles.  The result is deduped by `name` preserving first occurrence.
pub fn ralphy_label_specs(triage_doc: Option<&str>) -> Vec<LabelSpec> {
    let doc = triage_doc.unwrap_or("");
    let resolve = |canonical: &str| -> String {
        parse_triage_mapping(doc, canonical).unwrap_or_else(|| canonical.to_string())
    };

    let mut specs = vec![
        LabelSpec {
            name: resolve("needs-triage"),
            color: "e4e669".into(),
            description: "Needs a human triage pass before it can be worked".into(),
        },
        LabelSpec {
            name: resolve("needs-info"),
            color: "0075ca".into(),
            description: "Blocked — waiting for more information from the author".into(),
        },
        LabelSpec {
            name: resolve("ready-for-agent"),
            color: "0e8a16".into(),
            description: "Ready for an agent to pick up and implement".into(),
        },
        LabelSpec {
            name: resolve("ready-for-human"),
            color: "5319e7".into(),
            description: "Agent finished — waiting for human review and merge".into(),
        },
        LabelSpec {
            name: resolve("wontfix"),
            color: "e6e6e6".into(),
            description: "This issue will not be worked".into(),
        },
        LabelSpec {
            name: "AFK".into(),
            color: "f9d0c4".into(),
            description: "Agent away — run paused, will resume".into(),
        },
        LabelSpec {
            name: "HITL".into(),
            color: "b60205".into(),
            description: "Human-in-the-loop required before the agent can continue".into(),
        },
        LabelSpec {
            name: "stop-before".into(),
            color: "d93f0b".into(),
            description: "Fixed flow-control: agent must stop before acting on this issue".into(),
        },
        LabelSpec {
            name: TRIAGE_AGENT_LABEL.into(),
            color: "fbca04".into(),
            description:
                "Awaiting an agent triage pass (`ralphy triage`) before it enters the queue".into(),
        },
    ];

    // Dedup by name, preserving first occurrence.
    let mut seen = std::collections::HashSet::new();
    specs.retain(|s| seen.insert(s.name.clone()));
    specs
}

/// What to do with one desired label given the current repository state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelAction {
    Create(LabelSpec),
    UpdateColor {
        name: String,
        from: String,
        to: String,
    },
    Skip(String),
}

/// Compare `desired` against `existing` (a `(name, color)` slice from the repo)
/// and return one [`LabelAction`] per desired spec.
pub fn plan_label_actions(
    desired: &[LabelSpec],
    existing: &[(String, String)],
) -> Vec<LabelAction> {
    desired
        .iter()
        .map(|spec| {
            match existing
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(&spec.name))
            {
                None => LabelAction::Create(spec.clone()),
                Some((_, existing_color)) => {
                    let norm_existing = normalize_color(existing_color);
                    let norm_desired = normalize_color(&spec.color);
                    if norm_existing != norm_desired {
                        LabelAction::UpdateColor {
                            name: spec.name.clone(),
                            from: norm_existing,
                            to: norm_desired,
                        }
                    } else {
                        LabelAction::Skip(spec.name.clone())
                    }
                }
            }
        })
        .collect()
}

/// Build the `gh label create` argv for a spec (no `--force`; only absent labels
/// are created).
fn label_create_argv(spec: &LabelSpec) -> Vec<String> {
    vec![
        "label".into(),
        "create".into(),
        spec.name.clone(),
        "--color".into(),
        spec.color.clone(),
        "--description".into(),
        spec.description.clone(),
    ]
}

/// Build the `gh label edit` argv to update a label's color.
fn label_edit_argv(name: &str, color: &str) -> Vec<String> {
    vec![
        "label".into(),
        "edit".into(),
        name.to_string(),
        "--color".into(),
        color.to_string(),
    ]
}

#[derive(serde::Deserialize)]
struct GhLabelColor {
    name: String,
    color: String,
}

/// Parse `[{"name":..,"color":..}]` JSON from `gh label list --json name,color`.
fn parse_label_list(json: &str) -> Result<Vec<(String, String)>> {
    let raw: Vec<GhLabelColor> =
        serde_json::from_str(json).context("parsing `gh label list` JSON array")?;
    Ok(raw.into_iter().map(|l| (l.name, l.color)).collect())
}

/// Fetch the current repository labels via `gh label list --json name,color --limit 200`.
pub fn list_repo_labels(repo_root: &Path) -> Result<Vec<(String, String)>> {
    let out = gh_output("gh label list --json name,color", || {
        let mut cmd = gh(repo_root);
        cmd.args(["label", "list", "--json", "name,color", "--limit", "200"]);
        cmd
    })?;
    parse_label_list(&String::from_utf8_lossy(&out.stdout))
}

/// Render a human-readable plan of label actions: one tagged line per action
/// plus a summary.
pub fn format_label_plan(actions: &[LabelAction]) -> String {
    let mut out = String::new();
    let mut n_create = 0usize;
    let mut n_update = 0usize;
    let mut n_skip = 0usize;
    for action in actions {
        match action {
            LabelAction::Create(spec) => {
                n_create += 1;
                out.push_str(&format!("  create  {}\n", spec.name));
            }
            LabelAction::UpdateColor { name, from, to } => {
                n_update += 1;
                out.push_str(&format!("  update  {} ({} → {})\n", name, from, to));
            }
            LabelAction::Skip(name) => {
                n_skip += 1;
                out.push_str(&format!("  skip    {}\n", name));
            }
        }
    }
    out.push_str(&format!(
        "labels: {} to create, {} to update, {} unchanged\n",
        n_create, n_update, n_skip
    ));
    out
}

/// Execute the label actions against the repository, routing each to the
/// appropriate `gh` call.  `Skip` actions are a no-op.
pub fn apply_label_actions(actions: &[LabelAction], repo_root: &Path) -> Result<()> {
    for action in actions {
        match action {
            LabelAction::Create(spec) => {
                let argv = label_create_argv(spec);
                let args: Vec<&str> = argv.iter().map(String::as_str).collect();
                gh_output(&format!("gh label create {}", spec.name), || {
                    let mut cmd = gh(repo_root);
                    cmd.args(&args);
                    cmd
                })?;
            }
            LabelAction::UpdateColor { name, to, .. } => {
                let argv = label_edit_argv(name, to);
                let args: Vec<&str> = argv.iter().map(String::as_str).collect();
                gh_output(&format!("gh label edit {}", name), || {
                    let mut cmd = gh(repo_root);
                    cmd.args(&args);
                    cmd
                })?;
            }
            LabelAction::Skip(_) => {}
        }
    }
    Ok(())
}

/// Build the effective queue label set. If `explicit` is non-empty, return it
/// verbatim (explicit overrides everything). Otherwise start from the defaults
/// `["ready-for-agent", "AFK"]`, read `docs/agents/triage-labels.md` under
/// `repo_root` (absent is fine), and append the `parse_triage_mapping` result
/// for `"ready-for-agent"`, deduped. Ports `Resolve-TriageLabels`.
pub fn resolve_queue_labels(explicit: &[String], repo_root: &Path) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    let mut labels: Vec<String> = vec!["ready-for-agent".into(), "AFK".into()];
    let triage_path = repo_root
        .join("docs")
        .join("agents")
        .join("triage-labels.md");
    if let Ok(doc) = std::fs::read_to_string(&triage_path) {
        if let Some(mapped) = parse_triage_mapping(&doc, "ready-for-agent") {
            if !labels.contains(&mapped) {
                labels.push(mapped);
            }
        }
    }
    labels
}

/// The human-return label set (ADR-0016): labels that return an issue to a human
/// and therefore outrank any queue label. Triage-role names (`ready-for-human`,
/// `needs-info`, `needs-triage`, `wontfix`) resolve through `triage_doc` like the
/// label specs do; the fixed names (`HITL` alias, `triage-agent`) stay literal.
/// Deduped, first occurrence preserved.
pub fn human_return_labels(triage_doc: Option<&str>) -> Vec<String> {
    let doc = triage_doc.unwrap_or("");
    let resolve = |canonical: &str| -> String {
        parse_triage_mapping(doc, canonical).unwrap_or_else(|| canonical.to_string())
    };
    let mut labels = vec![
        resolve("ready-for-human"),
        "HITL".to_string(),
        resolve("needs-info"),
        resolve("needs-triage"),
        resolve("wontfix"),
        TRIAGE_AGENT_LABEL.to_string(),
    ];
    let mut seen = std::collections::HashSet::new();
    labels.retain(|l| seen.insert(l.clone()));
    labels
}

/// [`human_return_labels`] with the repo's `docs/agents/triage-labels.md` read
/// from disk (absent is fine — canonical names are then kept). Mirrors
/// [`resolve_queue_labels`] so the CLI resolves the set once and hands it to the
/// `gh`-free core through [`crate::runner::QueueConfig`].
pub fn resolve_human_return_labels(repo_root: &Path) -> Vec<String> {
    let triage_path = repo_root
        .join("docs")
        .join("agents")
        .join("triage-labels.md");
    let doc = std::fs::read_to_string(&triage_path).ok();
    human_return_labels(doc.as_deref())
}

#[cfg(test)]
mod tests {
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
    fn ralphy_label_specs_returns_9_names_including_triage_agent() {
        let specs = ralphy_label_specs(None);
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), 9, "expected 9 specs, got: {names:?}");
        for expected in &[
            "needs-triage",
            "needs-info",
            "ready-for-agent",
            "ready-for-human",
            "wontfix",
            "AFK",
            "HITL",
            "stop-before",
            "triage-agent",
        ] {
            assert!(names.contains(expected), "missing {expected} in {names:?}");
        }
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
        assert_eq!(actions.len(), 9);
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
        assert_eq!(n_skip, 9, "expected 9 Skip");
    }

    #[test]
    fn plan_label_actions_differing_color_yields_update_no_create_for_present() {
        let desired = ralphy_label_specs(None);
        // Provide all 9 labels as existing, but one with a wrong color.
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
        assert_eq!(n_skip, 8, "expected 8 Skip");
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
}
