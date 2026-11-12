//! The display names of workbench consoles.

/// Mint a workbench console's display name: `wb-<repo>-<4 hex>`.
///
/// The `wb-` head is what the operator reads the answer off — a bare
/// `ralphy-3f9c` is the shape a session picks for ITSELF, so without the head a
/// roster row cannot say whether a workbench opened it. `<repo>` is the last
/// segment of the `owner/repo` slug, folded to `[a-z0-9-]` because the name is
/// typed back as an address.
///
/// The suffix is two CSPRNG bytes, hex, mirroring `auth::generate_token`.
///
/// # Panics
///
/// When the OS random source is not available.
pub fn console_name(repo_slug: &str) -> String {
    let tail = repo_slug.rsplit('/').next().unwrap_or_default();
    let repo = fold(tail);
    let repo = if repo.is_empty() { "repo" } else { &repo };

    let mut bytes = [0u8; 2];
    getrandom::fill(&mut bytes)
        .expect("the OS CSPRNG must be available to name a workbench console");
    format!("wb-{repo}-{:02x}{:02x}", bytes[0], bytes[1])
}

/// Claude's `--name` for a workbench console: `wb-` + the folded console name,
/// or [`console_name`] when there is none or it folds to nothing.
///
/// The raw console name never reaches argv: `portable-pty` does not escape
/// `& | < > ^ %`, and a `.cmd` shim is read again by `cmd.exe` (ADR-0066 §6).
pub(super) fn claude_session_name(repo_slug: &str, console: Option<&str>) -> String {
    console
        .map(fold)
        .filter(|folded| !folded.is_empty())
        .map_or_else(|| console_name(repo_slug), |folded| format!("wb-{folded}"))
}

/// Fold `text` to lowercase `[a-z0-9]` joined by single dashes, trimmed.
fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9') => out.push(c),
            // Collapse every run of punctuation into ONE dash, so `my..repo`
            // and `my-repo` do not read as different names.
            _ if !out.ends_with('-') => out.push('-'),
            _ => {}
        }
    }
    out.trim_matches('-').to_owned()
}
