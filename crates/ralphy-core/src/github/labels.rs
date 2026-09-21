//! Repo-label vocabulary: the canonical Ralphy label set, planning/applying
//! label actions against a repository, and the queue/human-return label sets.

use std::path::Path;

use anyhow::{Context, Result};

use crate::github::client::gh_output;
use crate::runner::{NEEDS_HUMAN_REVIEW_LABEL, NEEDS_SPLIT_LABEL, TRIAGE_AGENT_LABEL};

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

/// The canonical Ralphy labels, with triage-role names resolved through
/// `triage_doc` when provided.  Each canonical triage role is looked up via
/// `parse_triage_mapping`; if absent in the doc the canonical name is kept.
/// Fixed-name specs (`AFK`, `HITL`, `stop-before`, `needs-split`,
/// `needs-human-review`) are appended after the five triage roles — the runner
/// applies those by literal name, so they are never remapped.  The result is
/// deduped by `name` preserving first occurrence.
///
/// Every label the runner can apply must be listed here, or `ralphy init` will
/// not create it and the first `gh issue edit --add-label` fails with
/// `'<name>' not found` — a non-transient failure the runner only warns about.
pub fn ralphy_label_specs(triage_doc: Option<&str>) -> Vec<LabelSpec> {
    let doc = triage_doc.unwrap_or("");
    let resolve = |canonical: &str| -> String {
        parse_triage_mapping(doc, canonical).unwrap_or_else(|| canonical.to_string())
    };

    // Colour is a signal, not decoration: labels in the same family share a hue,
    // and no two specs — nor a spec and a GitHub default — share an exact colour.
    //   green  = go, the queue set (`resolve_queue_labels`)
    //   purple = blocked on a person (`HUMAN_GATE_LABELS` and human-return)
    //   amber  = the triage pipeline
    //   red    = the run stopped here
    let mut specs = vec![
        LabelSpec {
            name: resolve("needs-triage"),
            color: "fbca04".into(),
            description: "Needs a human triage pass before it can be worked".into(),
        },
        LabelSpec {
            name: resolve("needs-info"),
            color: "d4c5f9".into(),
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
            description: "Needs human implementation or decision before an agent can proceed"
                .into(),
        },
        LabelSpec {
            name: resolve("wontfix"),
            color: "e6e6e6".into(),
            description: "This issue will not be worked".into(),
        },
        LabelSpec {
            name: "AFK".into(),
            color: "9be9a8".into(),
            description: "Agent away — run paused, will resume".into(),
        },
        LabelSpec {
            name: "HITL".into(),
            color: "8957e5".into(),
            description: "Human-in-the-loop required before the agent can continue".into(),
        },
        LabelSpec {
            name: "stop-before".into(),
            color: "d93f0b".into(),
            description: "Fixed flow-control: agent must stop before acting on this issue".into(),
        },
        LabelSpec {
            name: NEEDS_SPLIT_LABEL.into(),
            color: "e99695".into(),
            description: "Ralphy: bundle issue awaiting split into child issues".into(),
        },
        LabelSpec {
            name: NEEDS_HUMAN_REVIEW_LABEL.into(),
            color: "bfd4f2".into(),
            description: "Ralphy closed it green, but its acceptance ledger left review-only \
                          criteria for a human to confirm"
                .into(),
        },
        LabelSpec {
            name: TRIAGE_AGENT_LABEL.into(),
            color: "ffe0a6".into(),
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
mod tests;
