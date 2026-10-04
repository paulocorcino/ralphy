use super::*;

// --- resolve_opencode_model precedence ---
/// The flag wins over the persisted model; an absent or empty flag falls
/// through to it; nothing anywhere is `None`.
#[test]
fn resolve_opencode_model_precedence() {
    // (case, flag, persisted, expected)
    type Row<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Option<&'a str>);
    let rows: [Row; 4] = [
        ("flag wins", Some("flag"), Some("persisted"), Some("flag")),
        (
            "persisted when the flag is absent",
            None,
            Some("kimi-for-coding/k2p7"),
            Some("kimi-for-coding/k2p7"),
        ),
        ("both unset", None, None, None),
        (
            "an empty flag falls through",
            Some(""),
            Some("k2p7"),
            Some("k2p7"),
        ),
    ];
    for (case, flag, persisted, want) in rows {
        assert_eq!(
            resolve_opencode_model(flag.map(str::to_string), persisted.map(str::to_string)),
            want.map(str::to_string),
            "{case}"
        );
    }
}

// --- resolve_str / resolve_u64 precedence ---
/// The flag wins, then the persisted value, then the default; an empty flag
/// or an empty persisted value counts as absent, and with neither set the
/// default comes back byte for byte.
#[test]
fn resolve_str_precedence() {
    // (case, flag, persisted, default, expected)
    type Row<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a str, &'a str);
    let rows: [Row; 5] = [
        (
            "flag wins",
            Some("flag"),
            Some("persisted"),
            "default",
            "flag",
        ),
        (
            "persisted when the flag is absent",
            None,
            Some("persisted"),
            "default",
            "persisted",
        ),
        (
            "an empty flag falls through",
            Some(""),
            Some("persisted"),
            "default",
            "persisted",
        ),
        (
            "an empty persisted value falls through",
            None,
            Some(""),
            "default",
            "default",
        ),
        (
            "the default verbatim",
            None,
            None,
            "origin/main",
            "origin/main",
        ),
    ];
    for (case, flag, persisted, default, want) in rows {
        assert_eq!(
            resolve_str(
                flag.map(str::to_string),
                persisted.map(str::to_string),
                default
            ),
            want,
            "{case}"
        );
    }
}

#[test]
fn resolve_effort_applies_precedence_and_validates_raw_settings() {
    assert_eq!(
        resolve_effort(Some(Effort::High), Some("low".into()), Some(Effort::Medium)).unwrap(),
        Some(Effort::High)
    );
    assert_eq!(
        resolve_effort(None, Some("low".into()), Some(Effort::Medium)).unwrap(),
        Some(Effort::Low)
    );
    assert_eq!(
        resolve_effort(None, None, Some(Effort::Medium)).unwrap(),
        Some(Effort::Medium)
    );
    assert_eq!(resolve_effort(None, None, None).unwrap(), None);
    assert!(resolve_effort(None, Some("hihg".into()), None).is_err());
}

#[test]
fn resolve_u64_flag_wins_then_persisted_then_default() {
    assert_eq!(resolve_u64(Some(10), Some(20), 90), 10);
    assert_eq!(resolve_u64(None, Some(20), 90), 20);
    assert_eq!(resolve_u64(None, None, 90), 90);
    // An explicit `0` (`--max-minutes-per-issue 0`) is a deliberate "no cap",
    // not an absent value.
    assert_eq!(resolve_u64(Some(0), Some(90), 30), 0);
}

// --- resolve_assignee precedence ---

#[test]
fn resolve_assignee_precedence() {
    // --assignee X wins over config.
    assert_eq!(
        resolve_assignee(Some("X"), false, Some("cfg")),
        Some("X".to_string())
    );
    // --no-assignee forces None even over a set config.
    assert_eq!(resolve_assignee(None, true, Some("cfg")), None);
    // Neither flag: persisted config is used.
    assert_eq!(
        resolve_assignee(None, false, Some("cfg")),
        Some("cfg".to_string())
    );
    // Nothing anywhere: no filter.
    assert_eq!(resolve_assignee(None, false, None), None);
    // Empty strings are treated as unset: an empty flag falls through to the
    // persisted value, and an empty persisted value falls through to None.
    assert_eq!(
        resolve_assignee(Some(""), false, Some("cfg")),
        Some("cfg".to_string())
    );
    assert_eq!(resolve_assignee(None, false, Some("")), None);
}

// --- resolve_remote_control precedence ---

#[test]
fn resolve_remote_control_precedence() {
    // flag on wins over persisted off.
    assert!(resolve_remote_control(true, false, Some(false)));
    // --no wins over persisted on.
    assert!(!resolve_remote_control(false, true, Some(true)));
    // persisted on used when no flag.
    assert!(resolve_remote_control(false, false, Some(true)));
    // default OFF.
    assert!(!resolve_remote_control(false, false, None));
    // remote_control precedes no_remote_control.
    assert!(resolve_remote_control(true, true, None));
}

// --- parse_branch_mode ---

#[test]
fn parse_branch_mode_ok_arms() {
    assert_eq!(parse_branch_mode("new").unwrap(), BranchMode::New);
    assert_eq!(parse_branch_mode("current").unwrap(), BranchMode::Current);
}

#[test]
fn parse_branch_mode_rejects_unknown() {
    let err = parse_branch_mode("sideways").unwrap_err();
    assert!(
        err.to_string().contains("must be 'new' or 'current'"),
        "got: {err}"
    );
}
