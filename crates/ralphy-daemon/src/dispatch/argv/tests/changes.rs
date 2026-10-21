use super::*;

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
