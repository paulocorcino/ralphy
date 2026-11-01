//! The pure part of a tree connection: reading a client command and shaping the
//! frames pushed back. Nothing here touches a socket or a watcher, and only the
//! checkout door ([`local_checkout`]) reads the disk.

use std::path::Path;

use serde_json::{json, Value};

use crate::protocol::Command;
use crate::{checkout, watch};

/// `(operator rel, checkout name)`: how a dir held under a `checkout` is shown
/// to the browser.
pub(super) type Alias = (String, String);

/// The verbs a page sends on a tree channel. Any other verb is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verb {
    Watch,
    RunsWatch,
    HeadWatch,
    Unwatch,
    RunsUnwatch,
}

impl Verb {
    fn parse(verb: &str) -> Option<Self> {
        match verb {
            "watch" => Some(Self::Watch),
            "runs.watch" => Some(Self::RunsWatch),
            "head.watch" => Some(Self::HeadWatch),
            "unwatch" => Some(Self::Unwatch),
            "runs.unwatch" => Some(Self::RunsUnwatch),
            _ => None,
        }
    }

    pub(super) fn runs(self) -> bool {
        matches!(self, Self::RunsWatch | Self::RunsUnwatch)
    }

    /// `head.watch` holds the checkout's gitdir, not a tree dir.
    pub(super) fn head(self) -> bool {
        self == Self::HeadWatch
    }
}

/// One client command, with its `checkout` already applied.
pub(super) struct Request {
    pub(super) verb: Verb,
    pub(super) repo_ref: String,
    /// The dir the connection holds: prefixed under the checkout, or the fixed
    /// runstate dir for a `runs.*` verb.
    pub(super) rel: String,
    pub(super) alias: Option<Alias>,
    /// The checkout a `watch` or `head.watch` names, shape-gated only; `None`
    /// is the primary. A local watch passes it through [`local_checkout`].
    pub(super) checkout: Option<checkout::Checkout>,
}

enum CheckoutArg {
    Primary,
    Named(checkout::Checkout),
    Malformed,
}

fn payload_str<'a>(payload: &'a Value, key: &str) -> &'a str {
    payload.get(key).and_then(Value::as_str).unwrap_or("")
}

// Same door as `checkout::from_payload`: absent or `null` is the primary;
// anything else must pass the gate or the frame is dropped — a non-string or
// `""` never silently holds a PRIMARY watch the client believes is the
// worktree's. `runs.*` ignore it: the runstate dir is the primary's (ADR-0063 §7).
fn checkout_arg(payload: &Value, runs: bool) -> CheckoutArg {
    match payload.get("checkout") {
        None | Some(Value::Null) => CheckoutArg::Primary,
        Some(_) if runs => CheckoutArg::Primary,
        Some(Value::String(name)) => match checkout::lexical(name) {
            Some(c) => CheckoutArg::Named(c),
            None => CheckoutArg::Malformed,
        },
        Some(_) => CheckoutArg::Malformed,
    }
}

/// Read a command. `None` drops the frame: an unknown verb, or a malformed
/// `checkout` (the way an unknown route is dropped). The `checkout` is applied
/// LEXICALLY here — the shape gate only (ADR-0063 §2); a local watch resolves
/// and confines it with [`local_checkout`], and a peer does the same for its own
/// repos.
pub(super) fn parse_request(cmd: &Command) -> Option<Request> {
    let verb = Verb::parse(&cmd.verb)?;
    let repo_ref = payload_str(&cmd.payload, "repo").to_string();
    let rel = watch::norm_rel(payload_str(&cmd.payload, "path"));
    let (rel, alias, checkout) = place(rel, verb.head(), checkout_arg(&cmd.payload, verb.runs()))?;
    // The runs subscription ignores the payload path: its dir is fixed
    // (ADR-0047 §9), so a client cannot aim it elsewhere.
    let rel = if verb.runs() {
        watch::RUNSTATE_REL.to_string()
    } else {
        rel
    };
    Some(Request {
        verb,
        repo_ref,
        rel,
        alias,
        checkout,
    })
}

/// The door of a local watch under a `checkout`: the name is resolved against
/// the pointer file under the registered `root`, and its folder is confined
/// there, as for a verb (`checkout::from_payload`, `Checkout::confine`).
/// `Ok(None)` is the primary. `Err` refuses the watch: the caller holds nothing,
/// and one line is logged here, because a watch socket has no refusal frame
/// (ADR-0036 `checkout` amendment of 2026-11-01).
pub(super) fn local_checkout(
    req: &Request,
    root: &Path,
) -> Result<Option<checkout::ConfinedCheckout>, checkout::CheckoutError> {
    let Some(c) = &req.checkout else {
        return Ok(None);
    };
    let confined = checkout::resolve(c.name(), |n| checkout::is_linked(root, n))
        .map_err(|e| e.to_string())
        .and_then(|c| c.confine(root).map_err(|e| e.to_string()));
    match confined {
        Ok(confined) => Ok(Some(confined)),
        Err(why) => {
            tracing::warn!(checkout = c.name(), reason = %why, "refused a checkout watch");
            Err(checkout::CheckoutError::Unknown)
        }
    }
}

type Placed = (String, Option<Alias>, Option<checkout::Checkout>);

fn place(rel: String, head: bool, arg: CheckoutArg) -> Option<Placed> {
    match arg {
        CheckoutArg::Primary => Some((rel, None, None)),
        // `head.watch` ignores the payload path: `checkout` names WHICH gitdir.
        CheckoutArg::Named(c) if head => {
            Some((rel, Some((String::new(), c.name().to_string())), Some(c)))
        }
        CheckoutArg::Named(c) => {
            let shown = (rel.clone(), c.name().to_string());
            Some((c.prefix(&rel), Some(shown), Some(c)))
        }
        CheckoutArg::Malformed => None,
    }
}

/// The push for a settled change on a held dir. It is discriminated by REL,
/// not by the verb that subscribed, so every subscription kind shares ONE
/// `watched` list (and one exactly-once teardown). The browser's consumers react
/// differently: re-read a subtree, `runs.list`, or the checkout's branch.
pub(super) fn dirty_push(repo: &str, rel: &str, alias: Option<&Alias>) -> (&'static str, Value) {
    if rel == watch::RUNSTATE_REL {
        ("runs.dirty", json!({ "repo": repo }))
    } else if watch::is_head_rel(rel) {
        ("head.dirty", head_dirty(repo, alias))
    } else {
        ("tree.dirty", tree_dirty(repo, rel, alias))
    }
}

// The checkout's HEAD moved: the name says which checkout.
fn head_dirty(repo: &str, alias: Option<&Alias>) -> Value {
    match alias {
        Some((_, name)) => json!({ "repo": repo, "checkout": name }),
        None => json!({ "repo": repo }),
    }
}

// A checkout watch pushes the OPERATOR's rel plus the name, never the prefixed
// dir it is held under.
fn tree_dirty(repo: &str, rel: &str, alias: Option<&Alias>) -> Value {
    match alias {
        Some((shown, name)) => json!({ "repo": repo, "path": shown, "checkout": name }),
        None => json!({ "repo": repo, "path": rel }),
    }
}

/// The `tree.failed` payload: paths are shown like `tree.dirty`, and a head
/// watch has no path.
pub(super) fn failed_payload(
    repo: &str,
    rel: &str,
    alias: Option<&Alias>,
    head: bool,
    reason: &str,
) -> Value {
    match (alias, head) {
        (Some((_, name)), true) => json!({ "repo": repo, "checkout": name, "reason": reason }),
        (None, true) => json!({ "repo": repo, "reason": reason }),
        (Some((shown, name)), false) => {
            json!({ "repo": repo, "path": shown, "checkout": name, "reason": reason })
        }
        (None, false) => json!({ "repo": repo, "path": rel, "reason": reason }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(verb: &str, payload: Value) -> Command {
        Command {
            id: 0,
            verb: verb.to_string(),
            payload,
        }
    }

    fn alias(shown: &str, name: &str) -> Alias {
        (shown.to_string(), name.to_string())
    }

    #[test]
    fn a_checkout_prefixes_the_rel_and_keeps_the_operator_rel_as_alias() {
        let req = parse_request(&cmd(
            "watch",
            json!({ "repo": "r", "path": "src", "checkout": "wt-a" }),
        ))
        .expect("a well formed checkout is kept");
        assert_eq!(req.rel, ".ralphy/worktrees/wt-a/src");
        assert_eq!(req.alias, Some(alias("src", "wt-a")));
        assert_eq!(req.checkout.as_ref().map(|c| c.name()), Some("wt-a"));
    }

    #[test]
    fn a_head_watch_keeps_the_checkout_and_ignores_the_path() {
        let req = parse_request(&cmd(
            "head.watch",
            json!({ "repo": "r", "path": "src", "checkout": "wt-a" }),
        ))
        .expect("a head watch names a checkout");
        assert_eq!(req.rel, "src");
        assert_eq!(req.alias, Some(alias("", "wt-a")));
        assert_eq!(req.checkout.as_ref().map(|c| c.name()), Some("wt-a"));
    }

    #[test]
    fn a_malformed_checkout_drops_the_frame() {
        for bad in [json!(""), json!(7), json!("../x")] {
            let payload = json!({ "repo": "r", "path": "", "checkout": bad });
            assert!(parse_request(&cmd("watch", payload)).is_none(), "{bad}");
        }
        let payload = json!({ "repo": "r", "checkout": null });
        assert!(parse_request(&cmd("watch", payload)).is_some());
    }

    #[test]
    fn a_runs_verb_holds_the_runstate_dir_and_ignores_the_checkout() {
        let payload = json!({ "repo": "r", "path": "elsewhere", "checkout": "../bad" });
        let req = parse_request(&cmd("runs.watch", payload)).expect("runs ignores checkout");
        assert_eq!(req.rel, watch::RUNSTATE_REL);
        assert!(req.alias.is_none());
        assert!(req.verb.runs());
    }

    #[test]
    fn an_unknown_verb_is_dropped() {
        assert!(parse_request(&cmd("tree.list", json!({ "repo": "r" }))).is_none());
    }

    #[test]
    fn the_dirty_push_is_chosen_by_the_rel() {
        let shown = alias("src", "wt-a");
        let (verb, payload) = dirty_push("r", watch::RUNSTATE_REL, Some(&shown));
        assert_eq!((verb, payload), ("runs.dirty", json!({ "repo": "r" })));

        let (verb, payload) = dirty_push("r", ".git", Some(&shown));
        assert_eq!(
            (verb, payload),
            ("head.dirty", json!({ "repo": "r", "checkout": "wt-a" }))
        );
        let (verb, payload) = dirty_push("r", ".git", None);
        assert_eq!((verb, payload), ("head.dirty", json!({ "repo": "r" })));

        let (verb, payload) = dirty_push("r", ".ralphy/worktrees/wt-a/src", Some(&shown));
        assert_eq!(
            (verb, payload),
            (
                "tree.dirty",
                json!({ "repo": "r", "path": "src", "checkout": "wt-a" })
            )
        );
        let (verb, payload) = dirty_push("r", "src", None);
        assert_eq!(
            (verb, payload),
            ("tree.dirty", json!({ "repo": "r", "path": "src" }))
        );
    }

    #[test]
    fn the_failed_payload_has_a_shape_per_kind_of_watch() {
        let shown = alias("src", "wt-a");
        assert_eq!(
            failed_payload("r", "p", Some(&shown), true, "why"),
            json!({ "repo": "r", "checkout": "wt-a", "reason": "why" })
        );
        assert_eq!(
            failed_payload("r", "p", None, true, "why"),
            json!({ "repo": "r", "reason": "why" })
        );
        assert_eq!(
            failed_payload("r", "p", Some(&shown), false, "why"),
            json!({ "repo": "r", "path": "src", "checkout": "wt-a", "reason": "why" })
        );
        assert_eq!(
            failed_payload("r", "p", None, false, "why"),
            json!({ "repo": "r", "path": "p", "reason": "why" })
        );
    }
}
