use super::*;

#[test]
fn verb_effect_classes() {
    assert_eq!(Verb::TreeList.effect_class(), EffectClass::Observe);
    assert_eq!(Verb::TreeFind.effect_class(), EffectClass::Observe);
    assert_eq!(Verb::TreeGrep.effect_class(), EffectClass::Observe);
    assert_eq!(Verb::FileRead.effect_class(), EffectClass::Observe);
    assert_eq!(Verb::FileStat.effect_class(), EffectClass::Observe);
    assert_eq!(Verb::TreeSize.effect_class(), EffectClass::Observe);
    assert_eq!(Verb::ImageRead.effect_class(), EffectClass::Observe);
    assert_eq!(Verb::from_query("file.image"), Some(Verb::ImageRead));
    assert_eq!(Verb::RunsList.effect_class(), EffectClass::Observe);
    assert_eq!(Verb::from_query("runs.list"), Some(Verb::RunsList));
    assert_eq!(Verb::Run.effect_class(), EffectClass::Spawn);
    assert_eq!(Verb::Triage.effect_class(), EffectClass::Spawn);
    assert_eq!(Verb::PushQueue.effect_class(), EffectClass::Spawn);
    assert_eq!(Verb::ConfigGet.effect_class(), EffectClass::Query);
    assert_eq!(Verb::BoardList.effect_class(), EffectClass::Query);
    assert_eq!(Verb::IssueShow.effect_class(), EffectClass::Query);
    assert_eq!(Verb::ConfigSet.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::ConfigUnset.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::FileWrite.effect_class(), EffectClass::Write);
    assert_eq!(Verb::FileCreate.effect_class(), EffectClass::Write);
    assert_eq!(Verb::FileRename.effect_class(), EffectClass::Write);
    assert_eq!(Verb::FileCopy.effect_class(), EffectClass::Write);
    assert_eq!(Verb::FileDelete.effect_class(), EffectClass::Write);
    assert_eq!(Verb::BranchList.effect_class(), EffectClass::Query);
    assert_eq!(Verb::BranchSwitch.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::BranchCreate.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::LabelSet.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::SyncStatus.effect_class(), EffectClass::Query);
    assert_eq!(Verb::SyncFetch.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::SyncPull.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::SyncPush.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::ChangesStage.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::ChangesUnstage.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::ChangesCommit.effect_class(), EffectClass::Mutate);
    assert_eq!(Verb::ChangesDiscard.effect_class(), EffectClass::Mutate);
    // A stop is a Mutate, never a Spawn: it runs one short `ralphy stop` and
    // collects it, exactly like `sync push` (docs/adr/0054).
    assert_eq!(Verb::RunStop.effect_class(), EffectClass::Mutate);
    // A plan discard is a Write: one in-daemon unlink of a target the VERB
    // fixes, never a spawn and never a client-named path.
    assert_eq!(Verb::from_query("plan.discard"), Some(Verb::PlanDiscard));
    assert_eq!(Verb::PlanDiscard.effect_class(), EffectClass::Write);
    // A clipboard drop is a Write: one in-daemon confined write of bytes the
    // daemon verified, to a name the daemon chose — never a spawn and never
    // a client-named path (ADR-0055).
    assert_eq!(Verb::from_query("image.write"), Some(Verb::ImageWrite));
    assert_eq!(Verb::ImageWrite.effect_class(), EffectClass::Write);
    assert_eq!(Verb::from_query("file.upload"), Some(Verb::FileUpload));
    assert_eq!(Verb::FileUpload.effect_class(), EffectClass::Write);
    // A project removal is a Mutate: it spawns the existing `daemon remove`
    // subcommand, the ONE owner of the registry file (issue #363).
    assert_eq!(
        Verb::from_query("project.remove"),
        Some(Verb::ProjectRemove)
    );
    assert_eq!(Verb::ProjectRemove.effect_class(), EffectClass::Mutate);
    // The two note verbs (ADR-0064 §5). `note.read` is an Observe read
    // like `file.image`; `note.write` is a Write whose target class the
    // VERB fixes — only a `.note` path, and the container is the daemon's
    // to build, so the client never hands over bytes.
    assert_eq!(Verb::from_query("note.read"), Some(Verb::NoteRead));
    assert_eq!(Verb::NoteRead.effect_class(), EffectClass::Observe);
    assert_eq!(Verb::from_query("note.write"), Some(Verb::NoteWrite));
    assert_eq!(Verb::NoteWrite.effect_class(), EffectClass::Write);
    // The host family (ADR-0036 amendment 2026-09-29): two reads, one
    // known_hosts write, and four streamed runs of `ralphy host`.
    for (query, verb, class) in [
        ("host.aliases", Verb::HostAliases, EffectClass::Query),
        ("host.key", Verb::HostKey, EffectClass::Query),
        ("host.trust", Verb::HostTrust, EffectClass::Mutate),
        ("host.check", Verb::HostCheck, EffectClass::Spawn),
        ("host.add", Verb::HostAdd, EffectClass::Spawn),
        ("host.install", Verb::HostInstall, EffectClass::Spawn),
        ("host.remove", Verb::HostRemove, EffectClass::Spawn),
    ] {
        assert_eq!(Verb::from_query(query), Some(verb));
        assert_eq!(verb.effect_class(), class, "{query}");
        assert!(verb.is_host(), "{query}");
    }
    assert_eq!(
        Verb::ALL.iter().filter(|v| v.is_host()).count(),
        7,
        "the host family is seven verbs"
    );
    // The registry family (ADR-0036 amendment "the registry verbs"): a
    // one-level folder read and a spawn of `daemon add`, the one owner of
    // the registry file. Neither is a host verb: both are relayed to a peer.
    for (query, verb, class) in [
        ("dir.list", Verb::DirList, EffectClass::Observe),
        ("project.add", Verb::ProjectAdd, EffectClass::Mutate),
    ] {
        assert_eq!(Verb::from_query(query), Some(verb));
        assert_eq!(verb.effect_class(), class, "{query}");
        assert!(!verb.is_host(), "{query}");
    }
    assert_eq!(
        Verb::ALL.len(),
        54,
        "the registry holds exactly fifty-four verbs"
    );
}

#[test]
fn from_query_maps_config_verbs() {
    assert_eq!(Verb::from_query("config.get"), Some(Verb::ConfigGet));
    assert_eq!(Verb::from_query("board.list"), Some(Verb::BoardList));
    assert_eq!(Verb::from_query("issue.show"), Some(Verb::IssueShow));
    assert_eq!(Verb::from_query("config.set"), Some(Verb::ConfigSet));
    assert_eq!(Verb::from_query("config.unset"), Some(Verb::ConfigUnset));
    assert_eq!(Verb::from_query("file.write"), Some(Verb::FileWrite));
    assert_eq!(Verb::from_query("file.create"), Some(Verb::FileCreate));
    assert_eq!(Verb::from_query("file.rename"), Some(Verb::FileRename));
    assert_eq!(Verb::from_query("file.copy"), Some(Verb::FileCopy));
    assert_eq!(Verb::from_query("file.delete"), Some(Verb::FileDelete));
    assert_eq!(Verb::from_query("branch.list"), Some(Verb::BranchList));
    assert_eq!(Verb::from_query("branch.switch"), Some(Verb::BranchSwitch));
    assert_eq!(Verb::from_query("branch.create"), Some(Verb::BranchCreate));
    assert_eq!(Verb::from_query("label.set"), Some(Verb::LabelSet));
    assert_eq!(Verb::from_query("blob.read"), Some(Verb::BlobRead));
}

/// The ADR-0040 canary: `from_query` (what the workbench sends IN) and
/// `agent_flag` (what the CLI receives OUT) are hand-maintained in two places,
/// so a vendor added to one and not the other silently refuses a launch.
#[test]
fn agent_flag_round_trips_through_from_query() {
    for a in Agent::ALL {
        assert_eq!(
            Agent::from_query(agent_flag(a)),
            Some(a),
            "{a:?}'s CLI flag does not parse back through from_query"
        );
    }
}

/// The workbench's vendor list used to be hand-maintained in THREE places in
/// `app.ts` and nothing compiled it — Kimi shipped missing from all three
/// (issue #228). Issue #304 removed the list: the menu renders from
/// `GET /api/agents`, whose digits an exhaustive match owns (`roster.rs`).
/// So the pin INVERTED — `app.ts` must hold NO vendor enumeration, and the
/// guarantee that every adapter reaches the menu lives in `roster.rs`.
#[test]
fn app_js_holds_no_vendor_list() {
    let js = include_str!("../../assets/ui/app.ts");
    // The roster and its read live in the Alpine store `projects` (#621).
    let store = include_str!("../../assets/ui/wb-projects-store.ts");
    let api_js = include_str!("../../assets/ui/wb-api.ts");

    // Non-vacuous first: the store reads the roster route, which the route
    // list of `wb-api.ts` declares.
    assert!(
        store.contains("apiFetch(\"GET /api/agents\"") && api_js.contains("\"GET /api/agents\": "),
        "the store `projects` must render the menu from the daemon's roster endpoint"
    );

    // The `agents` binding survives (the run dialog's pickers bind it) but it
    // is now filled from the roster, so its literal must be EMPTY. Anchored on
    // the leading space so `planAgents: [` cannot satisfy it, and required to
    // be UNIQUE — a second, populated `agents: [ … ]` further down would
    // otherwise leave this first (empty) one answering for it.
    assert_eq!(
        store.matches(" agents: [").count(),
        1,
        "the store `projects` must declare exactly one `agents: [` literal"
    );
    let (_, rest) = store
        .split_once(" agents: [")
        .expect("the store `projects` no longer declares `agents: [`");
    let (agents, _) = rest
        .split_once(']')
        .expect("the store's `agents: [` is never closed");
    assert!(
        agents.trim().is_empty(),
        "the store's `agents:` must stay an empty literal; found: {agents:?}"
    );

    // The accelerator map keyed by vendor is gone; digits come from the rows.
    assert!(
        !js.contains("const map = { Digit1"),
        "app.ts still maps accelerator digits to vendors"
    );

    // No launchable vendor is named in `app.ts` — except `claude`, the run
    // dialog's default value (a default naming one vendor is not an
    // enumeration; it is the CLI's own default). Checked
    // QUOTE-AGNOSTICALLY: `app.ts` is full of template literals and single
    // quotes, so pinning only `"codex"` would wave `'codex'` and `` `codex` ``
    // straight through — the reintroduced list would look exactly like that.
    let named_non_default_vendors = |source: &str| {
        Agent::ALL
            .into_iter()
            .map(agent_flag)
            .filter(|flag| *flag != "claude")
            .filter(|flag| {
                ['"', '\'', '`']
                    .into_iter()
                    .any(|quote| source.contains(&format!("{quote}{flag}{quote}")))
            })
            .collect::<Vec<_>>()
    };
    assert!(
        named_non_default_vendors(js).is_empty(),
        "app.ts must hold no non-default vendor names"
    );
    assert!(
        named_non_default_vendors(store).is_empty(),
        "the store `projects` must hold no non-default vendor names"
    );
    assert_eq!(
        named_non_default_vendors(r#"agents: [{ id: "codex" }]"#),
        ["codex"],
        "the vendor-list guard must reject a populated production roster"
    );
    assert!(
        !js.contains(r#"kind: "claude""#) && !js.contains(r#"Digit1: "claude""#),
        "app.ts still routes a console launch through a hardcoded vendor"
    );
}

#[test]
fn from_query_accepts_only_the_blessed_verbs() {
    assert_eq!(Verb::from_query("run"), Some(Verb::Run));
    assert_eq!(Verb::from_query("triage"), Some(Verb::Triage));
    assert_eq!(Verb::from_query("push"), Some(Verb::PushQueue));
    assert_eq!(Verb::from_query("tree.list"), Some(Verb::TreeList));
    assert_eq!(Verb::from_query("tree.find"), Some(Verb::TreeFind));
    assert_eq!(Verb::from_query("tree.grep"), Some(Verb::TreeGrep));
    assert_eq!(Verb::from_query("file.read"), Some(Verb::FileRead));
    assert_eq!(Verb::from_query("file.stat"), Some(Verb::FileStat));
    assert_eq!(Verb::from_query("tree.size"), Some(Verb::TreeSize));
    // No destructive verb, and no arbitrary composition, is reachable.
    // `stop` and `kill` stay unrepresentable even though `run.stop` now
    // exists (docs/adr/0054): that verb spawns a `ralphy stop` which WRITES A
    // REQUEST, and nothing on this surface ever signals a process. Do not
    // relax this list to accommodate it.
    // `plan.discard` is the whole plan capability: no `plan.write`, no
    // `plan.read` (the panel reads the plan through `file.read` like any other
    // file), and no path parameter anywhere near it.
    // `image.write` is likewise the whole clipboard-drop capability: the
    // read side is `file.image`, and there is no generic `image.*` family.
    // The note family is two verbs and stays two: a note is renamed and
    // deleted with `file.rename`/`file.delete`, which reach it through the
    // denylist's one carve-out (ADR-0064 §5), not through a `note.*` verb
    // that would have to re-derive the same confinement.
    for rejected in [
        "note",
        "note.delete",
        "note.rename",
        "note.list",
        // The properties are two verbs and stay two: no `file.size`, no
        // `tree.stat`, no `file.*` or `tree.*` family to widen them.
        "file.size",
        "tree.stat",
        "image.read",
        "image",
        "image.delete",
        "kill",
        "stop",
        "issues",
        "run --if-idle",
        "",
        "Run",
        "PUSH",
        "run.kill",
        "plan.write",
        "plan.delete",
        "plan",
        "host",
        "host.list",
        "host.run",
        "host.exec",
        "dir",
        "dir.read",
        "dir.create",
        "project.create",
    ] {
        assert_eq!(
            Verb::from_query(rejected),
            None,
            "{rejected:?} must not parse to a verb"
        );
    }
}

/// The cwd-taking set is exactly the 13 git-backed verbs (ADR-0063 §2 plus
/// `sync.*`): every Observe/Write/Spawn verb and the `.ralphy`-owning
/// Query/Mutate verbs keep the registry path whatever `checkout` says.
#[test]
fn takes_checkout_cwd_is_exactly_the_git_backed_family() {
    let expected = [
        Verb::BranchList,
        Verb::BranchSwitch,
        Verb::BranchCreate,
        Verb::ChangesList,
        Verb::ChangesStage,
        Verb::ChangesUnstage,
        Verb::ChangesCommit,
        Verb::ChangesDiscard,
        Verb::BlobRead,
        Verb::SyncStatus,
        Verb::SyncFetch,
        Verb::SyncPull,
        Verb::SyncPush,
    ];
    let mut count = 0;
    for &v in Verb::ALL {
        assert_eq!(
            v.takes_checkout_cwd(),
            expected.contains(&v),
            "{v:?} membership in the git-backed family"
        );
        if v.takes_checkout_cwd() {
            count += 1;
        }
    }
    assert_eq!(count, 13, "the family is the 13 git-backed verbs");
    assert_eq!(Verb::ALL.len(), 54, "Verb::ALL grew — revisit the family");

    for &v in Verb::ALL {
        if matches!(
            v.effect_class(),
            EffectClass::Observe | EffectClass::Write | EffectClass::Spawn
        ) {
            assert!(
                !v.takes_checkout_cwd(),
                "{v:?} is not Query/Mutate, so never takes the cwd"
            );
        }
    }
    for v in [
        Verb::ConfigGet,
        Verb::ConfigSet,
        Verb::ConfigUnset,
        Verb::BoardList,
        Verb::IssueShow,
        Verb::WorktreeList,
        Verb::WorktreeAdd,
        Verb::WorktreeRemove,
        Verb::LabelSet,
        Verb::RunStop,
        Verb::ProjectRemove,
    ] {
        assert!(
            !v.takes_checkout_cwd(),
            "{v:?} owns the primary's .ralphy/ and keeps the registry path"
        );
    }
}
