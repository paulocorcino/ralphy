//! The non-streaming verbs of `/ws/command`: spawn-and-collect and the
//! Observe family, answered by effect class (ADR-0036 §2, ADR-0063 §2).

use std::path::{Path, PathBuf};

use anyhow::Result;

use encoding_rs::Encoding;

use crate::routes::blocking_read;
use crate::{checkout, clipboard, confine, dispatch, fswrite, protocol, session, textcodec, tree};

/// The optional `encoding` of a `file.read`/`file.write` payload (ADR-0036
/// amendment 2026-09-22): a WHATWG label, or absent. A label the decoder
/// cannot name is the ready-made refusal `unknown encoding` — never a silent
/// fall-through to detection, which would answer with bytes the client did not
/// ask for.
fn encoding_param(cmd: &protocol::Command) -> Result<Option<&'static Encoding>, serde_json::Value> {
    match cmd.payload.get("encoding").and_then(|v| v.as_str()) {
        None => Ok(None),
        Some(name) => textcodec::label(name)
            .map(Some)
            .ok_or_else(|| serde_json::json!({ "status": "error", "reason": "unknown encoding" })),
    }
}

/// Spawn-and-COLLECT a config CLI invocation (`config get|set|unset`) for a
/// Query/Mutate verb off the tokio runtime (ADR-0036 §2): unlike the streaming
/// Spawn path, a config verb yields ONE collected reply. `None` when the blocking
/// join or the spawn itself failed. Runs in `cwd` with the dispatch `daemon_id`.
pub(crate) async fn collect_config(
    argv: Vec<String>,
    cwd: PathBuf,
    daemon_id: Option<String>,
) -> Option<(Option<i32>, Vec<u8>)> {
    tokio::task::spawn_blocking(move || {
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        dispatch::collect(
            &dispatch::ProcessSpawner,
            &dispatch::ralphy_exe(),
            &argv_refs,
            &cwd,
            daemon_id.as_deref(),
        )
    })
    .await
    .ok()
    .and_then(Result::ok)
}

/// Resolve the optional `checkout` key of `cmd` against `repo_path`. `Err` is
/// the reply to send in its place: `unknown checkout` for a name that does not
/// resolve (the text the shell drops its selection on), `unavailable` when the
/// pointer-file read could not run. The read is a filesystem read like every
/// Observe sibling: off the runtime, so a cold `/mnt/c` never parks a worker.
pub(crate) async fn checkout_of(
    cmd: &protocol::Command,
    repo_path: &Path,
) -> Result<Option<checkout::Checkout>, serde_json::Value> {
    let (payload, root) = (cmd.payload.clone(), repo_path.to_path_buf());
    match blocking_read(move || checkout::from_payload(&payload, &root)).await {
        Some(Ok(c)) => Ok(c),
        Some(Err(e)) => Err(serde_json::json!({ "status": "error", "message": e.to_string() })),
        None => Err(serde_json::json!({ "status": "error", "reason": "unavailable" })),
    }
}

/// The `current_dir` of a spawn-and-collect verb: the selected worktree for the
/// git-backed family (`Verb::takes_checkout_cwd`), the registry path for every
/// other verb — which never reads the key, so it never answers `unknown
/// checkout` either. Resolved AFTER the argv composed (a malformed param keeps
/// its "invalid … options" reply) and BEFORE any spawn. The worktree dir is
/// confined against the registered root exactly as the searches confine their
/// walk root: a `.ralphy/worktrees/<name>` that is a symlink out of the repo
/// is `unknown checkout` here too, never a cwd for `changes discard`.
pub(crate) async fn spawn_cwd(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
) -> Result<PathBuf, serde_json::Value> {
    if !verb.takes_checkout_cwd() {
        return Ok(repo_path.to_path_buf());
    }
    let Some(c) = checkout_of(cmd, repo_path).await? else {
        return Ok(repo_path.to_path_buf());
    };
    let (root, rel) = (repo_path.to_path_buf(), c.prefix(""));
    match blocking_read(move || confine::confine(&root, &rel)).await {
        Some(Ok(_)) => Ok(c.dir(repo_path)),
        Some(Err(_)) => Err(serde_json::json!({ "status": "error", "message": checkout::UNKNOWN })),
        None => Err(serde_json::json!({ "status": "error", "reason": "unavailable" })),
    }
}

/// Answer one non-streaming verb by effect class. The optional `checkout`
/// argument (ADR-0036 `checkout` amendment, ADR-0063 §2) is read HERE, the one
/// place every Observe verb takes its `path`: it becomes a rel PREFIX under the
/// same registered root (`checkout::from_payload`), so confinement never learns
/// a second root, and every reply stays in the operator's coordinates — a
/// listing carries names, a search hit is relative to its walk root, a read
/// carries no path. `runs.list` ignores it (runs are primary-tree state); a
/// Write verb refuses it until the `.ralphy` denylist is lifted for worktrees.
/// The git-backed verbs (`Verb::takes_checkout_cwd`) run their composed
/// command with the worktree as `current_dir` (ADR-0063 §2) — the argv is
/// unchanged; a worktree act is neither held by nor holds the primary's run
/// lock (the worktree has no `.ralphy/`; the lock is the primary tree's).
/// `worktree.remove` is gated HERE first (ADR-0063 §2): while a live session
/// of `slug` was spawned in that worktree, the reply is `has a live console`
/// and nothing is composed or spawned — the session table is the daemon's
/// alone, so the CLI cannot apply this gate.
pub(crate) async fn execute_oneshot(
    verb: dispatch::Verb,
    cmd: &protocol::Command,
    repo_path: &Path,
    daemon_id: Option<&str>,
    slug: &str,
    sessions: &session::SessionManager,
) -> Option<serde_json::Value> {
    match verb.effect_class() {
        dispatch::EffectClass::Observe => {
            let checkout = match checkout_of(cmd, repo_path).await {
                Ok(c) => c,
                Err(reply) => return Some(reply),
            };
            let rel = cmd
                .payload
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let rel = checkout
                .as_ref()
                .map_or_else(|| rel.to_string(), |c| c.prefix(rel));
            let rel = rel.as_str();
            Some(match verb {
                dispatch::Verb::TreeList => {
                    let (root, path) = (repo_path.to_path_buf(), rel.to_string());
                    match blocking_read(move || tree::list(&root, &path)).await {
                        Some(Ok(entries)) => {
                            serde_json::json!({ "status": "ok", "entries": entries })
                        }
                        Some(Err(_)) => {
                            serde_json::json!({ "status": "error", "reason": "not found" })
                        }
                        None => serde_json::json!({ "status": "error", "reason": "unavailable" }),
                    }
                }
                // The two searches (ADR-0036 amendment 2026-09-15) take `query`,
                // not `path`: they always walk from the root — the worktree's
                // root under a `checkout`, so hits come back relative to it
                // (`search::rel_of` strips the walk root). That walk root is
                // resolved through `confine` against the REGISTERED root first:
                // a worktree dir that is a symlink out of the repo must be
                // refused here exactly as `tree.list` refuses it, never handed
                // to the walker as a root that would confine against itself.
                // Their budget is the wire default; nothing upstream caps an
                // Observe read, so the walker stops itself and says `truncated`.
                dispatch::Verb::TreeFind | dispatch::Verb::TreeGrep => {
                    let query = cmd
                        .payload
                        .get("query")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let repo_root = repo_path.to_path_buf();
                    let settings_root = repo_path.to_path_buf();
                    let walk_rel = checkout.as_ref().map(|c| c.prefix(""));
                    let budget = tree::SearchBudget::default();
                    let find = verb == dispatch::Verb::TreeFind;
                    let searched = blocking_read(move || {
                        let root = match walk_rel {
                            None => repo_root,
                            Some(rel) => confine::confine(&repo_root, &rel)?,
                        };
                        if find {
                            tree::find(&root, &query, &budget).map(|r| serde_json::json!(r))
                        } else {
                            // The primary's settings, not the checkout's: `.ralphy`
                            // is run state a worktree does not carry.
                            let fallback = textcodec::fallback_for(&settings_root);
                            tree::grep_with(&root, &query, &budget, fallback)
                                .map(|r| serde_json::json!(r))
                        }
                    })
                    .await;
                    match searched {
                        Some(Ok(mut reply)) => {
                            reply["status"] = serde_json::json!("ok");
                            reply
                        }
                        Some(Err(_)) => {
                            serde_json::json!({ "status": "error", "reason": "not found" })
                        }
                        None => serde_json::json!({ "status": "error", "reason": "unavailable" }),
                    }
                }
                dispatch::Verb::FileRead => {
                    let hint = match encoding_param(cmd) {
                        Ok(hint) => hint,
                        Err(refusal) => return Some(refusal),
                    };
                    let (root, path) = (repo_path.to_path_buf(), rel.to_string());
                    let read = blocking_read(move || {
                        let fallback = textcodec::fallback_for(&root);
                        tree::read_with(&root, &path, hint, fallback)
                    })
                    .await;
                    match read {
                        Some(Ok(decoded)) => serde_json::json!({
                            "status": "ok",
                            "content": decoded.text,
                            "encoding": decoded.encoding.name(),
                            "bom": decoded.bom,
                        }),
                        Some(Err(e)) => {
                            serde_json::json!({ "status": "error", "reason": e.reason() })
                        }
                        None => serde_json::json!({ "status": "error", "reason": "unavailable" }),
                    }
                }
                dispatch::Verb::ImageRead => {
                    let (root, path) = (repo_path.to_path_buf(), rel.to_string());
                    match blocking_read(move || tree::read_image(&root, &path)).await {
                        Some(Ok(image)) => serde_json::json!({
                            "status": "ok",
                            "mediaType": image.media_type,
                            "base64": data_encoding::BASE64.encode(&image.bytes),
                        }),
                        Some(Err(e)) => {
                            serde_json::json!({ "status": "error", "reason": e.reason() })
                        }
                        None => serde_json::json!({ "status": "error", "reason": "unavailable" }),
                    }
                }
                dispatch::Verb::RunsList => {
                    let listing =
                        ralphy_run_snapshot::list_runs(repo_path, ralphy_proc_util::pid_is_alive);
                    serde_json::json!({
                        "status": "ok",
                        "runs": listing.live,
                        "unreadable": listing.unreadable,
                    })
                }
                _ => serde_json::json!({ "status": "error", "reason": "refused" }),
            })
        }
        dispatch::EffectClass::Write => {
            // A write under a `checkout` is refused BEFORE any arm: the
            // `.ralphy` denylist (`fswrite::PROTECTED_DIRS`) could never let a
            // prefixed target through, and silently dropping the key would
            // land a Save from a tab showing worktree bytes on the PRIMARY's
            // file at the same rel (ADR-0063 §2; lifted in a later slice).
            if cmd.payload.get("checkout").is_some_and(|v| !v.is_null()) {
                return Some(serde_json::json!({
                    "status": "error",
                    "reason": "refused",
                    "message": "writes inside a worktree are not available yet",
                }));
            }
            // A clipboard drop (ADR-0055) answers on its own: unlike its Write
            // siblings its success reply carries the PATH the daemon chose, and
            // it reads no `path` from the client at all. Validation lives here —
            // decode, cap, sniff — so `clipboard::write_image` stays a dumb
            // confined writer of bytes already known to be an image.
            if verb == dispatch::Verb::ImageWrite {
                let root = repo_path.to_path_buf();
                let outcome = match clipboard::decode_image(
                    cmd.payload.get("base64").and_then(|v| v.as_str()),
                ) {
                    Err(reason) => Some(Err(reason)),
                    Ok((kind, bytes)) => {
                        blocking_read(move || clipboard::write_image(&root, kind, &bytes))
                            .await
                            .map(|r| r.map_err(clipboard::write_reason))
                    }
                };
                return Some(match outcome {
                    Some(Ok(path)) => serde_json::json!({ "status": "ok", "path": path }),
                    Some(Err(reason)) => {
                        serde_json::json!({ "status": "error", "reason": reason })
                    }
                    None => serde_json::json!({ "status": "error", "reason": "unavailable" }),
                });
            }
            let rel = cmd
                .payload
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let result = match verb {
                dispatch::Verb::FileWrite => {
                    let content = cmd
                        .payload
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    // Absent `encoding` is UTF-8 without a BOM: every client
                    // that predates the encoding amendment keeps its bytes.
                    let encoding = match encoding_param(cmd) {
                        Ok(hint) => hint.unwrap_or(encoding_rs::UTF_8),
                        Err(refusal) => return Some(refusal),
                    };
                    let bom = cmd
                        .payload
                        .get("bom")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    fswrite::write_encoded(repo_path, rel, content, encoding, bom)
                }
                dispatch::Verb::FileCreate => {
                    let dir = cmd
                        .payload
                        .get("dir")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    fswrite::create(repo_path, rel, dir)
                }
                dispatch::Verb::FileRename => {
                    let to = cmd.payload.get("to").and_then(|v| v.as_str()).unwrap_or("");
                    fswrite::rename(repo_path, rel, to)
                }
                dispatch::Verb::FileCopy => {
                    let to = cmd.payload.get("to").and_then(|v| v.as_str()).unwrap_or("");
                    fswrite::copy(repo_path, rel, to)
                }
                dispatch::Verb::FileDelete => fswrite::delete(repo_path, rel),
                dispatch::Verb::PlanDiscard => fswrite::discard_plan(repo_path),
                _ => Err(fswrite::WriteError::Io),
            };
            Some(match result {
                Ok(()) => serde_json::json!({ "status": "ok" }),
                Err(e @ fswrite::WriteError::Unencodable { char_index }) => serde_json::json!({
                    "status": "error",
                    "reason": e.reason(),
                    "char_index": char_index,
                }),
                Err(e) => serde_json::json!({ "status": "error", "reason": e.reason() }),
            })
        }
        dispatch::EffectClass::Query => {
            let (argv_result, field): (Result<Vec<String>, dispatch::ArgvError>, &str) = match verb
            {
                dispatch::Verb::ConfigGet => (dispatch::config_argv(verb, &cmd.payload), "config"),
                dispatch::Verb::BoardList => (Ok(dispatch::board_argv()), "board"),
                dispatch::Verb::IssueShow => (dispatch::issue_show_argv(&cmd.payload), "issue"),
                dispatch::Verb::BranchList => (Ok(dispatch::branch_list_argv()), "branches"),
                dispatch::Verb::WorktreeList => (Ok(dispatch::worktree_list_argv()), "checkouts"),
                dispatch::Verb::ChangesList => (Ok(dispatch::changes_list_argv()), "changes"),
                dispatch::Verb::BlobRead => (dispatch::blob_read_argv(&cmd.payload), "blob"),
                dispatch::Verb::SyncStatus => (Ok(dispatch::sync_status_argv()), "sync"),
                _ => (Err(dispatch::ArgvError::BadParam("verb")), "config"),
            };
            Some(match argv_result {
                Err(e) => {
                    tracing::warn!(error = %e, "refused a query with invalid params");
                    serde_json::json!({ "status": "error", "message": "invalid query options" })
                }
                Ok(argv) => {
                    let cwd = match spawn_cwd(verb, cmd, repo_path).await {
                        Ok(cwd) => cwd,
                        Err(reply) => return Some(reply),
                    };
                    match collect_config(argv, cwd, daemon_id.map(str::to_owned)).await {
                        Some((Some(0), bytes)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            let parsed: serde_json::Value = serde_json::from_str(text.trim())
                                .unwrap_or_else(|_| {
                                    serde_json::Value::String(text.trim().to_string())
                                });
                            let mut obj = serde_json::Map::new();
                            obj.insert("status".to_string(), serde_json::json!("ok"));
                            obj.insert(field.to_string(), parsed);
                            serde_json::Value::Object(obj)
                        }
                        Some((_, bytes)) => serde_json::json!({
                            "status": "error",
                            "message": String::from_utf8_lossy(&bytes).trim(),
                        }),
                        None => {
                            serde_json::json!({ "status": "error", "message": "query read failed" })
                        }
                    }
                }
            })
        }
        dispatch::EffectClass::Mutate => {
            // ADR-0063 §2: refused HERE, before any argv is composed or command
            // spawned; the session table is the daemon's alone.
            if verb == dispatch::Verb::WorktreeRemove {
                if let Some(name) = cmd
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                {
                    if sessions.console_in(slug, name) {
                        return Some(serde_json::json!({
                            "status": "error",
                            "message": format!("worktree '{name}' has a live console: close it first"),
                        }));
                    }
                }
            }
            let argv_result = match verb {
                dispatch::Verb::ConfigSet | dispatch::Verb::ConfigUnset => {
                    dispatch::config_argv(verb, &cmd.payload)
                }
                dispatch::Verb::BranchSwitch | dispatch::Verb::BranchCreate => {
                    dispatch::branch_argv(verb, &cmd.payload)
                }
                dispatch::Verb::WorktreeAdd => dispatch::worktree_add_argv(&cmd.payload),
                dispatch::Verb::WorktreeRemove => dispatch::worktree_remove_argv(&cmd.payload),
                dispatch::Verb::LabelSet => dispatch::label_argv(&cmd.payload),
                dispatch::Verb::SyncFetch | dispatch::Verb::SyncPull | dispatch::Verb::SyncPush => {
                    dispatch::sync_argv(verb)
                }
                dispatch::Verb::ChangesStage
                | dispatch::Verb::ChangesUnstage
                | dispatch::Verb::ChangesDiscard => {
                    dispatch::changes_paths_argv(verb, &cmd.payload)
                }
                dispatch::Verb::ChangesCommit => dispatch::changes_commit_argv(&cmd.payload),
                dispatch::Verb::RunStop => dispatch::run_stop_argv(&cmd.payload),
                dispatch::Verb::ProjectRemove => dispatch::project_remove_argv(&cmd.payload),
                _ => Err(dispatch::ArgvError::BadParam("verb")),
            };
            Some(match argv_result {
                Err(e) => {
                    tracing::warn!(error = %e, "refused a mutation with invalid params");
                    serde_json::json!({ "status": "error", "message": "invalid mutation options" })
                }
                Ok(argv) => {
                    let cwd = match spawn_cwd(verb, cmd, repo_path).await {
                        Ok(cwd) => cwd,
                        Err(reply) => return Some(reply),
                    };
                    match collect_config(argv, cwd, daemon_id.map(str::to_owned)).await {
                        // A clean exit's output is discarded — a Mutate reply
                        // is a status, not a read — with ONE exception:
                        // `worktree.add` reports its carry-over warnings
                        // (`worktree.copy`/`worktree.share` entries it skipped)
                        // on stdout after the add succeeded, and those are the
                        // picker's notice. Relayed under `message` when there
                        // is any; the `{status:"ok"}` shape otherwise.
                        Some((Some(0), bytes)) => {
                            let msg = String::from_utf8_lossy(&bytes);
                            let msg = msg.trim();
                            if msg.is_empty() || !matches!(verb, dispatch::Verb::WorktreeAdd) {
                                serde_json::json!({ "status": "ok" })
                            } else {
                                serde_json::json!({ "status": "ok", "message": msg })
                            }
                        }
                        Some((_, bytes)) => {
                            let msg = String::from_utf8_lossy(&bytes);
                            let msg = msg.trim();
                            let msg = if msg.is_empty() { "refused" } else { msg };
                            serde_json::json!({ "status": "error", "message": msg })
                        }
                        None => serde_json::json!({
                            "status": "error",
                            "message": "mutation write failed"
                        }),
                    }
                }
            })
        }
        dispatch::EffectClass::Spawn | dispatch::EffectClass::Native => None,
    }
}
