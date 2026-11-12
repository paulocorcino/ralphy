//! The argv composer of the config verbs, and the keys a browser may not set.

use super::{ArgvError, Verb};

/// Whether `key` is a well-shaped config key (`^[a-z0-9_.]+$`): a closed
/// character class, NOT a value allowlist (that lives in the CLI's
/// `require_known_key`, ADR-0036 Decision). Argv-safety comes from no-shell
/// `Command::args()`, not a closed key set; an unknown-but-well-shaped key is
/// accepted here and rejected by `ralphy config set`, relayed as a Mutate error.
fn well_shaped_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

/// Config keys the workbench may READ but never SET — the operator's own
/// `ralphy config set` in a terminal on the host is the one way in. Each is
/// a deliberate local gesture that a browser session (a month-old cookie, a
/// leaked bearer) must not be able to make (security audit 2026-09-21, F12):
///
/// - `verify.command` becomes `argv[0]` of the verification gate's child
///   (`ralphy_core::verify`): arbitrary deferred code execution, persistent on
///   disk, fired under a run the operator started, invisible in `git status`
///   because `.ralphy/` self-ignores. Shape-checking the value cannot fix it —
///   the gate spawns with `current_dir(repo_root)` and Windows `CreateProcess`
///   searches the current directory ahead of `PATH`, so even a bare program
///   name resolves to a file the Write verbs can plant.
/// - the two `*_i_understand_the_risk` hatches re-arm what an ADR closed on
///   purpose: Copilot's credentialled builtin MCP server that can open a PR on
///   its own (ADR-0041 D7), Cursor's upload of the repository to the vendor
///   (ADR-0042 D6). The long key IS the gesture; a checkbox is not.
/// - `events.token` is the bearer every `CloudEvent` carries to the sink: set
///   remotely, it redirects the run's telemetry credential.
/// - `queue.trust_all_comments` lets a comment by any GitHub account reach the
///   agent's prompt: set remotely, it opens the prompt to strangers
///   (ADR-0072 D5).
/// - `claude.run_without_guard_proof_i_understand_the_risk` lets a Claude
///   session whose hooks did not prove they run go on, so the guard may not
///   run: set remotely, it turns off the one check that the guard is there
///   (ADR-0072 D6).
///
/// `pub(crate)` so the settings-panel gate in `lib.rs` can assert the schema
/// declares each of these `readonly` — a key denied here but offered as an
/// editable field is a control that takes an edit and answers "refused".
pub(crate) const LOCAL_ONLY_KEYS: [&str; 7] = [
    "verify.command",
    "copilot.allow_builtin_mcp_servers_i_understand_the_risk",
    "cursor.allow_codebase_indexing_i_understand_the_risk",
    "events.token",
    "queue.trust_all_comments",
    "claude.run_without_guard_proof_i_understand_the_risk",
    "antigravity.run_without_guard_proof_i_understand_the_risk",
];

/// Whether `key` may be set through the daemon's `config.set`/`config.unset`.
/// Well-shaped AND not exec-adjacent.
fn remotely_settable_key(key: &str) -> bool {
    well_shaped_key(key) && !LOCAL_ONLY_KEYS.contains(&key)
}

/// Compose the blessed argv for a config Query/Mutate verb (ADR-0036 §2). The
/// verb picks the static shape; `ConfigSet`/`ConfigUnset` read `key` (must match
/// `^[a-z0-9_.]+$`) and — for `set` — a non-empty `value` from `payload`, each
/// passed as a single argv token (never a shell string). Any absent/ill-shaped
/// value yields [`ArgvError`] and NO argv. Runs in `cwd = <repo path>` with no
/// `--repo` flag (defaults to `.`).
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "a verb this builder does not serve is refused, which fails closed"
)]
pub fn config_argv(verb: Verb, payload: &serde_json::Value) -> Result<Vec<String>, ArgvError> {
    let owned = |parts: &[&str]| parts.iter().map(ToString::to_string).collect::<Vec<_>>();
    let key = || -> Result<String, ArgvError> {
        let key = payload
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or(ArgvError::BadParam("key"))?;
        if remotely_settable_key(key) {
            Ok(key.to_owned())
        } else {
            Err(ArgvError::BadParam("key"))
        }
    };
    match verb {
        Verb::ConfigGet => Ok(owned(&["config", "get", "--json"])),
        // `--` ends option parsing: without it a value like `--help`/`-V` is
        // consumed by clap as a flag (help exits 0 → silent false-success), and a
        // legit dash-leading value can't be stored. The key is `^[a-z0-9_.]+$`, so
        // it is never a flag; `--` protects the free-text value.
        Verb::ConfigSet => {
            let key = key()?;
            let value = payload
                .get("value")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .ok_or(ArgvError::BadParam("value"))?;
            Ok(vec![
                "config".to_owned(),
                "set".to_owned(),
                "--".to_owned(),
                key,
                value.to_owned(),
            ])
        }
        Verb::ConfigUnset => Ok(vec![
            "config".to_owned(),
            "unset".to_owned(),
            "--".to_owned(),
            key()?,
        ]),
        // Non-config verbs never route here (`execute_oneshot` picks the branch
        // by effect class); refuse an argv defensively.
        _ => Err(ArgvError::BadParam("verb")),
    }
}
