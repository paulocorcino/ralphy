//! The configuration root Ralphy owns (ADR-0043 D4).
//!
//! `GEMINI_CLI_HOME` names a directory the CLI appends `.gemini` to, so pointing
//! it at `<workspace>/.ralphy/gemini-home` gives every run a root Ralphy created:
//! the operator's `~/.gemini` is never read by the child and never written, their
//! `GEMINI.md` never reaches the prompt, and `--model` cannot rewrite their
//! defaults.
//!
//! The root is **persistent**, not scratch: `.ralphy/` is gitignored (`*`), so it
//! cannot dirty the tree Ralphy refuses to run against, and keeping it across runs
//! keeps `installation_id` stable rather than minting a throwaway identity per
//! invocation.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// The directory name under the caller's base directory.
const ROOT_DIR_NAME: &str = "gemini-home";

/// The subdirectory the CLI itself appends to `GEMINI_CLI_HOME`.
const CLI_SUBDIR: &str = ".gemini";

/// Ralphy's own root on disk, after [`ensure`].
pub(crate) struct GeminiRoot {
    /// What `GEMINI_CLI_HOME` is set to — the CLI appends `.gemini` itself.
    pub(crate) home: PathBuf,
    /// `<home>/.gemini/settings.json`, written by [`ensure`].
    pub(crate) settings: PathBuf,
}

impl GeminiRoot {
    /// `<home>/.gemini` — where the policy document and the settings live.
    pub(crate) fn cli_dir(&self) -> PathBuf {
        self.home.join(CLI_SUBDIR)
    }
}

/// The operator's own root. Ralphy reads exactly two things from it — the declared
/// auth mode and their restrictive policy rules — and the CHILD never sees it.
pub(crate) fn operator_root() -> Option<PathBuf> {
    ralphy_proc_util::home_dir().map(|h| h.join(CLI_SUBDIR))
}

/// The operator's declared authentication mode, from `settings.json`'s
/// `security.auth.selectedType`.
///
/// A NON-SECRET POINTER, not a credential: it names which mode the operator chose,
/// never the key itself (ADR-0043 D17 — the credential is never read, copied or
/// replayed). Without it an isolated root is exit 41 for every operator whose key
/// lives in the OS credential store. Any error — missing file, bad JSON, missing
/// key — is `None`, which forwards nothing and lets the vendor's own sentence
/// surface (D6) rather than guessing an auth mode.
pub(crate) fn operator_auth_type(root: Option<&Path>) -> Option<String> {
    let text = std::fs::read_to_string(root?.join("settings.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("security")?
        .get("auth")?
        .get("selectedType")?
        .as_str()
        .map(str::to_string)
}

/// The minimal settings document Ralphy writes into its own root.
///
/// Three keys and no more:
/// - `security.auth.selectedType` mirrors the operator's declared mode, so an
///   isolated root authenticates the way their own does (omitted entirely when
///   unknown, so exit 41 surfaces with the vendor's own instruction);
/// - `privacy.usageStatisticsEnabled = false` — a run is not the operator opting
///   into telemetry;
/// - `experimental.enableAgents = false` — defensive only (D15). The policy's
///   `invoke_agent` deny is the load-bearing control; this key was observed NOT to
///   remove the tool from the schema, which is why it is not relied on.
///
/// Pure over its input so the document is asserted without touching a filesystem.
pub(crate) fn settings_document(auth_type: Option<&str>) -> Value {
    let mut doc = json!({
        "privacy": { "usageStatisticsEnabled": false },
        "experimental": { "enableAgents": false }
    });
    if let Some(t) = auth_type {
        doc["security"] = json!({ "auth": { "selectedType": t } });
    }
    doc
}

/// Create `<base>/gemini-home/.gemini/` and write `settings.json` into it.
///
/// `base` is a directory rather than a `Workspace` so `ralphy init` — which has no
/// workspace — can pass `<home>/.ralphy` and reach the same code path, instead of
/// a second root implementation that can drift, and without minting a throwaway
/// installation identity on every probe.
///
/// The file is rewritten only when its bytes differ, so a run does not churn the
/// mtime of a root that is already correct, and a drifted or truncated file is
/// repaired. Nothing else in the directory is touched.
pub(crate) fn ensure(base: &Path) -> Result<GeminiRoot> {
    let home = base.join(ROOT_DIR_NAME);
    let cli_dir = home.join(CLI_SUBDIR);
    std::fs::create_dir_all(&cli_dir)
        .with_context(|| format!("creating the owned gemini root {}", cli_dir.display()))?;

    let settings = cli_dir.join("settings.json");
    let auth_type = operator_auth_type(operator_root().as_deref());
    let want = format!(
        "{}\n",
        serde_json::to_string_pretty(&settings_document(auth_type.as_deref()))?
    );
    let differs = std::fs::read_to_string(&settings)
        .map(|s| s != want)
        .unwrap_or(true);
    if differs {
        std::fs::write(&settings, &want)
            .with_context(|| format!("writing {}", settings.display()))?;
    }
    Ok(GeminiRoot { home, settings })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D4: the root is per-workspace and persistent, so `ensure` must be a no-op
    /// on a root that is already correct — a rewrite every run would churn the
    /// installation identity the vendor keys on.
    #[test]
    fn ensure_is_idempotent() {
        let base = tempfile::tempdir().unwrap();
        let first = ensure(base.path()).unwrap();
        let bytes = std::fs::read(&first.settings).unwrap();
        let second = ensure(base.path()).unwrap();
        assert_eq!(first.settings, second.settings);
        assert_eq!(std::fs::read(&second.settings).unwrap(), bytes);

        // Exactly one file, in exactly one place.
        let entries: Vec<_> = std::fs::read_dir(first.cli_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, ["settings.json"], "{entries:?}");
        assert_eq!(first.home, base.path().join("gemini-home"));
        assert!(first.settings.starts_with(&first.home));
    }

    /// A truncated or hand-edited settings file is repaired — and anything else
    /// the root accumulated across runs (session state, the installation id) is
    /// left exactly as it was.
    #[test]
    fn ensure_restores_a_drifted_settings_file() {
        let base = tempfile::tempdir().unwrap();
        let root = ensure(base.path()).unwrap();
        let want = std::fs::read_to_string(&root.settings).unwrap();

        let sibling = root.cli_dir().join("installation_id");
        std::fs::write(&sibling, b"keep-me").unwrap();
        std::fs::write(&root.settings, b"{ corrupted").unwrap();

        ensure(base.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&root.settings).unwrap(), want);
        assert_eq!(
            std::fs::read_to_string(&sibling).unwrap(),
            "keep-me",
            "an unrelated sibling file must survive"
        );
    }

    /// The two keys Ralphy forces regardless of the operator, and the one it
    /// mirrors. An absent auth mode omits the key entirely rather than guessing.
    #[test]
    fn the_settings_document_forces_the_privacy_and_agents_keys() {
        let doc = settings_document(Some("gemini-api-key"));
        assert_eq!(doc["privacy"]["usageStatisticsEnabled"], json!(false));
        assert_eq!(doc["experimental"]["enableAgents"], json!(false));
        assert_eq!(
            doc["security"]["auth"]["selectedType"],
            json!("gemini-api-key")
        );

        let unknown = settings_document(None);
        assert!(
            unknown.get("security").is_none(),
            "an unknown auth mode must not be guessed: {unknown}"
        );
        assert_eq!(unknown["privacy"]["usageStatisticsEnabled"], json!(false));
    }

    /// The auth-mode read is a pointer lookup that fails to `None` on every bad
    /// shape — never a panic, never a partial guess.
    #[test]
    fn the_operator_auth_type_fails_soft() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(operator_auth_type(None), None);
        assert_eq!(operator_auth_type(Some(dir.path())), None, "no file");

        std::fs::write(dir.path().join("settings.json"), b"not json").unwrap();
        assert_eq!(operator_auth_type(Some(dir.path())), None, "bad json");

        std::fs::write(dir.path().join("settings.json"), br#"{"security":{}}"#).unwrap();
        assert_eq!(operator_auth_type(Some(dir.path())), None, "missing key");

        std::fs::write(
            dir.path().join("settings.json"),
            br#"{"security":{"auth":{"selectedType":"vertex-ai"}}}"#,
        )
        .unwrap();
        assert_eq!(
            operator_auth_type(Some(dir.path())).as_deref(),
            Some("vertex-ai")
        );
    }

    /// D17: the operator's root is reached for the auth POINTER and their policy
    /// rules only — no credential file is ever named here.
    #[test]
    fn the_root_module_names_no_credential_file() {
        let production = include_str!("root.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for banned in ["oauth_creds", "google_accounts", "keytar", "access_token"] {
            assert!(
                !production.contains(banned),
                "the credential is never read (D17); found {banned}"
            );
        }
    }
}
