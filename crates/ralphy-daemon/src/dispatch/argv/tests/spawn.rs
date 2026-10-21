use super::*;

/// The list verbs take no client input: their argv is fixed.
#[test]
fn list_verbs_argv_is_static() {
    // (case, argv, expected)
    let rows = [
        (
            "branch.list",
            branch_list_argv(),
            vec!["branch", "list", "--format", "json"],
        ),
        (
            "worktree.list",
            worktree_list_argv(),
            vec!["worktree", "list", "--format", "json"],
        ),
        (
            "changes.list",
            changes_list_argv(),
            vec!["changes", "list", "--format", "json"],
        ),
        (
            "board",
            board_argv(),
            vec!["issues", "--format", "json", "--board"],
        ),
    ];
    for (case, argv, want) in rows {
        assert_eq!(argv, want, "{case}");
    }
    for (query, verb) in [
        ("worktree.list", Verb::WorktreeList),
        ("changes.list", Verb::ChangesList),
    ] {
        assert_eq!(Verb::from_query(query), Some(verb), "{query}");
        assert_eq!(verb.effect_class(), EffectClass::Query, "{query}");
    }
}

#[test]
fn label_argv_composes_single_token_op() {
    assert_eq!(
        label_argv(&serde_json::json!({ "number": 7, "label": "AFK", "op": "add" })).unwrap(),
        vec!["label", "set", "7", "--add=AFK"]
    );
    assert_eq!(
        label_argv(&serde_json::json!({ "number": 7, "label": "AFK", "op": "remove" })).unwrap(),
        vec!["label", "set", "7", "--remove=AFK"]
    );
    // Zero/absent number, empty label, and out-of-enum op are all refused.
    assert_eq!(
        label_argv(&serde_json::json!({ "number": 0, "label": "AFK", "op": "add" })),
        Err(ArgvError::BadParam("number"))
    );
    assert_eq!(
        label_argv(&serde_json::json!({ "number": 7, "label": "", "op": "add" })),
        Err(ArgvError::BadParam("label"))
    );
    assert_eq!(
        label_argv(&serde_json::json!({ "number": 7, "label": "AFK", "op": "toggle" })),
        Err(ArgvError::BadParam("op"))
    );
}

#[test]
fn issue_show_argv_validates_number() {
    // A positive integer composes the detail argv.
    assert_eq!(
        issue_show_argv(&serde_json::json!({ "number": 42 })).unwrap(),
        vec!["issues", "show", "42", "--format", "json"]
    );
    // Zero, missing, and non-integer are all refused — no argv reaches the CLI.
    assert_eq!(
        issue_show_argv(&serde_json::json!({ "number": 0 })),
        Err(ArgvError::BadParam("number"))
    );
    assert_eq!(
        issue_show_argv(&serde_json::json!({})),
        Err(ArgvError::BadParam("number"))
    );
    assert_eq!(
        issue_show_argv(&serde_json::json!({ "number": "12" })),
        Err(ArgvError::BadParam("number"))
    );
}

#[test]
fn spawn_argv_static_verbs() {
    assert_eq!(
        spawn_argv(Verb::Triage, &serde_json::json!({})).unwrap(),
        vec!["triage", "--if-idle", "--yes"]
    );
    assert_eq!(
        spawn_argv(Verb::PushQueue, &serde_json::json!({})).unwrap(),
        vec!["issues", "--push"]
    );
}

#[test]
fn spawn_argv_run_composes_validated_flags() {
    // Executor-only, new branch.
    assert_eq!(
        spawn_argv(
            Verb::Run,
            &serde_json::json!({ "agent": "claude", "branchMode": "new" })
        )
        .unwrap(),
        vec![
            "run",
            "--if-idle",
            "--agent",
            "claude",
            "--branch-mode",
            "new"
        ]
    );
    // Split planner + current branch.
    assert_eq!(
        spawn_argv(
            Verb::Run,
            &serde_json::json!({
                "agent": "opencode",
                "planAgent": "claude",
                "branchMode": "current"
            })
        )
        .unwrap(),
        vec![
            "run",
            "--if-idle",
            "--agent",
            "opencode",
            "--plan-agent",
            "claude",
            "--branch-mode",
            "current"
        ]
    );
    // A JSON-null planAgent is omitted, not refused (the modal sends null when
    // not split).
    assert_eq!(
        spawn_argv(
            Verb::Run,
            &serde_json::json!({ "agent": "claude", "planAgent": null, "branchMode": "new" })
        )
        .unwrap(),
        vec![
            "run",
            "--if-idle",
            "--agent",
            "claude",
            "--branch-mode",
            "new"
        ]
    );
}

/// Every agent the daemon knows reaches the CLI as its own `--agent` value:
/// a vendor missing from the daemon's enum refused a workbench run with
/// `BadParam("agent")` (Kimi, issue #228), and the value is the CLI's name,
/// never a binary name such as `cursor-agent` (#248).
#[test]
fn spawn_argv_carries_every_agent_through_to_the_agent_flag() {
    for a in crate::session::Agent::ALL {
        let flag = crate::dispatch::agent_flag(a);
        assert_eq!(
            flag,
            format!("{a:?}").to_lowercase(),
            "{a:?}: the CLI's name"
        );
        assert_eq!(
            spawn_argv(Verb::Run, &json!({ "agent": flag, "branchMode": "new" })).unwrap(),
            vec!["run", "--if-idle", "--agent", flag, "--branch-mode", "new"],
            "{a:?}"
        );
    }
}

#[test]
fn spawn_argv_refuses_out_of_enum_params() {
    // Out-of-enum or free-text (a shell injection attempt) never reaches argv.
    assert_eq!(
        spawn_argv(
            Verb::Run,
            &serde_json::json!({ "agent": "bogus", "branchMode": "new" })
        ),
        Err(ArgvError::BadParam("agent"))
    );
    assert_eq!(
        spawn_argv(
            Verb::Run,
            &serde_json::json!({ "agent": "claude", "branchMode": "sideways" })
        ),
        Err(ArgvError::BadParam("branchMode"))
    );
    assert_eq!(
        spawn_argv(
            Verb::Run,
            &serde_json::json!({ "agent": "claude", "planAgent": "x;rm", "branchMode": "new" })
        ),
        Err(ArgvError::BadParam("planAgent"))
    );
    // Absent required params are refused too.
    assert_eq!(
        spawn_argv(Verb::Run, &serde_json::json!({ "branchMode": "new" })),
        Err(ArgvError::BadParam("agent"))
    );
    assert_eq!(
        spawn_argv(Verb::Run, &serde_json::json!({ "agent": "claude" })),
        Err(ArgvError::BadParam("branchMode"))
    );
}

/// The run-stop verb: a Mutate (not a Spawn), a fixed two-token argv, and a
/// `runid` validated tightly enough that it can never become a flag, a path,
/// or a second argument.
#[test]
fn run_stop_argv_composes_an_exact_vector_and_refuses_anything_odd() {
    assert_eq!(Verb::from_query("run.stop"), Some(Verb::RunStop));
    assert_eq!(
        Verb::RunStop.effect_class(),
        EffectClass::Mutate,
        "a stop must never reach the Spawn path"
    );

    let argv = run_stop_argv(&json!({ "runid": "01J8ZQK4N7T5V9WQ0X2Y3Z4A5B" })).unwrap();
    assert_eq!(
        argv,
        vec!["stop", "--runid=01J8ZQK4N7T5V9WQ0X2Y3Z4A5B"],
        "the argv is fixed by the verb; the client contributes one token"
    );
    assert_eq!(
        argv.len(),
        2,
        "the runid is ONE token — a split one dies in clap"
    );

    // Refusals, each with NO argv. `-`/`=`/separators/whitespace are the
    // shapes that could turn one token into two, or into a flag.
    for bad in [
        "",
        "-rf",
        "--repo=/etc",
        "a b",
        "a/b",
        "a\\b",
        "a=b",
        "01J-8ZQ",
        "01J\n8ZQ",
        &"0".repeat(65),
    ] {
        assert_eq!(
            run_stop_argv(&json!({ "runid": bad })),
            Err(ArgvError::BadParam("runid")),
            "{bad:?} must yield NO argv"
        );
    }
    // …and a missing or non-string runid, since the browser always knows it.
    assert_eq!(run_stop_argv(&json!({})), Err(ArgvError::BadParam("runid")));
    assert_eq!(
        run_stop_argv(&json!({ "runid": 7 })),
        Err(ArgvError::BadParam("runid"))
    );
}
