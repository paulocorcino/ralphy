use super::*;
use crate::dispatch::EffectClass;
use serde_json::json;

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

/// The shared shape gate as its OWN unit, exercised directly rather than
/// through a builder — and with NO `#[cfg]` anywhere in it, which is what
/// makes "a Windows daemon refuses a POSIX-absolute path exactly as a Linux
/// one refuses a drive prefix" a test rather than a claim.
#[test]
fn validated_path_refuses_every_bad_shape_on_every_platform() {
    for bad in [
        "/etc/passwd",
        "C:\\Windows\\x",
        "D:/x",
        "../../secret",
        "src/../../secret",
        "-rf",
        "",
        // Pathspec MAGIC, which is a leading-`:` shape, not a glob char.
        ":(glob)x",
        ":(top)etc/passwd",
        ":!x",
    ] {
        assert_eq!(
            validated_path(bad),
            None,
            "{bad:?} must be refused on every platform"
        );
    }

    // Glob metacharacters are LEGAL FILENAMES and must survive: refusing
    // them made `app/blog/[slug]/page.tsx` unreadable in the diff tab and
    // poisoned every group action over such a repo. `--literal-pathspecs`
    // plus change-set membership are what stop a pattern from expanding.
    for legal in [
        "app/blog/[slug]/page.tsx",
        "[Content_Types].xml",
        "src/*.rs",
        "a?.txt",
        "*",
        "**/x",
    ] {
        assert_eq!(
            validated_path(legal).as_deref(),
            Some(legal),
            "{legal:?} is a legal filename and must pass through unchanged"
        );
    }

    // The positive control: a real path survives, and a Windows-shaped one
    // normalises to the SAME string rather than being a second spelling.
    assert_eq!(
        validated_path("src/main.rs").as_deref(),
        Some("src/main.rs")
    );
    assert_eq!(
        validated_path("src\\main.rs").as_deref(),
        Some("src/main.rs")
    );
    assert_eq!(
        validated_path("a file.txt").as_deref(),
        Some("a file.txt"),
        "a space is not a shape problem"
    );
    // `..` is refused as a SEGMENT, not as a substring — a real filename
    // containing dots stays readable.
    assert_eq!(validated_path("v..1/x").as_deref(), Some("v..1/x"));
}

/// The discard verb rides the SHARED builder — no second definition of what
/// a path is — so this pins its exact vector and that a malformed path
/// yields no argv at all.
#[test]
fn changes_discard_argv_composes_an_exact_vector() {
    assert_eq!(
        Verb::from_query("changes.discard"),
        Some(Verb::ChangesDiscard)
    );
    assert_eq!(
        changes_paths_argv(
            Verb::ChangesDiscard,
            &json!({ "paths": ["a.txt", "b/c.txt"] })
        )
        .unwrap(),
        vec!["changes", "discard", "--path=a.txt", "--path=b/c.txt"]
    );
    // A Windows-shaped path composes the same token as its POSIX spelling.
    assert_eq!(
        changes_paths_argv(Verb::ChangesDiscard, &json!({ "paths": ["b\\c.txt"] })).unwrap(),
        vec!["changes", "discard", "--path=b/c.txt"]
    );
    // The untracked-directory entry shape git itself reports.
    assert_eq!(
        changes_paths_argv(Verb::ChangesDiscard, &json!({ "paths": ["newdir/"] })).unwrap(),
        vec!["changes", "discard", "--path=newdir/"]
    );
}

#[test]
fn changes_discard_refuses_a_malformed_path_with_no_argv() {
    for bad in [
        "/etc/passwd",
        "C:\\x",
        "../secret",
        "-rf",
        ":(glob)x",
        ":!x",
        "",
    ] {
        assert_eq!(
            changes_paths_argv(Verb::ChangesDiscard, &json!({ "paths": [bad] })),
            Err(ArgvError::BadParam("paths")),
            "{bad:?} must yield NO argv"
        );
    }
    for empty in [
        json!({}),
        json!({ "paths": [] }),
        json!({ "paths": "a.txt" }),
    ] {
        assert_eq!(
            changes_paths_argv(Verb::ChangesDiscard, &empty),
            Err(ArgvError::BadParam("paths")),
            "{empty} must yield NO argv"
        );
    }
    // One bad element poisons the whole list — a partial argv would discard
    // the good half of a request the daemon refused, unrecoverably.
    assert_eq!(
        changes_paths_argv(
            Verb::ChangesDiscard,
            &json!({ "paths": ["ok.txt", "/etc/passwd"] })
        ),
        Err(ArgvError::BadParam("paths"))
    );
    let over: Vec<String> = (0..MAX_STAGE_PATHS + 1)
        .map(|i| format!("f{i}.txt"))
        .collect();
    assert_eq!(
        changes_paths_argv(Verb::ChangesDiscard, &json!({ "paths": over })),
        Err(ArgvError::BadParam("paths")),
        "257 paths is over MAX_STAGE_PATHS"
    );
}

#[test]
fn changes_mutate_argv_composes_exact_vectors_and_refuses() {
    assert_eq!(Verb::from_query("changes.stage"), Some(Verb::ChangesStage));
    assert_eq!(
        Verb::from_query("changes.unstage"),
        Some(Verb::ChangesUnstage)
    );
    assert_eq!(
        Verb::from_query("changes.commit"),
        Some(Verb::ChangesCommit)
    );

    assert_eq!(
        changes_paths_argv(
            Verb::ChangesStage,
            &json!({ "paths": ["a.txt", "b/c.txt"] })
        )
        .unwrap(),
        vec!["changes", "stage", "--path=a.txt", "--path=b/c.txt"]
    );
    assert_eq!(
        changes_paths_argv(
            Verb::ChangesUnstage,
            &json!({ "paths": ["a.txt", "b/c.txt"] })
        )
        .unwrap(),
        vec!["changes", "unstage", "--path=a.txt", "--path=b/c.txt"]
    );
    // A Windows-shaped path composes the SAME token as its POSIX spelling.
    assert_eq!(
        changes_paths_argv(Verb::ChangesStage, &json!({ "paths": ["b\\c.txt"] })).unwrap(),
        vec!["changes", "stage", "--path=b/c.txt"]
    );

    let commit = changes_commit_argv(&json!({ "message": "-oops" })).unwrap();
    assert_eq!(commit, vec!["changes", "commit", "--message=-oops"]);
    assert_eq!(
        commit.len(),
        3,
        "the message is ONE token — a split one dies in clap"
    );

    // Refusals, each with NO argv.
    for bad in ["/etc/passwd", "C:\\x", "../x", "-rf", ":(glob)x", ":!x", ""] {
        assert_eq!(
            changes_paths_argv(Verb::ChangesStage, &json!({ "paths": [bad] })),
            Err(ArgvError::BadParam("paths")),
            "{bad:?} must yield NO argv"
        );
    }
    // …while a bracketed route file — the Next.js convention — composes its
    // token unchanged. `--literal-pathspecs` is what keeps git from
    // expanding it, not a refusal here.
    assert_eq!(
        changes_paths_argv(
            Verb::ChangesStage,
            &json!({ "paths": ["app/blog/[slug]/page.tsx"] })
        )
        .unwrap(),
        vec!["changes", "stage", "--path=app/blog/[slug]/page.tsx"]
    );
    assert!(
        blob_read_argv(&json!({ "revision": "head", "path": "app/blog/[slug]/page.tsx" })).is_ok(),
        "the diff tab must still read a bracketed route file"
    );
    // One bad element poisons the whole list — a partial argv would stage
    // the good half of a request the daemon refused.
    assert_eq!(
        changes_paths_argv(
            Verb::ChangesStage,
            &json!({ "paths": ["ok.txt", "/etc/passwd"] })
        ),
        Err(ArgvError::BadParam("paths"))
    );
    for empty in [
        json!({}),
        json!({ "paths": [] }),
        json!({ "paths": "a.txt" }),
    ] {
        assert_eq!(
            changes_paths_argv(Verb::ChangesStage, &empty),
            Err(ArgvError::BadParam("paths")),
            "{empty} must yield NO argv"
        );
    }
    // A single over-long path is refused too — the count bound alone does
    // not bound the command line.
    assert_eq!(
        changes_paths_argv(
            Verb::ChangesStage,
            &json!({ "paths": ["x".repeat(MAX_PATH_CHARS + 1)] })
        ),
        Err(ArgvError::BadParam("paths"))
    );
    assert!(
        changes_paths_argv(
            Verb::ChangesStage,
            &json!({ "paths": ["x".repeat(MAX_PATH_CHARS)] })
        )
        .is_ok(),
        "the bound is the bound: a path of exactly MAX_PATH_CHARS is accepted"
    );
    let over: Vec<String> = (0..257).map(|i| format!("f{i}.txt")).collect();
    assert_eq!(
        changes_paths_argv(Verb::ChangesStage, &json!({ "paths": over })),
        Err(ArgvError::BadParam("paths")),
        "257 paths is over MAX_STAGE_PATHS"
    );
    let at_bound: Vec<String> = (0..MAX_STAGE_PATHS).map(|i| format!("f{i}.txt")).collect();
    assert_eq!(
        changes_paths_argv(Verb::ChangesStage, &json!({ "paths": at_bound }))
            .unwrap()
            .len(),
        MAX_STAGE_PATHS + 2,
        "the bound is the bound: 256 paths are accepted"
    );

    for bad in [
        json!({}),
        json!({ "message": "" }),
        json!({ "message": "   " }),
    ] {
        assert_eq!(
            changes_commit_argv(&bad),
            Err(ArgvError::BadParam("message")),
            "{bad} must yield NO argv"
        );
    }
    assert_eq!(
        changes_commit_argv(&json!({ "message": "x".repeat(MAX_COMMIT_MESSAGE + 1) })),
        Err(ArgvError::BadParam("message")),
        "4097 chars is over MAX_COMMIT_MESSAGE"
    );
    assert!(
        changes_commit_argv(&json!({ "message": "x".repeat(MAX_COMMIT_MESSAGE) })).is_ok(),
        "the bound is the bound: 4096 chars are accepted"
    );

    // The verb is a parameter too, and a wrong one yields no argv.
    assert_eq!(
        changes_paths_argv(Verb::ChangesCommit, &json!({ "paths": ["a.txt"] })),
        Err(ArgvError::BadParam("verb"))
    );
    assert_eq!(
        changes_paths_argv(Verb::Run, &json!({ "paths": ["a.txt"] })),
        Err(ArgvError::BadParam("verb"))
    );
    for verb in [
        Verb::ChangesStage,
        Verb::ChangesUnstage,
        Verb::ChangesCommit,
        Verb::ChangesDiscard,
    ] {
        assert_eq!(
            spawn_argv(verb, &json!({})),
            Err(ArgvError::BadParam("verb")),
            "a Mutate verb never reaches the spawn path"
        );
    }

    // The SAME bad vector is refused by the read-side builder too — the one
    // shared gate, exercised through both callers.
    assert_eq!(
        blob_read_argv(&json!({ "revision": "head", "path": ":(glob)x" })),
        Err(ArgvError::BadParam("path"))
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
fn blob_read_argv_composes_exact_vector_and_refuses() {
    assert_eq!(Verb::from_query("blob.read"), Some(Verb::BlobRead));
    assert_eq!(Verb::BlobRead.effect_class(), EffectClass::Query);

    let expected: Vec<String> = [
        "blob",
        "read",
        "--revision",
        "head",
        "--path",
        "src/main.rs",
        "--format",
        "json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    assert_eq!(
        blob_read_argv(&json!({ "revision": "head", "path": "src/main.rs" })).unwrap(),
        expected
    );
    // A Windows-shaped path from the browser composes the SAME vector: the
    // separator is normalised, not a second accepted spelling.
    assert_eq!(
        blob_read_argv(&json!({ "revision": "head", "path": "src\\main.rs" })).unwrap(),
        expected
    );

    for bad in [
        "/etc/passwd",
        "C:\\Windows\\x",
        "../../secret",
        "-rf",
        "",
        "src/../../secret",
    ] {
        assert_eq!(
            blob_read_argv(&json!({ "revision": "head", "path": bad })),
            Err(ArgvError::BadParam("path")),
            "{bad:?} must yield NO argv"
        );
    }
    assert_eq!(
        blob_read_argv(&json!({ "revision": "head" })),
        Err(ArgvError::BadParam("path")),
        "a missing path yields no argv"
    );

    // The revision is a closed enum: HEAD is the only side this slice diffs against.
    assert_eq!(
        blob_read_argv(&json!({ "revision": "work", "path": "a" })),
        Err(ArgvError::BadParam("revision"))
    );
    assert_eq!(
        blob_read_argv(&json!({ "path": "a" })),
        Err(ArgvError::BadParam("revision"))
    );
}

#[test]
fn branch_list_argv_is_static() {
    assert_eq!(
        branch_list_argv(),
        vec!["branch", "list", "--format", "json"],
        "the branch-list verb takes no client input"
    );
}

#[test]
fn worktree_list_argv_is_static() {
    assert_eq!(
        worktree_list_argv(),
        vec!["worktree", "list", "--format", "json"],
        "the worktree-list verb takes no client input"
    );
    assert_eq!(Verb::from_query("worktree.list"), Some(Verb::WorktreeList));
    assert_eq!(Verb::WorktreeList.effect_class(), EffectClass::Query);
}

#[test]
fn changes_list_argv_is_static() {
    assert_eq!(
        changes_list_argv(),
        vec!["changes", "list", "--format", "json"],
        "the changes-list verb takes no client input"
    );
    assert_eq!(Verb::from_query("changes.list"), Some(Verb::ChangesList));
    assert_eq!(Verb::ChangesList.effect_class(), EffectClass::Query);
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
fn config_argv_refuses_exec_adjacent_keys() {
    // `verify.command` is well-shaped and a genuinely supported CLI key, so
    // neither the character class nor the CLI's `require_known_key` refuses
    // it. Its value becomes argv[0] of the verify gate's child, so the daemon
    // refuses the key itself — remotely, only.
    assert_eq!(
        config_argv(
            Verb::ConfigSet,
            &serde_json::json!({ "key": "verify.command", "value": "C:/evil.exe" })
        ),
        Err(ArgvError::BadParam("key"))
    );
    assert_eq!(
        config_argv(
            Verb::ConfigUnset,
            &serde_json::json!({ "key": "verify.command" })
        ),
        Err(ArgvError::BadParam("key"))
    );
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

#[test]
fn board_argv_is_static() {
    assert_eq!(
        board_argv(),
        vec!["issues", "--format", "json", "--board"],
        "the board verb takes no client input"
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

#[test]
fn spawn_argv_carries_copilot_through_to_the_agent_flag() {
    // Copilot's adapter and CLI variant landed in #229; the daemon's own enum is
    // hand-kept in step with them (ADR-0040 Tier 4, issue #238). The flag value
    // must be the CLI's own `--agent copilot`.
    assert_eq!(
        spawn_argv(
            Verb::Run,
            &serde_json::json!({ "agent": "copilot", "branchMode": "new" })
        )
        .unwrap(),
        vec![
            "run",
            "--if-idle",
            "--agent",
            "copilot",
            "--branch-mode",
            "new"
        ]
    );
}

#[test]
fn spawn_argv_carries_kimi_through_to_the_agent_flag() {
    // Kimi was absent from the daemon's enum while its adapter shipped, so a
    // workbench run refused with BadParam("agent") (issue #228). The flag value
    // must be the CLI's own `--agent kimi`.
    assert_eq!(
        spawn_argv(
            Verb::Run,
            &serde_json::json!({ "agent": "kimi", "branchMode": "new" })
        )
        .unwrap(),
        vec![
            "run",
            "--if-idle",
            "--agent",
            "kimi",
            "--branch-mode",
            "new"
        ]
    );
}

#[test]
fn spawn_argv_carries_cursor_through_to_the_agent_flag() {
    // ADR-0042 D1 deferred the daemon on purpose; #248 lifts it. The flag value
    // is the CLI's `--agent cursor`, NOT the binary name `cursor-agent`.
    assert_eq!(
        spawn_argv(
            Verb::Run,
            &serde_json::json!({ "agent": "cursor", "branchMode": "new" })
        )
        .unwrap(),
        vec![
            "run",
            "--if-idle",
            "--agent",
            "cursor",
            "--branch-mode",
            "new"
        ]
    );
}

#[test]
fn spawn_argv_carries_gemini_through_to_the_agent_flag() {
    assert_eq!(
        spawn_argv(
            Verb::Run,
            &serde_json::json!({ "agent": "gemini", "branchMode": "new" })
        )
        .unwrap(),
        vec![
            "run",
            "--if-idle",
            "--agent",
            "gemini",
            "--branch-mode",
            "new"
        ]
    );
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
