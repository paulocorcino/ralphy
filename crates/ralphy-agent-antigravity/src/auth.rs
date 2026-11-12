//! Signing in to `agy` (ADR-0086 D7) and the limit class of its errors (D6).
//! The signals are text: the logged-out signature was measured on Linux; on
//! Windows the same strings are in the binary, not yet seen in a run.

use std::path::Path;

use ralphy_adapter_support::auth_error;

/// The first stderr line of a logged-out run. `agy` then waits 60 s for a
/// sign-in code that a headless run never sends.
const AUTH_REQUIRED: &[&str] = &["authentication required", "please visit the url to log in"];

/// The error of a logged-out run, on stderr and in `result.error`.
const AUTH_FAILED: &[&str] = &["authentication failed or timed out"];

/// What `agy models` prints on stderr when no one is signed in.
const MODELS_SIGN_IN: &[&str] = &["please sign in to view available models"];

/// The words of a quota or rate error, lowercase (spike §7).
const LIMIT_WORDS: [&str; 5] = [
    "resource_exhausted",
    "quota",
    "rate limit",
    "exhausted",
    "credits balance is too low",
];

/// The HTTP status of a rate error, matched as a whole word: an id in the
/// same line may hold the digits.
const LIMIT_CODE: &str = "429";

/// Whether the operator is signed in to `agy` for Ralphy: `agy models` in
/// the owned root lists models. It makes no model call. A missing `agy`, a
/// sign-in error or any other failure reads as not signed in.
pub fn probe_antigravity_login() -> bool {
    crate::catalog::fetch_catalog().is_ok()
}

/// The message of a run that found `agy` signed out. `agy` has no login
/// command: the operator starts it in Ralphy's root and signs in.
pub fn auth_message(root: &Path) -> String {
    format!(
        "Antigravity is not signed in for Ralphy. Run `agy --gemini_dir=\"{}\"` with no other \
         argument, sign in, then run Ralphy again.",
        root.display()
    )
}

/// The early kill: `line` is the first stderr line of a logged-out run.
pub(crate) fn is_auth_line(line: &str) -> bool {
    auth_error(line, &[AUTH_REQUIRED])
}

/// `text` shows a logged-out run, by its first stderr line or its final error.
pub(crate) fn is_auth_error(text: &str) -> bool {
    auth_error(text, &[AUTH_REQUIRED, AUTH_FAILED])
}

/// `agy models` ended with the sign-in error: no one is signed in. Any other
/// failure is not an answer about the sign-in.
pub(crate) fn models_reads_logged_out(code: Option<i32>, stderr: &str) -> bool {
    code != Some(0) && auth_error(stderr, &[MODELS_SIGN_IN])
}

/// `stderr`, the `AGY_ERROR:` line included, names a quota or rate limit.
pub(crate) fn is_limit(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    LIMIT_WORDS.iter().any(|word| lower.contains(word)) || has_word(&lower, LIMIT_CODE)
}

/// `word` is in `text` with no letter or digit on either side.
fn has_word(text: &str, word: &str) -> bool {
    let bytes = text.as_bytes();
    text.match_indices(word).any(|(at, _)| {
        let before = at.checked_sub(1).and_then(|i| bytes.get(i));
        let after = bytes.get(at + word.len());
        !before.is_some_and(u8::is_ascii_alphanumeric)
            && !after.is_some_and(u8::is_ascii_alphanumeric)
    })
}

#[cfg(test)]
mod tests;
