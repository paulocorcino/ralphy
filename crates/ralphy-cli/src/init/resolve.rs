use ralphy_core::{DiagnosisReport, RepoKind};

use super::wizard::{InitConfig, Question};

/// The display form of an optional text field: the value, or the literal `none`
/// when absent — so a seeded default round-trips through [`resolve_text`].
pub(crate) fn display_opt(value: Option<&str>) -> String {
    value.unwrap_or("none").to_string()
}

/// The display form of a [`RepoKind`] — the same token [`resolve_kind`] parses.
pub(crate) fn display_kind(kind: RepoKind) -> String {
    match kind {
        RepoKind::Empty => "empty",
        RepoKind::Existing => "existing",
    }
    .to_string()
}

/// The display form of a bool field — the same token [`resolve_bool`] parses.
pub(crate) fn display_bool(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}

/// The display form of the milestone-docs list: comma-joined, or `none` when
/// empty.
pub(crate) fn display_list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

/// Resolve a free-text answer against a seeded default. Empty input keeps the
/// default; the literal `none` clears it to `None`; anything else is the trimmed
/// override.
pub(crate) fn resolve_text(default: Option<&str>, raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return default.map(str::to_string);
    }

    if trimmed.eq_ignore_ascii_case("none") {
        return None;
    }

    Some(trimmed.to_string())
}

/// Resolve a yes/no answer against a seeded default. Empty input keeps the
/// default; `y`/`yes`/`true` → `true`, `n`/`no`/`false` → `false`; an
/// unrecognized answer keeps the default.
pub(crate) fn resolve_bool(default: bool, raw: &str) -> bool {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "y" | "yes" | "true" => true,
        "n" | "no" | "false" => false,
        _ => default,
    }
}

/// Resolve a repo-kind answer against a seeded default. Empty input keeps the
/// default; `empty`/`existing` set it; an unrecognized answer keeps the default.
pub(crate) fn resolve_kind(default: RepoKind, raw: &str) -> RepoKind {
    match raw.trim().to_ascii_lowercase().as_str() {
        "empty" => RepoKind::Empty,
        "existing" => RepoKind::Existing,
        _ => default,
    }
}

/// Resolve a comma-separated list answer against a seeded default. Empty input
/// keeps the default; the literal `none` clears it to an empty list; otherwise
/// the comma-split, trimmed, non-empty entries replace it.
pub(crate) fn resolve_list(default: &[String], raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return default.to_vec();
    }

    if trimmed.eq_ignore_ascii_case("none") {
        return Vec::new();
    }

    trimmed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Build the seeded console questions from a diagnosis report — each default is
/// the report field's display form, so the dev confirms findings rather than
/// answering blind (ADR-0012 stage 3).
pub(crate) fn seed_questions(report: &DiagnosisReport) -> Vec<Question> {
    vec![
        Question {
            label: "Repository type".into(),
            help: "A new empty repo to set up, or an existing project to work in? \
                   (empty / existing)"
                .into(),
            default: display_kind(report.repo_kind),
            clearable: false,
        },
        Question {
            label: "Language & build".into(),
            help: "Main language and build/test command for this project. (e.g. cargo, npm)".into(),
            default: display_opt(report.language_build.as_deref()),
            clearable: true,
        },
        Question {
            label: "Backlog".into(),
            help: "Where your list of work to do lives, if any. (a link, a label, or a file path)"
                .into(),
            default: display_opt(report.backlog_location.as_deref()),
            clearable: true,
        },
        Question {
            label: "Planning docs".into(),
            help: "Roadmap or spec files to turn into tasks, if any. (file paths, comma-separated)"
                .into(),
            default: display_list(&report.milestone_docs),
            clearable: true,
        },
        Question {
            label: "Skills folder".into(),
            help: "Folder holding the agent's skill files, if any. (e.g. .claude/skills)".into(),
            default: display_opt(report.skills_dir.as_deref()),
            clearable: true,
        },
        Question {
            label: "Architecture docs".into(),
            help: "Do you already have notes about how the project is built? (yes / no)".into(),
            default: display_bool(report.has_context_or_adrs),
            clearable: false,
        },
        Question {
            label: "Code host".into(),
            help: "Where your code is hosted. (e.g. github.com)".into(),
            default: display_opt(report.remote_host.as_deref()),
            clearable: true,
        },
        Question {
            label: "Plan work from the docs above".into(),
            help: "Use the planning docs above to draft your first tasks? (yes / no)".into(),
            default: display_bool(!report.milestone_docs.is_empty()),
            clearable: false,
        },
    ]
}

/// A human-readable echo of the captured config, for the dev to confirm. Every
/// resolved field appears so the confirmation is complete.
pub(crate) fn format_config_echo(cfg: &InitConfig) -> String {
    let mut out = String::new();
    out.push_str(&format!("repo kind:     {}\n", display_kind(cfg.repo_kind)));
    out.push_str(&format!(
        "language/build: {}\n",
        display_opt(cfg.language_build.as_deref())
    ));
    out.push_str(&format!(
        "backlog:        {}\n",
        display_opt(cfg.backlog_location.as_deref())
    ));
    out.push_str(&format!(
        "milestone docs: {}\n",
        display_list(&cfg.milestone_docs)
    ));
    out.push_str(&format!(
        "skills dir:     {}\n",
        display_opt(cfg.skills_dir.as_deref())
    ));
    out.push_str(&format!(
        "context/ADRs:   {}\n",
        display_bool(cfg.has_context_or_adrs)
    ));
    out.push_str(&format!(
        "remote host:    {}\n",
        display_opt(cfg.remote_host.as_deref())
    ));
    out.push_str(&format!(
        "PRD/roadmap:    {}\n",
        display_bool(cfg.adopt_prd_roadmap)
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> DiagnosisReport {
        DiagnosisReport {
            repo_kind: RepoKind::Existing,
            language_build: Some("Rust / cargo".into()),
            backlog_location: Some("docs/backlog.md".into()),
            milestone_docs: vec!["docs/roadmap.md".into(), "docs/prd/0001.md".into()],
            skills_dir: Some(".claude".into()),
            has_context_or_adrs: true,
            remote_host: Some("github.com".into()),
        }
    }

    fn config_of(report: &DiagnosisReport) -> InitConfig {
        InitConfig {
            repo_kind: report.repo_kind,
            language_build: report.language_build.clone(),
            backlog_location: report.backlog_location.clone(),
            milestone_docs: report.milestone_docs.clone(),
            skills_dir: report.skills_dir.clone(),
            has_context_or_adrs: report.has_context_or_adrs,
            remote_host: report.remote_host.clone(),
            adopt_prd_roadmap: !report.milestone_docs.is_empty(),
        }
    }

    #[test]
    fn resolve_text_empty_keeps_default_typed_overrides_none_clears() {
        // Empty input keeps the seeded default.
        assert_eq!(resolve_text(Some("Rust"), "  "), Some("Rust".to_string()));
        // A typed value overrides it.
        assert_eq!(resolve_text(Some("Rust"), "Go"), Some("Go".to_string()));
        // The literal `none` clears it.
        assert_eq!(resolve_text(Some("Rust"), "none"), None);
    }

    #[test]
    fn resolve_bool_empty_keeps_default_typed_overrides() {
        assert!(resolve_bool(true, ""));
        assert!(!resolve_bool(true, "no"));
        assert!(resolve_bool(false, "yes"));
        // Unrecognized → keep the default.
        assert!(!resolve_bool(false, "maybe"));
    }

    #[test]
    fn resolve_kind_empty_keeps_default_typed_overrides() {
        assert_eq!(resolve_kind(RepoKind::Existing, ""), RepoKind::Existing);
        assert_eq!(resolve_kind(RepoKind::Existing, "empty"), RepoKind::Empty);
        assert_eq!(
            resolve_kind(RepoKind::Empty, "existing"),
            RepoKind::Existing
        );
    }

    #[test]
    fn resolve_list_empty_keeps_default_none_clears_csv_splits() {
        let default = vec!["a.md".to_string(), "b.md".to_string()];
        // Empty input keeps the default.
        assert_eq!(resolve_list(&default, "  "), default);
        // The literal `none` clears it.
        assert!(resolve_list(&default, "none").is_empty());
        // A CSV override splits, trims, and drops blanks.
        assert_eq!(
            resolve_list(&default, " x.md , , y.md "),
            vec!["x.md".to_string(), "y.md".to_string()]
        );
    }

    #[test]
    fn seed_questions_defaults_match_report_fields() {
        let report = sample_report();
        let qs = seed_questions(&report);
        assert_eq!(qs[0].default, display_kind(report.repo_kind));
        assert_eq!(qs[1].default, report.language_build.clone().unwrap());
        assert_eq!(qs[2].default, report.backlog_location.clone().unwrap());
        assert_eq!(qs[3].default, report.milestone_docs.join(", "));
        assert_eq!(qs[4].default, report.skills_dir.clone().unwrap());
        assert_eq!(qs[5].default, display_bool(report.has_context_or_adrs));
        assert_eq!(qs[6].default, report.remote_host.clone().unwrap());
        assert_eq!(
            qs[7].default,
            display_bool(!report.milestone_docs.is_empty())
        );
    }

    #[test]
    fn format_config_echo_contains_each_field() {
        let cfg = config_of(&sample_report());
        let echo = format_config_echo(&cfg);
        assert!(echo.contains("existing"), "repo kind missing:\n{echo}");
        assert!(echo.contains("Rust / cargo"), "language missing:\n{echo}");
        assert!(echo.contains("docs/backlog.md"), "backlog missing:\n{echo}");
        assert!(
            echo.contains("docs/roadmap.md"),
            "milestone missing:\n{echo}"
        );
        assert!(echo.contains(".claude"), "skills dir missing:\n{echo}");
        assert!(echo.contains("github.com"), "remote missing:\n{echo}");
        assert!(echo.contains("PRD/roadmap:"), "PRD opt-in missing:\n{echo}");
    }
}
