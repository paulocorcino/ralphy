use super::*;

fn all_green() -> EnvFindings {
    EnvFindings {
        git: true,
        python: true,
        gh_authenticated: true,
        github_remote: true,
        agents_present: vec![Agent::Claude, Agent::Codex],
        agents_logged_in: vec![Agent::Claude],
    }
}

// Capability values are authored in each adapter crate; the CLI only routes.
#[test]
fn accepts_images_reflects_crate_consts() {
    assert!(Agent::Claude.accepts_images());
    assert!(Agent::Codex.accepts_images());
    assert!(Agent::Copilot.accepts_images());
    assert!(!Agent::Cursor.accepts_images());
    assert!(Agent::Gemini.accepts_images());
    assert!(!Agent::Kimi.accepts_images());
    assert!(!Agent::Opencode.accepts_images());
    // ALL must hold every variant once: a new variant that never joins ALL
    // is invisible to `ralphy init`'s agent report. `value_variants` is the
    // enum's own list, so this holds with no count to update.
    let variants = Agent::value_variants();
    assert_eq!(Agent::ALL.len(), variants.len(), "{:?}", Agent::ALL);
    for a in variants {
        assert!(Agent::ALL.contains(a), "{a:?} is missing from Agent::ALL");
    }
}

/// `init`/`triage` auto-selection takes the FIRST logged-in agent in `ALL`, and
/// `Agent::ALL` is the auto-selection ORDER for a no-flag `ralphy init`/
/// `triage` on a multi-login machine. The one-shots exist now (#237), so the
/// reason to pin Copilot last is no longer a missing `tasks.rs` — it is
/// auto-selection STABILITY: promoting Copilot would silently change which
/// vendor drives a no-flag run, a behavior change no issue has asked for.
/// The rule generalizes: each newcomer is APPENDED, so no existing vendor's
/// auto-selection position ever moves.
#[test]
fn newcomers_go_last_in_all() {
    // The whole order, not only the tail: the prefix is the
    // auto-selection preference, and a newcomer extends it at the end.
    assert_eq!(
        Agent::ALL,
        [
            Agent::Claude,
            Agent::Codex,
            Agent::Kimi,
            Agent::Opencode,
            Agent::Copilot,
            Agent::Cursor,
            Agent::Gemini,
        ]
    );
}

/// Each environment finding maps to the blockers the gate reports, in
/// order. Missing git LEADS: `git::origin_url` fails without git, so the
/// operator is told the root cause, not the "no GitHub remote" symptom it
/// would otherwise masquerade as. One logged-in agent of two passes.
#[test]
fn evaluate_gate_reports_each_missing_prerequisite() {
    // (case, findings, expected blockers)
    let rows: [(&str, EnvFindings, Vec<HardFail>); 8] = [
        ("all green", all_green(), vec![]),
        (
            "missing git",
            EnvFindings {
                git: false,
                ..all_green()
            },
            vec![HardFail::MissingGit],
        ),
        (
            "missing python",
            EnvFindings {
                python: false,
                ..all_green()
            },
            vec![HardFail::MissingPython],
        ),
        (
            "gh not authenticated",
            EnvFindings {
                gh_authenticated: false,
                ..all_green()
            },
            vec![HardFail::GhNotAuthenticated],
        ),
        (
            "no github remote",
            EnvFindings {
                github_remote: false,
                ..all_green()
            },
            vec![HardFail::NoGithubRemote],
        ),
        (
            "no agent cli",
            EnvFindings {
                agents_present: vec![],
                agents_logged_in: vec![],
                ..all_green()
            },
            vec![HardFail::NoAgentCli],
        ),
        (
            "agents present, none logged in",
            EnvFindings {
                agents_present: vec![Agent::Claude, Agent::Codex],
                agents_logged_in: vec![],
                ..all_green()
            },
            vec![HardFail::NoAgentLoggedIn],
        ),
        (
            "one of two logged in",
            EnvFindings {
                agents_present: vec![Agent::Claude, Agent::Codex],
                agents_logged_in: vec![Agent::Codex],
                ..all_green()
            },
            vec![],
        ),
    ];
    for (case, findings, want) in rows {
        assert_eq!(evaluate_gate(&findings), want, "{case}");
    }
}

// (b1) The report names git and points at Git for Windows when it is missing.
#[test]
fn format_report_names_git_and_its_remediation_when_missing() {
    let f = EnvFindings {
        git: false,
        ..all_green()
    };
    let report = format_report(&f, &evaluate_gate(&f));
    assert!(report.contains("git:           MISSING"), "{report}");
    assert!(report.contains("git-scm.com/download/win"), "{report}");
}

// (h) format_report literal substring assertions.
#[test]
fn format_report_logged_in_and_not_logged_in_substrings() {
    let f = EnvFindings {
        git: true,
        python: true,
        gh_authenticated: true,
        github_remote: true,
        agents_present: vec![Agent::Claude, Agent::Codex],
        agents_logged_in: vec![Agent::Claude],
    };
    let fails = evaluate_gate(&f);
    let report = format_report(&f, &fails);
    assert!(
        report.contains("claude: logged in"),
        "expected 'claude: logged in' in:\n{report}"
    );
    assert!(
        report.contains("codex: not logged in"),
        "expected 'codex: not logged in' in:\n{report}"
    );
}
/// #257: the router tax is priced into the operator's first read of `init`,
/// and only when this vendor is actually installed.
#[test]
fn the_report_surfaces_geminis_router_tax() {
    let with_gemini = EnvFindings {
        git: true,
        python: true,
        gh_authenticated: true,
        github_remote: true,
        agents_present: vec![Agent::Gemini],
        agents_logged_in: vec![Agent::Gemini],
    };
    let report = format_report(&with_gemini, &evaluate_gate(&with_gemini));
    assert!(report.contains("gemini.exec_model"), "{report}");
    assert!(report.contains("halve request consumption"), "{report}");

    let without = EnvFindings {
        agents_present: vec![Agent::Claude],
        agents_logged_in: vec![Agent::Claude],
        ..with_gemini
    };
    let report = format_report(&without, &evaluate_gate(&without));
    assert!(!report.contains("gemini.exec_model"), "{report}");
    assert!(!report.contains("halve request consumption"), "{report}");
}

/// The whole report of a machine with git and one logged-in agent, and
/// with python, the gh login and the GitHub remote missing.
#[test]
fn format_report_of_a_machine_with_three_blockers() {
    let f = EnvFindings {
        python: false,
        gh_authenticated: false,
        github_remote: false,
        agents_present: vec![Agent::Claude],
        agents_logged_in: vec![Agent::Claude],
        ..all_green()
    };
    let want = "git:           ok
python:        MISSING
gh auth:       NOT AUTHENTICATED
github remote: NO GITHUB REMOTE
agents:
  claude: logged in
  codex: absent
  kimi: absent
  opencode: absent
  copilot: absent
  cursor: absent
  gemini: absent
result: 3 blocker(s)
";
    assert_eq!(format_report(&f, &evaluate_gate(&f)), want);
}
