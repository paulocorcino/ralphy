use super::*;

#[test]
fn project_remove_argv_composes_the_subcommand() {
    assert_eq!(
        project_remove_argv(&json!({ "slug": "owner/repo" })).unwrap(),
        vec!["daemon", "remove", "owner/repo"]
    );
    assert_eq!(
        project_remove_argv(&json!({ "slug": "my-org.x/some_repo.rs" })).unwrap(),
        vec!["daemon", "remove", "my-org.x/some_repo.rs"]
    );
    // The remoteless form `git::project_slug` falls back to: a SINGLE
    // segment. Refusing it would make every repo without a forge remote
    // unremovable — which is what the first browser pass found.
    assert_eq!(
        project_remove_argv(&json!({ "slug": "path-47f82f1bdb2a7517" })).unwrap(),
        vec!["daemon", "remove", "path-47f82f1bdb2a7517"]
    );
    // `slug_from_url` takes the remote's last two segments verbatim, so a
    // self-hosted `ssh://git@host/~paulo/repo` registers as `~paulo/repo`.
    // Refusing it would leave that project permanently unremovable.
    assert_eq!(
        project_remove_argv(&json!({ "slug": "~paulo/repo" })).unwrap(),
        vec!["daemon", "remove", "~paulo/repo"]
    );
    // The bounds, asserted rather than assumed.
    let over = "a".repeat(MAX_SLUG_CHARS + 1);
    assert_eq!(
        project_remove_argv(&json!({ "slug": over })),
        Err(ArgvError::BadParam("slug"))
    );
    assert!(project_remove_argv(&json!({ "slug": "a".repeat(MAX_SLUG_CHARS) })).is_ok());
    // A non-string slug is not a slug.
    assert_eq!(
        project_remove_argv(&json!({ "slug": 7 })),
        Err(ArgvError::BadParam("slug"))
    );
}

#[test]
fn project_remove_argv_refuses_a_bad_slug() {
    for bad in [
        "",
        "owner/",
        "/repo",
        "owner/re po",
        "owner/repo;rm",
        "owner/repo/extra",
        "--flag/repo",
        "-repo",
        "owner/../repo",
        "owner\\repo",
        "a/b/c",
    ] {
        assert_eq!(
            project_remove_argv(&json!({ "slug": bad })),
            Err(ArgvError::BadParam("slug")),
            "slug {bad:?} must be refused"
        );
    }
    assert_eq!(
        project_remove_argv(&json!({})),
        Err(ArgvError::BadParam("slug"))
    );
}

#[test]
fn sync_argv_composes_exact_vectors_and_refuses() {
    assert_eq!(Verb::from_query("sync.status"), Some(Verb::SyncStatus));
    assert_eq!(Verb::from_query("sync.fetch"), Some(Verb::SyncFetch));
    assert_eq!(Verb::from_query("sync.pull"), Some(Verb::SyncPull));
    assert_eq!(Verb::from_query("sync.push"), Some(Verb::SyncPush));

    assert_eq!(
        sync_status_argv(),
        vec!["sync", "status", "--format", "json"]
    );
    assert_eq!(sync_argv(Verb::SyncFetch).unwrap(), vec!["sync", "fetch"]);
    assert_eq!(sync_argv(Verb::SyncPull).unwrap(), vec!["sync", "pull"]);
    assert_eq!(sync_argv(Verb::SyncPush).unwrap(), vec!["sync", "push"]);

    // The verbs carry no client input, so the verb itself is the only
    // parameter there is to malform — and a bad one yields NO argv.
    assert_eq!(sync_argv(Verb::Run), Err(ArgvError::BadParam("verb")));
    assert_eq!(
        sync_argv(Verb::ChangesList),
        Err(ArgvError::BadParam("verb"))
    );
    assert_eq!(
        spawn_argv(Verb::SyncFetch, &serde_json::json!({})),
        Err(ArgvError::BadParam("verb")),
        "a Mutate verb never reaches the spawn path"
    );
    assert_eq!(
        spawn_argv(Verb::SyncStatus, &serde_json::json!({})),
        Err(ArgvError::BadParam("verb"))
    );
}

#[test]
fn branch_argv_composes_guarded_vectors() {
    assert_eq!(
        branch_argv(Verb::BranchSwitch, &serde_json::json!({ "name": "feat/x" })).unwrap(),
        vec!["branch", "switch", "--", "feat/x"]
    );
    assert_eq!(
        branch_argv(Verb::BranchCreate, &serde_json::json!({ "name": "feat/x" })).unwrap(),
        vec!["branch", "create", "--", "feat/x"]
    );
    // Empty / whitespace-only / absent name never reaches argv.
    assert_eq!(
        branch_argv(Verb::BranchSwitch, &serde_json::json!({ "name": "" })),
        Err(ArgvError::BadParam("name"))
    );
    assert_eq!(
        branch_argv(Verb::BranchSwitch, &serde_json::json!({ "name": "   " })),
        Err(ArgvError::BadParam("name"))
    );
    assert_eq!(
        branch_argv(Verb::BranchCreate, &serde_json::json!({})),
        Err(ArgvError::BadParam("name"))
    );
}

/// The ref shape gate (audit F10/F11): what git would read as an option or
/// refuse as a ref never reaches the CLI, and every legitimate spelling of a
/// branch or base does. Both verbs share it, so both are pinned.
#[test]
fn branch_and_base_names_are_shape_gated() {
    for bad in [
        "-b",
        "--pathspec-from-file=.gitignore",
        "--detach",
        "a..b",
        "a b",
        "x.lock",
        "feat/",
        "feat/.hidden",
        "a@{1}",
        "a:b",
        "a~1",
        "a^",
        "a?b",
        "a*",
        "a[b",
        "a\\b",
        "a\u{7}b",
        "@",
        "a//b",
    ] {
        assert_eq!(
            branch_argv(Verb::BranchSwitch, &serde_json::json!({ "name": bad })),
            Err(ArgvError::BadParam("name")),
            "name {bad:?}"
        );
        assert_eq!(
            worktree_add_argv(&serde_json::json!({ "name": "wt-x", "base": bad })),
            Err(ArgvError::BadParam("base")),
            "base {bad:?}"
        );
    }
    for good in [
        "main",
        "feat/x",
        "origin/main",
        "release-1.2",
        "user/feat_x",
        "0123456789abcdef0123456789abcdef01234567",
    ] {
        assert_eq!(
            branch_argv(Verb::BranchSwitch, &serde_json::json!({ "name": good })).unwrap(),
            vec!["branch", "switch", "--", good],
            "name {good:?}"
        );
        assert_eq!(
            worktree_add_argv(&serde_json::json!({ "name": "wt-x", "base": good })).unwrap(),
            vec!["worktree", "add", &format!("--base={good}"), "--", "wt-x"],
            "base {good:?}"
        );
    }
}

#[test]
fn worktree_add_argv_composes_guarded_vector() {
    assert_eq!(
        worktree_add_argv(&serde_json::json!({ "name": "wt-x" })).unwrap(),
        vec!["worktree", "add", "--", "wt-x"]
    );
    assert_eq!(
        worktree_add_argv(&serde_json::json!({ "name": "wt-x", "base": "main" })).unwrap(),
        vec!["worktree", "add", "--base=main", "--", "wt-x"]
    );
    assert_eq!(
        worktree_add_argv(&serde_json::json!({ "name": "wt-x", "base": null })).unwrap(),
        vec!["worktree", "add", "--", "wt-x"]
    );
    // Empty / absent name never reaches argv; a present-but-empty base is malformed.
    assert_eq!(
        worktree_add_argv(&serde_json::json!({ "name": "" })),
        Err(ArgvError::BadParam("name"))
    );
    assert_eq!(
        worktree_add_argv(&serde_json::json!({})),
        Err(ArgvError::BadParam("name"))
    );
    assert_eq!(
        worktree_add_argv(&serde_json::json!({ "name": "x", "base": "" })),
        Err(ArgvError::BadParam("base"))
    );
    assert_eq!(
        worktree_add_argv(&serde_json::json!({ "name": "x", "base": 7 })),
        Err(ArgvError::BadParam("base"))
    );
    assert_eq!(Verb::from_query("worktree.add"), Some(Verb::WorktreeAdd));
    assert_eq!(Verb::WorktreeAdd.effect_class(), EffectClass::Mutate);
}

#[test]
fn worktree_remove_argv_composes_guarded_vector() {
    assert_eq!(
        worktree_remove_argv(&serde_json::json!({ "name": "wt-x" })).unwrap(),
        vec!["worktree", "remove", "--", "wt-x"]
    );
    for payload in [
        serde_json::json!({ "name": "" }),
        serde_json::json!({ "name": "  " }),
        serde_json::json!({}),
    ] {
        assert_eq!(
            worktree_remove_argv(&payload),
            Err(ArgvError::BadParam("name")),
            "{payload}"
        );
    }
    assert_eq!(
        Verb::from_query("worktree.remove"),
        Some(Verb::WorktreeRemove)
    );
    assert_eq!(Verb::WorktreeRemove.effect_class(), EffectClass::Mutate);
    assert!(!Verb::WorktreeRemove.takes_checkout_cwd());
}

#[test]
fn config_argv_composes_exact_vectors() {
    assert_eq!(
        config_argv(Verb::ConfigGet, &serde_json::json!({})).unwrap(),
        vec!["config", "get", "--json"]
    );
    assert_eq!(
        config_argv(
            Verb::ConfigSet,
            &serde_json::json!({ "key": "branch_mode", "value": "new" })
        )
        .unwrap(),
        vec!["config", "set", "--", "branch_mode", "new"]
    );
    assert_eq!(
        config_argv(
            Verb::ConfigUnset,
            &serde_json::json!({ "key": "branch_mode" })
        )
        .unwrap(),
        vec!["config", "unset", "--", "branch_mode"]
    );
    // A dash-leading value is stored, not parsed as a flag (the `--` guard).
    assert_eq!(
        config_argv(
            Verb::ConfigSet,
            &serde_json::json!({ "key": "opencode.model", "value": "--weird" })
        )
        .unwrap(),
        vec!["config", "set", "--", "opencode.model", "--weird"]
    );
}

#[test]
fn config_argv_refuses_local_only_keys() {
    // Each is well-shaped and a genuinely supported CLI key, so neither the
    // character class nor the CLI's `require_known_key` refuses it. The daemon
    // refuses the KEY itself — remotely, only: `verify.command` becomes argv[0]
    // of the verify gate's child, the two hatches re-arm what an ADR closed,
    // `events.token` is the sink's bearer (audit F12),
    // `queue.trust_all_comments` lets a stranger's comment reach the prompt,
    // the guard proof waiver lets a session run with no proof of the guard.
    for key in [
        "verify.command",
        "copilot.allow_builtin_mcp_servers_i_understand_the_risk",
        "cursor.allow_codebase_indexing_i_understand_the_risk",
        "events.token",
        "queue.trust_all_comments",
        "claude.run_without_guard_proof_i_understand_the_risk",
    ] {
        assert_eq!(
            config_argv(
                Verb::ConfigSet,
                &serde_json::json!({ "key": key, "value": "true" })
            ),
            Err(ArgvError::BadParam("key")),
            "set {key}"
        );
        assert_eq!(
            config_argv(Verb::ConfigUnset, &serde_json::json!({ "key": key })),
            Err(ArgvError::BadParam("key")),
            "unset {key}"
        );
    }
    // The sink's URL stays settable from the panel — only its bearer is local.
    assert!(config_argv(
        Verb::ConfigSet,
        &serde_json::json!({ "key": "events.url", "value": "https://sink.example/e" })
    )
    .is_ok());
    // A neighbouring verify key stays settable — the denylist is one key, not
    // a namespace.
    assert!(config_argv(
        Verb::ConfigSet,
        &serde_json::json!({ "key": "verify.require_verify_gate", "value": "true" })
    )
    .is_ok());
}

#[test]
fn config_argv_refuses_ill_shaped_key_and_empty_value() {
    // An out-of-class or empty key never reaches argv.
    assert_eq!(
        config_argv(
            Verb::ConfigSet,
            &serde_json::json!({ "key": "bad key!", "value": "x" })
        ),
        Err(ArgvError::BadParam("key"))
    );
    assert_eq!(
        config_argv(
            Verb::ConfigSet,
            &serde_json::json!({ "key": "", "value": "x" })
        ),
        Err(ArgvError::BadParam("key"))
    );
    assert_eq!(
        config_argv(Verb::ConfigUnset, &serde_json::json!({ "key": "Bad.Key" })),
        Err(ArgvError::BadParam("key"))
    );
    // An empty/absent value refuses `set`.
    assert_eq!(
        config_argv(
            Verb::ConfigSet,
            &serde_json::json!({ "key": "branch_mode", "value": "" })
        ),
        Err(ArgvError::BadParam("value"))
    );
    assert_eq!(
        config_argv(
            Verb::ConfigSet,
            &serde_json::json!({ "key": "branch_mode" })
        ),
        Err(ArgvError::BadParam("value"))
    );
}
