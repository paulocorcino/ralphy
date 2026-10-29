use super::*;
use ralphy_proc_util::hook_form::HookCheck;

fn all_found() -> MachineFacts {
    MachineFacts {
        git: Some("git version 2.47.1".to_string()),
        gh: Some("gh version 2.89.0 (2026-03-01)".to_string()),
        gh_signed_in: true,
        python: Some("Python 3.12.4".to_string()),
        agents: vec![(Agent::Claude, "2.1.0 (Claude Code)".to_string())],
        linger: None,
        hooks: vec![(Agent::Claude, HookCheck::Ok)],
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
    assert_eq!(names, ["git", "gh", "python", "claude", "claude hooks"]);
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
    assert_eq!(
        json["tools"][4],
        serde_json::json!({
            "tool": "hooks",
            "name": "claude hooks",
            "ok": true,
            "required": false,
            "version": null,
            "text": "the hook command ran and wrote a status line",
            "fix": null,
        })
    );
    let back: DoctorReport = serde_json::from_value(json).expect("reading it back");
    assert_eq!(back, report);
}

fn two_agents(hooks: Vec<(Agent, HookCheck)>) -> MachineFacts {
    MachineFacts {
        agents: vec![
            (Agent::Claude, "2.1.0 (Claude Code)".to_string()),
            (Agent::Codex, "codex-cli 0.9".to_string()),
        ],
        hooks,
        ..all_found()
    }
}

#[test]
fn each_agent_has_a_hook_line() {
    let report = evaluate(&two_agents(vec![(Agent::Claude, HookCheck::Ok)]), false);
    let json = serde_json::to_value(&report).expect("serializing");
    let hooks: Vec<&serde_json::Value> = json["tools"]
        .as_array()
        .expect("a list")
        .iter()
        .filter(|t| t["tool"] == "hooks")
        .collect();
    let names: Vec<&str> = hooks.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["claude hooks", "codex hooks"]);
    assert!(hooks.iter().all(|t| t["required"] == false), "{hooks:?}");

    let none = evaluate(
        &MachineFacts {
            agents: vec![],
            hooks: vec![],
            ..all_found()
        },
        false,
    );
    assert!(
        !none.tools.iter().any(|t| t.tool == Tool::Hooks),
        "no agent, no hook line"
    );
}

#[test]
fn a_hook_line_warns_and_never_fails() {
    let rows = [
        (HookCheck::Ok, "[ok  ] claude hooks: the hook command ran"),
        (
            HookCheck::Failed {
                cause: "ralphy.exe did not start".to_string(),
            },
            "[warn] claude hooks: ralphy.exe did not start",
        ),
        (
            HookCheck::NotChecked {
                reason: "Claude Code 2.1.100 runs a hook through a shell".to_string(),
            },
            "[warn] claude hooks: not checked: Claude Code 2.1.100 runs a hook through a shell",
        ),
    ];
    for (check, want) in rows {
        let mut facts = two_agents(vec![(Agent::Claude, check)]);
        facts
            .agents
            .push((Agent::Copilot, "GitHub Copilot CLI 1.0".to_string()));
        let report = evaluate(&facts, false);
        assert_eq!(report.missing_required(), 0, "{want}");
        let mut out = Vec::new();
        print_report(&report, &mut out).expect("printing");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(
            out.contains(want),
            "{want}
{out}"
        );
        assert!(
            out.contains("[ok  ] copilot hooks: no state source"),
            "an agent with no state source is ok: {out}"
        );
        assert!(out.contains("result: every required tool is here"), "{out}");
    }
    // A hook result that never came (the probe found no program) is not ok.
    let report = evaluate(&two_agents(vec![]), false);
    assert!(!line(&report, "claude hooks").ok);
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

#[test]
fn a_missing_git_on_windows_says_how_to_install_it() {
    let facts = MachineFacts {
        git: None,
        ..all_found()
    };
    assert_eq!(
        line(&evaluate(&facts, true), "git").fix.as_deref(),
        Some("winget install --id Git.Git -e")
    );
    assert_eq!(line(&evaluate(&facts, false), "git").fix, None);
}

/// Codex gives its state through hooks: its doctor line is a hooks line
/// with the result of its own check, and the check runs the program and the
/// arguments a Codex console's hooks run.
#[test]
fn doctor_names_the_codex_state_source() {
    let rows = [
        (
            HookCheck::Ok,
            "[ok  ] codex hooks: the hook command ran and wrote a status line",
        ),
        (
            HookCheck::Failed {
                cause: "ralphy.exe exited with code 1".to_string(),
            },
            "[warn] codex hooks: ralphy.exe exited with code 1",
        ),
    ];
    for (check, want) in rows {
        let report = evaluate(&two_agents(vec![(Agent::Codex, check)]), false);
        let codex = line(&report, "codex hooks");
        assert_eq!(codex.tool, Tool::Hooks);
        let mut out = Vec::new();
        print_report(&report, &mut out).expect("printing");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(out.contains(want), "{want}\n{out}");
    }
    let hook = hook_command(Agent::Codex, Path::new("codex")).expect("Codex has hooks");
    let entry = hook.entry();
    assert_eq!(
        entry["args"],
        serde_json::json!(["hook", "status", "codex"])
    );
    assert!(hook_command(Agent::Copilot, Path::new("copilot")).is_none());
}
