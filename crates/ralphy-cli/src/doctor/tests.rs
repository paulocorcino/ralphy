use super::*;

fn all_found() -> MachineFacts {
    MachineFacts {
        git: Some("git version 2.47.1".to_string()),
        gh: Some("gh version 2.89.0 (2026-03-01)".to_string()),
        gh_signed_in: true,
        python: Some("Python 3.12.4".to_string()),
        agents: vec![(Agent::Claude, "2.1.0 (Claude Code)".to_string())],
        linger: None,
    }
}

fn line(report: &DoctorReport, name: &str) -> ToolLine {
    report
        .tools
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no {name} line in {report:?}"))
        .clone()
}

#[test]
fn each_tool_found_shows_its_version_and_nothing_is_missing() {
    let report = evaluate(&all_found(), false);
    assert_eq!(report.missing_required(), 0);
    let names: Vec<&str> = report.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["git", "gh", "python", "claude"]);
    assert_eq!(
        line(&report, "git").version.as_deref(),
        Some("git version 2.47.1")
    );
    assert_eq!(
        line(&report, "claude").version.as_deref(),
        Some("2.1.0 (Claude Code)")
    );
}

#[test]
fn each_missing_required_tool_counts() {
    let rows: [(&str, MachineFacts, &str); 5] = [
        (
            "no git",
            MachineFacts {
                git: None,
                ..all_found()
            },
            "git",
        ),
        (
            "no gh",
            MachineFacts {
                gh: None,
                gh_signed_in: false,
                ..all_found()
            },
            "gh",
        ),
        (
            "gh signed out",
            MachineFacts {
                gh_signed_in: false,
                ..all_found()
            },
            "gh",
        ),
        (
            "no python",
            MachineFacts {
                python: None,
                ..all_found()
            },
            "python",
        ),
        (
            "no agent",
            MachineFacts {
                agents: vec![],
                ..all_found()
            },
            "agent CLI",
        ),
    ];
    for (case, facts, name) in rows {
        let report = evaluate(&facts, false);
        assert_eq!(report.missing_required(), 1, "{case}");
        assert!(!line(&report, name).ok, "{case}");
    }
}

#[test]
fn a_signed_out_gh_says_how_to_sign_in() {
    let report = evaluate(
        &MachineFacts {
            gh_signed_in: false,
            ..all_found()
        },
        false,
    );
    assert_eq!(line(&report, "gh").fix.as_deref(), Some("gh auth login"));
}

#[test]
fn lingering_warns_and_never_fails() {
    let off = evaluate(
        &MachineFacts {
            linger: Some(Some(false)),
            ..all_found()
        },
        false,
    );
    let linger = line(&off, "lingering");
    assert!(!linger.ok && !linger.required);
    assert_eq!(off.missing_required(), 0);
    assert_eq!(linger.text, linger_text(Some(false)));
    assert!(
        !evaluate(&all_found(), false)
            .tools
            .iter()
            .any(|t| t.tool == Tool::Linger),
        "no lingering line off Linux"
    );
    assert_eq!(parse_linger("Linger=yes\n"), Some(true));
    assert_eq!(parse_linger("Linger=no\n"), Some(false));
    assert_eq!(parse_linger("Failed to get user"), None);
}

/// `host add` reads this JSON on the host; its shape is the contract between
/// two versions of Ralphy.
#[test]
fn the_json_shape_is_pinned() {
    let report = evaluate(
        &MachineFacts {
            python: None,
            ..all_found()
        },
        false,
    );
    let json = serde_json::to_value(&report).expect("serializing");
    assert_eq!(
        json["tools"][2],
        serde_json::json!({
            "tool": "python",
            "name": "python",
            "ok": false,
            "required": true,
            "version": null,
            "text": "not found as `python` or `python3`",
            "fix": null,
        })
    );
    let back: DoctorReport = serde_json::from_value(json).expect("reading it back");
    assert_eq!(back, report);
}

#[test]
fn the_printed_report_tags_each_line_and_counts_the_missing() {
    let report = evaluate(
        &MachineFacts {
            python: None,
            ..all_found()
        },
        false,
    );
    let mut out = Vec::new();
    print_report(&report, &mut out).expect("printing");
    let out = String::from_utf8(out).expect("utf-8");
    assert!(out.contains("[ok  ] git: git version 2.47.1"), "{out}");
    assert!(out.contains("[miss] python:"), "{out}");
    assert!(out.contains("result: 1 required tool(s) missing"), "{out}");
}
