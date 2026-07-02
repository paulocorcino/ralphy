//! Vendor-neutral **detection scaffolds** for scanning a captured log: the
//! auth-error substring matcher ([`auth_error`]) and the usage-limit combinator
//! ([`detect_limit`]) plus the line-delimited-JSON scanner ([`scan_json_lines`])
//! its structural variants build on.
//!
//! Each adapter keeps its own vendor decision — which substrings signal auth,
//! which reset-string format to parse, which JSON fields mark a limit — and passes
//! it in. These helpers return a `bool`/`Option`, never an `Outcome`: the seam
//! ADR-0004 protects (each adapter's `classify_*`) stays untouched.

/// Return `true` when `text` matches any auth marker **group**. The outer slice is
/// OR (any group matching wins); each inner slice is AND (every substring in the
/// group must be present in `text`). Matching is case-insensitive, so **markers
/// must be given in lowercase**.
///
/// Codex/OpenCode pass single-substring groups (a plain OR of substrings);
/// Claude's per-line wrapper passes one AND-group — the genuine logged-out banner
/// carries both `not logged in` and `please run /login` on the same line, so an
/// AND avoids matching prose that merely mentions one of them.
pub fn auth_error(text: &str, markers: &[&[&str]]) -> bool {
    let lower = text.to_ascii_lowercase();
    markers
        .iter()
        .any(|group| group.iter().all(|needle| lower.contains(needle)))
}

/// Scan a line-delimited JSON stream, returning the first `pick` result that is
/// `Some`. Each line is trimmed and skipped unless it starts with `{` and parses
/// as a JSON object/value; `pick` inspects the parsed value and returns `Some`
/// when it is the event being looked for. `None` when no line matches.
///
/// This is the shared scaffold under Claude's transcript limit scan and OpenCode's
/// error-event limit scan — both walk the stream line by line, parse each JSON
/// record, and stop at the first that matches a vendor-specific predicate.
pub fn scan_json_lines<T>(
    text: &str,
    mut pick: impl FnMut(&serde_json::Value) -> Option<T>,
) -> Option<T> {
    text.lines().find_map(|line| {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            return None;
        }
        let value = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
        pick(&value)
    })
}

/// The "scan output for a limit, extract a reset hint" combinator, returning the
/// usage-limit shape the adapters classify on:
/// - `Some(Some(hint))` — a limit was detected and a reset hint was parsed.
/// - `Some(None)` — a limit was detected but no reset hint was found.
/// - `None` — no limit was detected.
///
/// The vendor supplies both halves: `is_limit` decides whether `text` shows a
/// usage limit at all, and `reset_hint` extracts the (best-effort) reset string in
/// that vendor's own format. Used by the text-based Claude and Codex detectors;
/// OpenCode's structural JSON parser produces the same shape via [`scan_json_lines`].
pub fn detect_limit(
    text: &str,
    is_limit: impl Fn(&str) -> bool,
    reset_hint: impl Fn(&str) -> Option<String>,
) -> Option<Option<String>> {
    is_limit(text).then(|| reset_hint(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_error_or_of_single_substring_groups() {
        let markers: &[&[&str]] = &[
            &["401 unauthorized"],
            &["missing bearer or basic authentication"],
        ];
        // Either group alone matches (case-insensitive).
        assert!(auth_error("HTTP 401 Unauthorized", markers));
        assert!(auth_error(
            "Missing bearer or basic authentication in header",
            markers
        ));
        assert!(!auth_error("everything is fine", markers));
    }

    #[test]
    fn auth_error_and_group_needs_all_substrings() {
        let markers: &[&[&str]] = &[&["not logged in", "please run /login"]];
        // Both substrings present → match.
        assert!(auth_error("Not logged in · Please run /login", markers));
        // Only one present → no match (guards against prose mentioning one).
        assert!(!auth_error("you are not logged in yet", markers));
        assert!(!auth_error("please run /login to continue", markers));
    }

    #[test]
    fn scan_json_lines_returns_first_match_and_skips_noise() {
        let stream = "not json\n{\"type\":\"other\"}\n{\"type\":\"target\",\"v\":7}\n{\"type\":\"target\",\"v\":9}";
        let got = scan_json_lines(stream, |v| {
            (v.get("type").and_then(|t| t.as_str()) == Some("target"))
                .then(|| v.get("v").and_then(|n| n.as_u64()))
                .flatten()
        });
        assert_eq!(got, Some(7));
    }

    #[test]
    fn scan_json_lines_none_when_no_match() {
        assert_eq!(
            scan_json_lines::<u64>("plain\n{\"type\":\"x\"}", |_| None),
            None
        );
    }

    #[test]
    fn detect_limit_maps_the_three_states() {
        // limit + hint
        assert_eq!(
            detect_limit("limit", |_| true, |_| Some("08:10".into())),
            Some(Some("08:10".to_string()))
        );
        // limit, no hint
        assert_eq!(detect_limit("limit", |_| true, |_| None), Some(None));
        // no limit
        assert_eq!(detect_limit("fine", |_| false, |_| Some("x".into())), None);
    }
}
