//! The token store (docs/adr/0032 §4): a mint-once 256-bit access token in
//! the global store, a SEPARATE file from `daemon.toml` so its lifecycle
//! survives a re-`daemon setup`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Env override for the access token: when set non-empty it wins over the
/// on-disk token (a spawned daemon can be handed its token this way). Stripped
/// from the process env at boot so no child inherits it (mirrors
/// `RALPHY_EVENTS_TOKEN`, ADR-0019).
pub const TOKEN_ENV: &str = "RALPHY_DAEMON_TOKEN";

/// The global daemon store root: `$RALPHY_DAEMON_DIR` when set (tests point it at
/// a temp dir), else `<home>/.ralphy` — the same root as `daemon.toml`, never a
/// repo-local `.ralphy/`. The one env-reading resolver the token/seed/password
/// paths and the security routes share.
pub fn store_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("RALPHY_DAEMON_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .context("could not resolve a home directory for the daemon store")?;
    Ok(PathBuf::from(home).join(".ralphy"))
}

/// The `daemon-token` path inside `dir`. Path-explicit so the security routes and
/// tests can point it at a temp dir without touching the process-global env.
pub fn token_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-token")
}

/// The production path of `daemon-token`. Mirrors [`identity::daemon_toml_path`].
pub fn token_path() -> Result<PathBuf> {
    Ok(token_path_in(&store_dir()?))
}

/// Load the token from `path`, or `Ok(None)` when the file does not exist yet
/// (an un-minted token). Trims a trailing newline so an editor-touched file
/// still compares equal.
pub fn load_token_from(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.trim_end_matches(['\r', '\n']).to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `token` to `path` owner-only, creating the parent directory.
pub fn save_token_to(token: &str, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, token).with_context(|| format!("writing {}", path.display()))?;
    set_owner_only(path)?;
    Ok(())
}

/// Generate a fresh access token: 32 CSPRNG bytes (256 bits) hex-encoded to 64
/// chars. No `hex` crate — inline `format!`.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS CSPRNG must be available to mint a token");
    let mut hex = String::with_capacity(64);
    for b in bytes {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Mint-once at `path`: return the existing token with `false`, or generate,
/// save, and return a fresh one with `true`. The `bool` is "was newly minted",
/// so `daemon setup` can show it exactly once.
pub fn ensure_token_at(path: &Path) -> Result<(String, bool)> {
    match load_token_from(path)? {
        Some(token) => Ok((token, false)),
        None => {
            let token = generate_token();
            save_token_to(&token, path)?;
            Ok((token, true))
        }
    }
}

/// Load the current access token from its production path.
pub fn load_token() -> Result<Option<String>> {
    load_token_from(&token_path()?)
}

/// The effective token: a non-empty [`TOKEN_ENV`] override wins, else the
/// on-disk token. `None` when neither resolves.
pub fn effective_token() -> Result<Option<String>> {
    if let Some(v) = std::env::var_os(TOKEN_ENV) {
        let v = v.to_string_lossy().into_owned();
        if !v.is_empty() {
            return Ok(Some(v));
        }
    }
    load_token()
}

/// Remove [`TOKEN_ENV`] from the process environment so no spawned child inherits
/// the access token. Called once at boot after the effective token is captured
/// into the [`super::AuthPolicy`] (mirrors `strip_events_token_from_env`, ADR-0019).
pub fn strip_token_from_env() {
    std::env::remove_var(TOKEN_ENV);
}

/// Restrict a freshly written token file to the owner only (mode `0o600` on
/// unix; the per-user home ACL on Windows), mirroring `identity::set_owner_only`.
#[cfg(unix)]
pub(crate) fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("setting owner-only permissions on {}", path.display()))
}

#[cfg(not(unix))]
pub(crate) fn set_owner_only(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
