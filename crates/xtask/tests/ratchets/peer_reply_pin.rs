//! The pin that ties the peer reply types to the peer protocol version (#653).

use super::*;

/// `(module under UI_DIR, type)` of each page type that reads a reply a peer
/// daemon wrote. The local daemon passes these replies on as the peer wrote
/// them: the one-reply verbs of a peer repo and the registry verbs that name
/// a peer (`relay_to_peer` in `routes/ws_command.rs`), the frames of a spawn
/// verb, the frames of a console channel (the `/ws/session` frames), and
/// `GET /api/agents?repo=`. The page
/// reads only the status of `POST /api/sessions/close`, so no type reads its
/// body. `/api/sessions`, `/api/fleet`, `/api/usage` and the tree pushes are
/// written again by the local daemon, so they are not here. A type that a
/// listed type names is listed too, or is in `NOT_PEER_REPLIES`.
const PEER_REPLY_TYPES: &[(&str, &str)] = &[
    ("globals.d.ts", "DaemonReplies"),
    ("globals.d.ts", "DaemonReply"),
    ("globals.d.ts", "FileReadOk"),
    ("globals.d.ts", "FileReadReply"),
    ("globals.d.ts", "JsonValue"),
    ("globals.d.ts", "ReplyOf"),
    ("globals.d.ts", "SpawnStatus"),
    ("globals.d.ts", "TreeEntry"),
    ("globals.d.ts", "WriteReply"),
    ("wb-api.ts", "AgentRow"),
    ("wb-changes.ts", "ChangeRow"),
    ("wb-changes.ts", "SyncBody"),
    ("wb-console-frames.ts", "FramePayload"),
    ("wb-console-frames.ts", "SessionFrame"),
    ("wb-kanban.ts", "BoardLabel"),
    ("wb-kanban.ts", "BoardRow"),
    ("wb-kanban.ts", "IssueComment"),
    ("wb-kanban.ts", "IssueDetail"),
    ("wb-project.ts", "Listing"),
    ("wb-project.ts", "Worktree"),
    ("wb-runs.ts", "RunSleep"),
    ("wb-runs.ts", "RunSnapshot"),
    // `FramePayload` reads it through the type of `announcement`'s parameter.
    ("wb-session-route.ts", "SessionOpen"),
];

/// `(module, type, why)` of each type a listed type names that no peer writes.
const NOT_PEER_REPLIES: &[(&str, &str, &str)] = &[
    (
        "wb-hosts.ts",
        "Alias",
        "a host verb is never relayed to a peer",
    ),
    (
        "wb-hosts.ts",
        "KeyReply",
        "a host verb is never relayed to a peer",
    ),
];

/// `(PEER_PROTOCOL_VERSION, hash of PEER_REPLY_TYPES)`. Version 4 pays for
/// the fields that version 3 got with no raise: `file.read`'s `encoding` and
/// `bom`, and the `char_index` of a refused write (#653). The optional
/// `rows` and `cols` of `session-size` keep version 4: with no such frame a
/// watcher fits its own box, as before (#689). So does the optional
/// `replay_encoding` of `session-open`: a peer that does not send it sends
/// the replay raw, and the page reads it raw. So do the optional `replay`,
/// `offset` and `resume` of `session-open`: with no `offset` the page never
/// sends `since`, and with no `resume` it resets its terminal, as before.
/// The new verbs `file.stat` and `tree.size` keep version 4 (ADR-0052): every
/// field of their replies is optional in the page type, and an older peer
/// answers them `unknown verb`, which the Properties dialog shows as a refusal
/// that names the older daemon. The optional `phase.agent` of a run snapshot
/// keeps version 4: a run of an older peer sends none, and the run card then
/// draws no agent dot, as for a vendor with no hooks. Version 5 changes
/// no reply type: it changes the transport of the relayed consoles and spawn
/// verbs, which ride a channel of the peer's page socket (#830).
const PEER_REPLY_PIN: (u32, u64) = (5, 0x1da2_853b_91d2_a9f8);

/// Where `PEER_PROTOCOL_VERSION` is, from the repo root.
const PEER_RS: &str = "crates/ralphy-daemon/src/peer.rs";

/// #653: a peer of the same protocol version sends the same reply shapes,
/// because a change to a type that reads one fails here until the change
/// shows one of two choices: a raise of `PEER_PROTOCOL_VERSION` (the default),
/// or new fields declared optional with the same version.
#[test]
fn peer_reply_types_are_pinned_to_the_protocol_version() {
    let root = workspace_root();
    let ui = root.join(UI_DIR);
    let mut sources = BTreeMap::new();
    for path in ts_modules(&ui) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        sources.insert(name, read(&path));
    }
    let (hash, mut errors) = peer_reply_hash(&sources, PEER_REPLY_TYPES, NOT_PEER_REPLIES);
    let peer_rs = read(&root.join(PEER_RS));
    let version = Regex::new(r"pub const PEER_PROTOCOL_VERSION: u32 = (\d+);")
        .expect("a valid regex")
        .captures(&peer_rs)
        .and_then(|c| c[1].parse::<u32>().ok());
    match version {
        None => errors.push(format!(
            "{PEER_RS} declares no `pub const PEER_PROTOCOL_VERSION: u32`"
        )),
        Some(version) if errors.is_empty() => {
            errors.extend(peer_pin_error(version, hash, PEER_REPLY_PIN));
        }
        Some(_) => {}
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn peer_reply_hash_skips_layout_and_sees_a_field() {
    let pin = |src: &str| {
        let sources = BTreeMap::from([("a.ts".to_string(), src.to_string())]);
        peer_reply_hash(&sources, &[("a.ts", "R")], &[])
    };
    let (base, errors) = pin("export type R = { a: string; b?: number; };\n");
    assert_eq!(errors, Vec::<String>::new());
    let (laid_out, _) =
        pin("/** R */\nexport type R = {\n  a: string; // the a\n  b?: number;\n};\n");
    assert_eq!(laid_out, base);
    let (changed, _) = pin("export type R = { a: string; b: number; };\n");
    assert_ne!(changed, base);

    let (_, errors) = pin("type S = { a: string };\n");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("a.ts declares no type R"),
        "{errors:?}"
    );
    let (_, errors) = pin("type R = { s: S };\ntype S = { a: string };\n");
    assert!(
        errors.len() == 1 && errors[0].starts_with("a.ts R names a.ts S"),
        "{errors:?}"
    );

    assert_eq!(peer_pin_error(4, 7, (4, 7)), None);
    for (version, hash) in [(4, 8), (5, 7), (5, 8)] {
        assert!(
            peer_pin_error(version, hash, (4, 7)).is_some(),
            "{version} {hash}"
        );
    }
}

/// The FNV-1a hash of the listed declarations, each as its tokens with no
/// comment and one space between tokens, and the errors: a listed type that
/// is not declared, and a type a listed type names that is not listed.
/// FNV-1a is written here because std's `DefaultHasher` may change between
/// Rust releases, and the pin must not.
fn peer_reply_hash(
    sources: &BTreeMap<String, String>,
    listed: &[(&str, &str)],
    excluded: &[(&str, &str, &str)],
) -> (u64, Vec<String>) {
    let lexed: BTreeMap<&str, (Vec<char>, Vec<lex::Token>)> = sources
        .iter()
        .map(|(name, src)| (name.as_str(), (src.chars().collect(), lex::lex(src, 1))))
        .collect();
    let known: BTreeSet<(&str, &str)> = listed
        .iter()
        .copied()
        .chain(excluded.iter().map(|(file, name, _)| (*file, *name)))
        .collect();
    let mut errors = Vec::new();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut sorted = listed.to_vec();
    sorted.sort_unstable();
    for (file, name) in sorted {
        let Some((cs, toks)) = lexed.get(file) else {
            errors.push(format!("{file} declares no type {name}: there is no {file} in {UI_DIR}; update PEER_REPLY_TYPES"));
            continue;
        };
        let Some(range) = type_declaration(toks, name) else {
            errors.push(format!(
                "{file} declares no type {name}: a type in PEER_REPLY_TYPES was renamed or moved; update the list"
            ));
            continue;
        };
        let decl = &toks[range];
        let text: Vec<String> = decl
            .iter()
            .map(|t| cs[t.start..t.end].iter().collect())
            .collect();
        for byte in format!("{file} {}\n", text.join(" ")).bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        for (ref_file, ref_name) in named_types(decl, file, toks, &lexed) {
            if ref_name != name && !known.contains(&(ref_file.as_str(), ref_name.as_str())) {
                errors.push(format!(
                    "{file} {name} names {ref_file} {ref_name}, which is not in PEER_REPLY_TYPES: \
                     add it, or add it to NOT_PEER_REPLIES with why no peer writes it"
                ));
            }
        }
    }
    (hash, errors)
}

/// `(module, type)` of each type that `decl` names and that a module declares:
/// `import("./m.ts").T`, a type of the same module, a type the module imports
/// with `import type { … }`, or a type of `globals.d.ts`.
fn named_types(
    decl: &[lex::Token],
    file: &str,
    toks: &[lex::Token],
    lexed: &BTreeMap<&str, (Vec<char>, Vec<lex::Token>)>,
) -> BTreeSet<(String, String)> {
    let declares = |module: &str, name: &str| {
        lexed
            .get(module)
            .is_some_and(|(_, t)| type_declaration(t, name).is_some())
    };
    let imports = type_imports(toks);
    let mut found = BTreeSet::new();
    for (k, t) in decl.iter().enumerate() {
        let Some(name) = t.ident() else { continue };
        if k > 0 && decl[k - 1].is(".") {
            continue;
        }
        if name == "import" && decl.get(k + 1).is_some_and(|t| t.is("(")) {
            if let (Some(lex::Tok::Str(path)), Some(dot), Some(member)) = (
                decl.get(k + 2).map(|t| &t.tok),
                decl.get(k + 4),
                decl.get(k + 5).and_then(|t| t.ident()),
            ) {
                if dot.is(".") {
                    let module = path.trim_start_matches("./");
                    found.insert((module.to_string(), member.to_string()));
                }
            }
            continue;
        }
        if declares(file, name) {
            found.insert((file.to_string(), name.to_string()));
        } else if let Some((module, original)) = imports.get(name) {
            found.insert((module.clone(), original.clone()));
        } else if declares("globals.d.ts", name) {
            found.insert(("globals.d.ts".to_string(), name.to_string()));
        }
    }
    found
}

/// `local name -> (module, exported name)` of each `import type { … } from "./m.ts"`.
fn type_imports(toks: &[lex::Token]) -> BTreeMap<String, (String, String)> {
    let mut out = BTreeMap::new();
    for k in 0..toks.len() {
        if toks[k].ident() != Some("import")
            || toks.get(k + 1).and_then(|t| t.ident()) != Some("type")
            || !toks.get(k + 2).is_some_and(|t| t.is("{"))
        {
            continue;
        }
        let Some(close) = (k + 3..toks.len()).find(|&j| toks[j].is("}")) else {
            continue;
        };
        let Some(lex::Tok::Str(path)) = toks.get(close + 2).map(|t| &t.tok) else {
            continue;
        };
        let module = path.trim_start_matches("./");
        let names: Vec<&str> = toks[k + 3..close]
            .iter()
            .filter_map(|t| t.ident())
            .collect();
        let mut i = 0;
        while i < names.len() {
            let (local, original) = if names.get(i + 1) == Some(&"as") {
                (names.get(i + 2).copied().unwrap_or(names[i]), names[i])
            } else {
                (names[i], names[i])
            };
            out.insert(
                local.to_string(),
                (module.to_string(), original.to_string()),
            );
            i += if names.get(i + 1) == Some(&"as") {
                3
            } else {
                1
            };
        }
    }
    out
}

/// The failure, if any, of `version` and `hash` against `pin`.
fn peer_pin_error(version: u32, hash: u64, pin: (u32, u64)) -> Option<String> {
    let (pinned_version, pinned_hash) = pin;
    if (version, hash) == pin {
        return None;
    }
    if version == pinned_version {
        return Some(format!(
            "The shape of a reply that a peer daemon can answer changed: the types in \
             PEER_REPLY_TYPES hash to {hash:#018x}, and PEER_REPLY_PIN has {pinned_hash:#018x} \
             for peer protocol {version}. A peer of the same protocol version must send the \
             same shapes. Choose one:\n\
             - the default: raise PEER_PROTOCOL_VERSION in {PEER_RS} to {next}, and set \
             PEER_REPLY_PIN to ({next}, {hash:#018x});\n\
             - the exception, only when each change is a new field: declare each new field \
             optional in the page type, set PEER_REPLY_PIN to ({version}, {hash:#018x}), and \
             say in the commit why the page is correct when an older peer does not send it.",
            next = version + 1
        ));
    }
    if hash == pinned_hash {
        return Some(format!(
            "PEER_PROTOCOL_VERSION is {version}, and PEER_REPLY_PIN names version \
             {pinned_version}. The reply types did not change. Set PEER_REPLY_PIN to \
             ({version}, {hash:#018x}), so the pin names the version these types belong to."
        ));
    }
    Some(format!(
        "PEER_PROTOCOL_VERSION is {version} and the reply types changed: set PEER_REPLY_PIN \
         to ({version}, {hash:#018x})."
    ))
}
