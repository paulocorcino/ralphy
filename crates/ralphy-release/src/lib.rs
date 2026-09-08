//! The published-release read (ADR-0056 §4).
//!
//! A leaf crate: it has no edge to `ralphy-core`, so the daemon may depend on
//! it under ADR-0032 §10's leaf-crate exception, and `ralphy update` in the CLI
//! depends on it too. Those two callers are why it is a crate rather than a
//! module on one side of the seam.
//!
//! It answers exactly one question — is this build behind, level with, or ahead
//! of what has been published — and it answers it from a TTL disk cache that a
//! failed fetch never disturbs. Nothing is sent: the fetch is an unauthenticated
//! `GET` with no query beyond the page size, no identifier and no body.

pub mod fetch;
pub mod version;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use fetch::{refresh_if_stale, RefreshOpts, DEFAULT_RELEASES_URL};
pub use version::{parse_tag, Build};

/// Resolve the releases disk cache: `$RALPHY_RELEASE_CACHE` when set, else
/// `<home>/.ralphy/releases.json`. `None` when the home directory cannot be
/// resolved at all, which every caller reads as "do not cache, do not fetch".
///
/// The daemon overrides this with a path inside its own store, so a scratch
/// store stays self-contained; the CLI takes the default.
pub fn cache_file() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("RALPHY_RELEASE_CACHE") {
        return Some(PathBuf::from(file));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".ralphy").join("releases.json"))
}

/// Which stream of releases a Ralphy follows. `Rc` is the default while the
/// project ships candidates, so the first stable release is a change of default
/// rather than a break for the operators who want them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Rc,
    Stable,
}

impl Channel {
    /// Whether this channel carries `release`.
    pub fn admits(self, release: &Release) -> bool {
        match self {
            Channel::Rc => true,
            Channel::Stable => !release.prerelease,
        }
    }
}

impl std::str::FromStr for Channel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rc" => Ok(Channel::Rc),
            "stable" => Ok(Channel::Stable),
            other => Err(format!("unknown channel `{other}` (expected rc or stable)")),
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Channel::Rc => "rc",
            Channel::Stable => "stable",
        })
    }
}

/// One published release, in GitHub's own field spelling so the API response
/// deserializes straight into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub body: Option<String>,
}

impl Release {
    /// The tag as a comparable version, or `None` when the tag is not one.
    pub fn version(&self) -> Option<semver::Version> {
        parse_tag(&self.tag_name)
    }

    /// What to call it on screen: the release's own title when it has one that
    /// says more than the tag, else the tag.
    pub fn title(&self) -> &str {
        match self.name.as_deref() {
            Some(name) if !name.trim().is_empty() => name,
            _ => &self.tag_name,
        }
    }
}

/// Where a build stands against what has been published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// Newer releases exist, newest first. The whole gap, not just the newest:
    /// at this cadence the typical user is several versions behind, and a panel
    /// showing only the last one hides the reason to upgrade (ADR-0056 §7).
    Behind(Vec<Release>),
    /// Level with the newest release on this channel.
    Level,
    /// The tree moved past its tag — a development build. Never offered an
    /// update: overwriting a developer's own binary with a release is wrong.
    Ahead,
    /// Nothing readable to compare: no releases, or a build string that is not
    /// a version (a bare describe sha, a source tarball with no tag).
    Unknown,
}

/// Compare `build` against `releases` on `channel`.
///
/// Drafts, releases the channel does not admit, and tags that are not versions
/// are all skipped rather than guessed at.
pub fn standing(build: &Build, releases: &[Release], channel: Channel) -> Standing {
    if build.ahead {
        return Standing::Ahead;
    }
    let Some(current) = build.version.as_ref() else {
        return Standing::Unknown;
    };

    let mut newer: Vec<(semver::Version, Release)> = releases
        .iter()
        .filter(|r| !r.draft && channel.admits(r))
        .filter_map(|r| r.version().map(|v| (v, r.clone())))
        .filter(|(v, _)| v > current)
        .collect();

    if newer.is_empty() {
        // "Level" only means something if there was anything comparable at all.
        let comparable = releases
            .iter()
            .any(|r| !r.draft && channel.admits(r) && r.version().is_some());
        return if comparable {
            Standing::Level
        } else {
            Standing::Unknown
        };
    }

    newer.sort_by(|a, b| b.0.cmp(&a.0));
    Standing::Behind(newer.into_iter().map(|(_, r)| r).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.to_string(),
            name: None,
            published_at: None,
            html_url: String::new(),
            prerelease,
            draft: false,
            body: None,
        }
    }

    fn candidates() -> Vec<Release> {
        vec![
            release("v0.1.0-rc.20", true),
            release("v0.1.0-rc19", true),
            release("v0.1.0-rc9", true),
        ]
    }

    #[test]
    fn a_build_behind_gets_the_whole_gap_newest_first() {
        let build = Build::parse("v0.1.0-rc9");
        let Standing::Behind(gap) = standing(&build, &candidates(), Channel::Rc) else {
            panic!("rc9 is behind rc19 and rc.20");
        };
        let tags: Vec<&str> = gap.iter().map(|r| r.tag_name.as_str()).collect();
        assert_eq!(tags, vec!["v0.1.0-rc.20", "v0.1.0-rc19"]);
    }

    #[test]
    fn the_newest_build_is_level() {
        let build = Build::parse("v0.1.0-rc.20");
        assert_eq!(
            standing(&build, &candidates(), Channel::Rc),
            Standing::Level
        );
    }

    #[test]
    fn a_development_build_is_never_offered_an_update() {
        let build = Build::parse("v0.1.0-rc9-18-gb2cc208");
        assert_eq!(
            standing(&build, &candidates(), Channel::Rc),
            Standing::Ahead
        );
        assert_eq!(
            standing(
                &Build::parse("v0.1.0-rc9-dirty"),
                &candidates(),
                Channel::Rc
            ),
            Standing::Ahead
        );
    }

    #[test]
    fn the_stable_channel_does_not_see_candidates() {
        let build = Build::parse("v0.1.0-rc9");
        assert_eq!(
            standing(&build, &candidates(), Channel::Stable),
            Standing::Unknown,
            "nothing on the stable channel is comparable yet"
        );

        let mut with_stable = candidates();
        with_stable.push(release("v0.2.0", false));
        let Standing::Behind(gap) = standing(&build, &with_stable, Channel::Stable) else {
            panic!("0.2.0 is a stable release newer than rc9");
        };
        assert_eq!(gap.len(), 1, "the candidates must not appear: {gap:?}");
        assert_eq!(gap[0].tag_name, "v0.2.0");
    }

    #[test]
    fn a_draft_is_never_an_update() {
        let mut releases = candidates();
        let mut draft = release("v0.9.0", false);
        draft.draft = true;
        releases.push(draft);
        let build = Build::parse("v0.1.0-rc.20");
        assert_eq!(standing(&build, &releases, Channel::Rc), Standing::Level);
    }

    #[test]
    fn an_unreadable_build_string_says_nothing() {
        assert_eq!(
            standing(&Build::parse("b2cc208"), &candidates(), Channel::Rc),
            Standing::Unknown
        );
    }

    #[test]
    fn a_tag_that_is_not_a_version_is_skipped_not_guessed() {
        let releases = vec![release("nightly", true), release("v0.1.0-rc.20", true)];
        let Standing::Behind(gap) = standing(&Build::parse("v0.1.0-rc9"), &releases, Channel::Rc)
        else {
            panic!("rc.20 is newer than rc9");
        };
        assert_eq!(gap.len(), 1);
        assert_eq!(gap[0].tag_name, "v0.1.0-rc.20");
    }

    #[test]
    fn a_channel_round_trips_through_its_spelling() {
        for channel in [Channel::Rc, Channel::Stable] {
            let spelled = channel.to_string();
            assert_eq!(spelled.parse::<Channel>(), Ok(channel));
        }
        assert!("nightly".parse::<Channel>().is_err());
    }

    #[test]
    fn a_release_titles_itself_by_tag_when_it_has_no_name() {
        let mut r = release("v0.1.0-rc.20", true);
        assert_eq!(r.title(), "v0.1.0-rc.20");
        r.name = Some("  ".into());
        assert_eq!(r.title(), "v0.1.0-rc.20", "a blank name is not a title");
        r.name = Some("The one with the console".into());
        assert_eq!(r.title(), "The one with the console");
    }
}
