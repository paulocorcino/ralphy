use serde_json::json;

use super::*;

/// An absolute path on the platform that runs the test.
fn absolute() -> &'static str {
    if cfg!(windows) {
        r"C:\Dev\-tricky"
    } else {
        "/home/me/-tricky"
    }
}

#[test]
fn project_add_argv_puts_the_separator_before_the_path() {
    let argv = project_add_argv(&json!({ "path": absolute() })).expect("valid");
    assert_eq!(argv, ["daemon", "add", "--", absolute()]);
}

#[test]
fn project_add_argv_adds_init_only_when_asked() {
    let with = project_add_argv(&json!({ "path": absolute(), "init": true })).expect("valid");
    assert_eq!(with, ["daemon", "add", "--init", "--", absolute()]);
    let without = project_add_argv(&json!({ "path": absolute(), "init": false })).expect("valid");
    assert_eq!(without, ["daemon", "add", "--", absolute()]);
}

#[test]
fn project_add_argv_adds_create_only_with_init() {
    let with = project_add_argv(&json!({ "path": absolute(), "init": true, "create": true }))
        .expect("valid");
    assert_eq!(
        with,
        ["daemon", "add", "--init", "--create", "--", absolute()]
    );
    let without = project_add_argv(&json!({ "path": absolute(), "init": true, "create": false }))
        .expect("valid");
    assert_eq!(without, ["daemon", "add", "--init", "--", absolute()]);
}

#[test]
fn project_add_argv_refuses_a_bad_path() {
    let long = format!("{}{}", absolute(), "a".repeat(4096));
    let control = format!("{}\u{1b}x", absolute());
    for payload in [
        json!({}),
        json!({ "path": 7 }),
        json!({ "path": "" }),
        json!({ "path": "relative/dir" }),
        json!({ "path": long }),
        json!({ "path": control }),
        json!({ "path": absolute(), "init": "yes" }),
        json!({ "path": absolute(), "init": true, "create": "yes" }),
        json!({ "path": absolute(), "create": true }),
    ] {
        assert!(project_add_argv(&payload).is_err(), "{payload}");
    }
}

#[test]
fn the_registry_family_is_exactly_the_two_verbs() {
    let family: Vec<Verb> = Verb::ALL
        .iter()
        .copied()
        .filter(|v| v.is_registry())
        .collect();
    assert_eq!(family, [Verb::DirList, Verb::ProjectAdd]);
}

#[test]
fn project_add_argv_adds_the_git_identity_after_init() {
    let payload = json!({
        "path": absolute(), "init": true, "gitName": "Ana Lima", "gitEmail": "ana@example.com"
    });
    let argv = project_add_argv(&payload).expect("valid");
    assert_eq!(
        argv,
        [
            "daemon",
            "add",
            "--init",
            "--git-name",
            "Ana Lima",
            "--git-email",
            "ana@example.com",
            "--",
            absolute()
        ]
    );
}

#[test]
fn project_add_argv_refuses_a_bad_git_identity() {
    let long = "a".repeat(257);
    for (name, email) in [
        (json!("Ana"), json!(null)),
        (json!(null), json!("ana@example.com")),
        (json!(""), json!("ana@example.com")),
        (json!(" Ana"), json!("ana@example.com")),
        (json!("-Ana"), json!("ana@example.com")),
        (json!("An\na"), json!("ana@example.com")),
        (json!(long), json!("ana@example.com")),
        (json!(7), json!("ana@example.com")),
        (json!("Ana"), json!("ana")),
        (json!("Ana"), json!("@example.com")),
        (json!("Ana"), json!("ana@")),
        (json!("Ana"), json!("ana lima@example.com")),
        (json!("Ana"), json!("<ana@example.com>")),
    ] {
        let payload =
            json!({ "path": absolute(), "init": true, "gitName": name, "gitEmail": email });
        assert!(project_add_argv(&payload).is_err(), "{payload}");
    }
    let without_init = json!({ "path": absolute(), "gitName": "Ana", "gitEmail": "a@b.c" });
    assert!(project_add_argv(&without_init).is_err(), "needs --init");
}
