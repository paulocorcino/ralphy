//! The release watch (ADR-0056 §5–§7): does a newer build exist, and how loudly
//! should the workbench say so.
//!
//! The read itself lives in `ralphy-release`, a leaf crate the daemon may depend
//! on (ADR-0032 §10). This module is the daemon's half: the store paths, the
//! opt-out marker, the severity rule, and the view the workbench renders.
//!
//! Nothing here sends anything. The fetch is an unauthenticated GET with no
//! query beyond the page size, no identifier and no body, it is TTL-cached to
//! disk, and a failure is silent — the workbench shows what was last known.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ralphy_release::{standing, Build, Channel, Release, Standing};
use serde::Serialize;

/// The releases cache inside `dir` — a sibling of `repos.toml` and `desk.toml`,
/// so a scratch store (`$RALPHY_DAEMON_DIR`) stays self-contained.
pub fn cache_path_in(dir: &Path) -> PathBuf {
    dir.join("releases.json")
}

/// The opt-out marker inside `dir`. Its PRESENCE turns the watch off. A marker
/// rather than a field in the cache: that file is machine-owned, and a human
/// flag inside it would break the one-owner-per-file rule (ADR-0034 A3).
pub fn watch_off_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-release-watch-off")
}

/// Whether the operator has turned the watch off under `dir`.
pub fn watch_disabled_in(dir: &Path) -> bool {
    watch_off_path_in(dir).exists()
}

/// Turn the watch off (write the marker) or on (remove it). Idempotent both ways.
pub fn set_watch_disabled_in(dir: &Path, disabled: bool) -> Result<()> {
    let path = watch_off_path_in(dir);
    if disabled {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, "1").with_context(|| format!("writing {}", path.display()))
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }
}

/// How loudly the workbench says it. Derived from the *kinds* the gap carries,
/// never from the version delta: while the project ships candidates there is no
/// minor-versus-patch signal to read (ADR-0056 §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Nothing to say: level, ahead, or nothing readable.
    None,
    /// Fixes only — a dot, not a badge.
    Quiet,
    /// A feature arrived.
    Notable,
    /// Breaking or security — it stays on screen until dismissed.
    Urgent,
}

/// One release in the gap, as the workbench needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GapEntry {
    pub version: String,
    pub title: String,
    pub date: String,
    pub url: String,
    /// The section headings the body carried, lowercased (`new`, `fixed`,
    /// `breaking`, `security`).
    pub kinds: Vec<String>,
    /// The bullets under those headings, in order.
    pub highlights: Vec<String>,
}

/// What `/api/release` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseView {
    /// The build this daemon is running, verbatim.
    pub current: String,
    pub channel: String,
    /// `behind` · `level` · `ahead` · `unknown`.
    pub standing: String,
    pub severity: Severity,
    /// The newest release on the channel, when there is a newer one.
    pub latest: Option<String>,
    /// The whole gap from the running build to the newest, newest first — not
    /// just the last release, because the typical user is several behind.
    pub gap: Vec<GapEntry>,
    /// True when the operator turned the watch off; the view is then whatever
    /// was last cached, and nothing is being fetched.
    pub disabled: bool,
}

/// Build the view from a build string and what is cached. Pure: every input is
/// an argument, so the whole severity and gap rule is testable without a
/// network, a clock or a store.
pub fn view(
    raw_version: &str,
    releases: &[Release],
    channel: Channel,
    disabled: bool,
) -> ReleaseView {
    let build = Build::parse(raw_version);
    let standing = standing(&build, releases, channel);

    let (label, gap) = match &standing {
        Standing::Behind(gap) => ("behind", gap.iter().map(entry).collect::<Vec<_>>()),
        Standing::Level => ("level", Vec::new()),
        Standing::Ahead => ("ahead", Vec::new()),
        Standing::Unknown => ("unknown", Vec::new()),
    };

    ReleaseView {
        current: build.raw,
        channel: channel.to_string(),
        standing: label.to_string(),
        severity: severity(&gap),
        latest: gap.first().map(|e| e.version.clone()),
        gap,
        disabled,
    }
}

fn entry(release: &Release) -> GapEntry {
    let (kinds, highlights) = parse_body(release.body.as_deref().unwrap_or_default());
    GapEntry {
        version: release.tag_name.clone(),
        title: release.title().to_string(),
        date: release
            .published_at
            .as_deref()
            .and_then(|ts| ts.split('T').next())
            .unwrap_or_default()
            .to_string(),
        url: release.html_url.clone(),
        kinds,
        highlights,
    }
}

/// The loudest thing the gap carries.
fn severity(gap: &[GapEntry]) -> Severity {
    let has = |kind: &str| gap.iter().any(|e| e.kinds.iter().any(|k| k == kind));
    if gap.is_empty() {
        Severity::None
    } else if has("breaking") || has("security") {
        Severity::Urgent
    } else if has("new") {
        Severity::Notable
    } else {
        Severity::Quiet
    }
}

/// Read the headings and bullets out of a release body.
///
/// The body is what the fold renders (`### New`, then `- text (#123)`), so this
/// reads its own output. A release whose body is anything else — one of the
/// candidates published before the record started, with auto-generated notes —
/// yields no kinds and no highlights rather than a guess, which reads as
/// `Quiet`: something changed, and we cannot say what.
fn parse_body(body: &str) -> (Vec<String>, Vec<String>) {
    let mut kinds = Vec::new();
    let mut highlights = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("### ") {
            let kind = heading.trim().to_ascii_lowercase();
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        } else if let Some(item) = line.strip_prefix("- ") {
            // Drop the trailing `(#123)`: the workbench shows prose, and the
            // number belongs to a page it is not linking to.
            let text = match item.rsplit_once(" (#") {
                Some((head, tail)) if tail.ends_with(')') => head,
                _ => item,
            };
            let text = text.trim();
            if !text.is_empty() {
                highlights.push(text.to_string());
            }
        }
    }
    (kinds, highlights)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, body: &str) -> Release {
        Release {
            tag_name: tag.to_string(),
            name: None,
            published_at: Some(format!("2026-09-0{}T10:00:00Z", &tag[tag.len() - 1..])),
            html_url: format!("https://example.invalid/{tag}"),
            prerelease: true,
            draft: false,
            body: Some(body.to_string()),
            assets: Vec::new(),
        }
    }

    #[test]
    fn a_body_yields_its_headings_and_its_bullets() {
        let (kinds, highlights) = parse_body(
            "\n### New\n\n- Paste a screenshot. (#389)\n\n### Fixed\n\n- A fix. (#390)\n",
        );
        assert_eq!(kinds, vec!["new", "fixed"]);
        assert_eq!(highlights, vec!["Paste a screenshot.", "A fix."]);
    }

    #[test]
    fn a_body_that_is_not_ours_yields_nothing_rather_than_a_guess() {
        // What `--generate-notes` produced for the candidates before the record
        // started: a commit list under a `## What's Changed` heading.
        let (kinds, highlights) =
            parse_body("## What's Changed\n* feat(workbench): a thing by @someone in #1\n");
        assert!(kinds.is_empty());
        assert!(highlights.is_empty());
    }

    #[test]
    fn the_severity_is_the_loudest_thing_in_the_gap() {
        let quiet = view(
            "v0.1.0-rc.19",
            &[release("v0.1.0-rc.20", "### Fixed\n\n- A fix. (#1)\n")],
            Channel::Rc,
            false,
        );
        assert_eq!(quiet.severity, Severity::Quiet);

        let notable = view(
            "v0.1.0-rc.19",
            &[
                release("v0.1.0-rc.20", "### Fixed\n\n- A fix. (#1)\n"),
                release("v0.1.0-rc.21", "### New\n\n- A feature. (#2)\n"),
            ],
            Channel::Rc,
            false,
        );
        assert_eq!(
            notable.severity,
            Severity::Notable,
            "a feature anywhere in the gap outranks the fixes around it"
        );

        let urgent = view(
            "v0.1.0-rc.19",
            &[release(
                "v0.1.0-rc.20",
                "### Security\n\n- Closed a hole. (#3)\n",
            )],
            Channel::Rc,
            false,
        );
        assert_eq!(urgent.severity, Severity::Urgent);
    }

    #[test]
    fn the_whole_gap_is_carried_newest_first() {
        let v = view(
            "v0.1.0-rc.19",
            &[
                release("v0.1.0-rc.20", "### Fixed\n\n- Older. (#1)\n"),
                release("v0.1.0-rc.21", "### New\n\n- Newer. (#2)\n"),
            ],
            Channel::Rc,
            false,
        );
        assert_eq!(v.standing, "behind");
        assert_eq!(v.latest.as_deref(), Some("v0.1.0-rc.21"));
        let versions: Vec<&str> = v.gap.iter().map(|e| e.version.as_str()).collect();
        assert_eq!(versions, vec!["v0.1.0-rc.21", "v0.1.0-rc.20"]);
        assert_eq!(v.gap[0].highlights, vec!["Newer."]);
    }

    #[test]
    fn a_development_build_says_nothing() {
        let v = view(
            "v0.1.0-rc.19-18-gb2cc208",
            &[release("v0.1.0-rc.20", "### New\n\n- A feature. (#1)\n")],
            Channel::Rc,
            false,
        );
        assert_eq!(v.standing, "ahead");
        assert_eq!(v.severity, Severity::None);
        assert!(
            v.gap.is_empty(),
            "a dev build is never handed a gap to take"
        );
    }

    #[test]
    fn being_level_is_silent() {
        let v = view(
            "v0.1.0-rc.20",
            &[release("v0.1.0-rc.20", "### New\n\n- A feature. (#1)\n")],
            Channel::Rc,
            false,
        );
        assert_eq!(v.standing, "level");
        assert_eq!(v.severity, Severity::None);
    }

    #[test]
    fn the_disabled_flag_rides_the_view_rather_than_emptying_it() {
        // Turning the watch off stops the fetching, not the reporting: what was
        // last cached is still true, and the operator can see it.
        let v = view(
            "v0.1.0-rc.19",
            &[release("v0.1.0-rc.20", "### New\n\n- A feature. (#1)\n")],
            Channel::Rc,
            true,
        );
        assert!(v.disabled);
        assert_eq!(v.standing, "behind");
    }

    #[test]
    fn the_marker_toggles_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("ralphy-relwatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");

        assert!(!watch_disabled_in(&dir), "the watch is on by default");
        set_watch_disabled_in(&dir, true).expect("disable");
        set_watch_disabled_in(&dir, true).expect("disabling twice is not an error");
        assert!(watch_disabled_in(&dir));
        set_watch_disabled_in(&dir, false).expect("enable");
        set_watch_disabled_in(&dir, false).expect("enabling twice is not an error");
        assert!(!watch_disabled_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cache_is_a_sibling_of_the_rest_of_the_store() {
        let dir = Path::new("/tmp/store");
        assert_eq!(cache_path_in(dir), dir.join("releases.json"));
        assert_eq!(watch_off_path_in(dir), dir.join("daemon-release-watch-off"));
    }
}
