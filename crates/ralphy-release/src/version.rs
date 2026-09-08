//! Version identity (ADR-0056 §5): what a build calls itself, and how that
//! string compares to a published release tag.
//!
//! Two facts make this less trivial than `Version::parse`. The build embeds
//! `git describe --tags --match 'v*' --always --dirty`, so `RALPHY_VERSION` is
//! not always a version: it carries a `v` prefix, may carry a commits-ahead and
//! short-sha suffix, may carry `-dirty`, and on a tree with no reachable tag is
//! a bare sha. And the project's own tags spell the candidate two ways —
//! `v0.1.0-rc19` historically, `v0.1.0-rc.20` from now on — which semver orders
//! *backwards* across the change, because `rc19` is one alphanumeric identifier
//! compared ASCII-wise while `rc.20` is `rc` followed by a numeric one, and
//! `"rc" < "rc19"`. Normalizing a trailing digit run into its own numeric
//! identifier makes both spellings order by the number a human reads.

use semver::Version;

/// A build's own claim about itself, parsed out of `RALPHY_VERSION`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// The embedded string, verbatim, for display.
    pub raw: String,
    /// The tag it was cut from, verbatim, when the string carries one — the
    /// spelling the operator will find on the Releases page, not the normalized
    /// form `version` compares by.
    pub tag: Option<String>,
    /// That tag as a comparable version.
    pub version: Option<Version>,
    /// The tree had moved past that tag: commits ahead, or dirty, or both.
    pub ahead: bool,
}

impl Build {
    /// Parse an embedded `RALPHY_VERSION`. Never fails: a string this cannot
    /// read becomes `version: None`, which every caller treats as "say nothing".
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        let mut rest = trimmed;
        let mut ahead = false;

        if let Some(stripped) = rest.strip_suffix("-dirty") {
            ahead = true;
            rest = stripped;
        }
        if let Some(stripped) = strip_describe_suffix(rest) {
            ahead = true;
            rest = stripped;
        }

        let version = parse_tag(rest);
        Build {
            raw: trimmed.to_string(),
            tag: version.is_some().then(|| rest.to_string()),
            version,
            ahead,
        }
    }
}

/// Parse a release tag (`v0.1.0-rc.20`, `v0.1.0-rc19`, `0.2.0`) into a version
/// that orders the way a reader expects. Returns `None` for anything that is
/// not a version — a bare describe sha, a `ralphy/pre-run-*` tag, an empty
/// string.
pub fn parse_tag(tag: &str) -> Option<Version> {
    let trimmed = tag.trim();
    let core = trimmed.strip_prefix('v').unwrap_or(trimmed);
    Version::parse(&normalize_prerelease(core)).ok()
}

/// Split a trailing digit run out of every pre-release identifier, so `rc19`
/// and `rc.19` become the same two identifiers and both order numerically.
/// Leading zeros are dropped, because a numeric semver identifier may not carry
/// them (`rc019` would otherwise fail to parse at all).
fn normalize_prerelease(core: &str) -> String {
    // Build metadata never participates in precedence; leave it untouched.
    let (versionish, build_meta) = match core.split_once('+') {
        Some((v, b)) => (v, Some(b)),
        None => (core, None),
    };
    let Some((numbers, pre)) = versionish.split_once('-') else {
        return core.to_string();
    };

    let normalized: Vec<String> = pre.split('.').map(normalize_identifier).collect();
    let mut out = format!("{numbers}-{}", normalized.join("."));
    if let Some(meta) = build_meta {
        out.push('+');
        out.push_str(meta);
    }
    out
}

fn normalize_identifier(id: &str) -> String {
    // Walk the trailing digit run back by CHARACTER, not by byte: `rfind(..) + 1`
    // lands mid-character whenever the char before the digits is multi-byte, and
    // `split_at` on a non-boundary panics. A tag is network data, so that panic
    // would be reachable from a release nobody in this repo published.
    let digits_start = id
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map_or(id.len(), |(index, _)| index);
    // All digits (already numeric) or no digits at all: nothing to split.
    if digits_start == 0 || digits_start == id.len() {
        return id.to_string();
    }
    let (alpha, digits) = id.split_at(digits_start);
    let trimmed = digits.trim_start_matches('0');
    let digits = if trimmed.is_empty() { "0" } else { trimmed };
    format!("{alpha}.{digits}")
}

/// Strip a `git describe` `-<commits>-g<sha>` suffix, returning the tag it was
/// counted from. `None` when the string does not end in one.
fn strip_describe_suffix(s: &str) -> Option<&str> {
    let (head, sha) = s.rsplit_once('-')?;
    let hex = sha.strip_prefix('g')?;
    if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let (tag, commits) = head.rsplit_once('-')?;
    if commits.is_empty() || !commits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(tag: &str) -> Version {
        parse_tag(tag).unwrap_or_else(|| panic!("{tag} must parse"))
    }

    #[test]
    fn a_candidate_orders_by_the_number_a_human_reads() {
        // The bug this whole module exists for: raw semver puts rc19 below rc9,
        // because one alphanumeric identifier is compared ASCII-wise.
        assert!(v("v0.1.0-rc9") < v("v0.1.0-rc19"));
        assert!(v("v0.1.0-rc.9") < v("v0.1.0-rc.10"));
        assert!(v("v0.1.0-rc.10") < v("v0.1.0-rc.19"));
    }

    #[test]
    fn the_dotted_spelling_outranks_the_undotted_one_it_replaces() {
        // The transition case. Without normalization `0.1.0-rc.20` sorts BELOW
        // `0.1.0-rc19` ("rc" < "rc19"), so every user on rc19 would be told
        // they were up to date forever.
        assert!(v("v0.1.0-rc19") < v("v0.1.0-rc.20"));
        assert!(v("v0.1.0-rc.19") == v("v0.1.0-rc19"));
    }

    #[test]
    fn a_release_outranks_its_own_candidates() {
        assert!(v("v0.1.0-rc.99") < v("v0.1.0"));
        assert!(v("v0.1.0") < v("v0.2.0-rc.1"));
    }

    #[test]
    fn a_padded_candidate_still_parses_and_orders() {
        // A numeric semver identifier may not carry leading zeros, so rc019
        // must lose them rather than fail to parse.
        assert!(v("v0.1.0-rc019") == v("v0.1.0-rc.19"));
    }

    #[test]
    fn a_non_ascii_tag_is_refused_rather_than_panicking() {
        // `rfind(non-digit) + 1` used to land inside the `é` and panic in
        // `split_at`. A tag is network data: every release in the fetched
        // document is parsed, so one such tag would have aborted `ralphy update`
        // and the daemon's release view for everyone.
        assert_eq!(
            parse_tag("v1.0.0-café19"),
            None,
            "not a version, but no panic"
        );
        assert_eq!(parse_tag("v1.0.0-é1"), None);
        // The same shape with an ASCII prefix still normalizes.
        assert!(v("v1.0.0-rc9") < v("v1.0.0-rc10"));
        // A multi-byte character that is not adjacent to digits is harmless too.
        assert_eq!(parse_tag("café"), None);
        assert_eq!(parse_tag("日本語"), None);
    }

    #[test]
    fn a_clean_tag_build_is_not_ahead() {
        let b = Build::parse("v0.1.0-rc19");
        assert!(!b.ahead);
        assert_eq!(b.version, Some(v("v0.1.0-rc19")));
        assert_eq!(b.raw, "v0.1.0-rc19");
        assert_eq!(b.tag.as_deref(), Some("v0.1.0-rc19"));
    }

    #[test]
    fn a_build_past_its_tag_is_ahead() {
        let b = Build::parse("v0.1.0-rc19-18-gb2cc208");
        assert!(b.ahead, "commits past the tag means ahead, never behind");
        assert_eq!(b.version, Some(v("v0.1.0-rc19")));
        assert_eq!(
            b.tag.as_deref(),
            Some("v0.1.0-rc19"),
            "the tag keeps the operator's spelling, not the normalized one"
        );
    }

    #[test]
    fn a_dirty_build_is_ahead() {
        assert!(Build::parse("v0.1.0-rc19-dirty").ahead);
        assert!(Build::parse("v0.1.0-rc.20-4-gdeadbee-dirty").ahead);
    }

    #[test]
    fn a_bare_sha_carries_no_version() {
        // `git describe --always` with no reachable v* tag.
        let b = Build::parse("b2cc208");
        assert_eq!(b.version, None);
        assert_eq!(b.tag, None);
        assert!(!b.ahead);
    }

    #[test]
    fn a_pre_run_tag_is_not_a_version() {
        // `--match 'v*'` keeps these out of describe, but a hand-passed string
        // must not be read as one either.
        assert_eq!(parse_tag("ralphy/pre-run-20260731-202919"), None);
    }

    #[test]
    fn a_suffix_that_only_looks_like_describe_is_left_alone() {
        // `-g<hex>` alone, with no commit count, is part of the pre-release.
        assert!(!Build::parse("v0.1.0-rc.1-gamma").ahead);
        assert!(!Build::parse("v0.1.0-alpha-2").ahead);
    }

    #[test]
    fn the_manifest_fallback_parses() {
        // A source tarball has no .git, so build.rs falls back to the manifest
        // version: no `v`, no describe suffix.
        let b = Build::parse("0.1.0-rc19");
        assert!(!b.ahead);
        assert_eq!(b.version, Some(v("v0.1.0-rc19")));
    }
}
