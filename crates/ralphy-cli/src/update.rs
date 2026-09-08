//! `ralphy update` — what is published, and where this build stands against it
//! (ADR-0056 §8).
//!
//! The read lives in `ralphy-release`; this module is the operator's view of it.
//! Nothing here reports a version the binary did not embed: `RALPHY_VERSION` is
//! the git-published string, and a build that has moved past its tag says so
//! rather than offering itself an update.

use anyhow::{anyhow, Result};
use clap::Args;
use ralphy_release::fetch::{self, RefreshOpts};
use ralphy_release::{standing, Build, Channel, Release, Standing};

#[derive(Args, Debug)]
pub(crate) struct UpdateArgs {
    /// Report what is published and exit without changing anything.
    #[arg(long)]
    pub(crate) check: bool,

    /// Which release stream to follow: `rc` (candidates included, the default
    /// while the project ships them) or `stable`.
    #[arg(long, default_value = "rc")]
    pub(crate) channel: String,

    /// Ask now instead of reading a cache that is still fresh.
    #[arg(long)]
    pub(crate) force: bool,
}

pub(crate) fn run(args: &UpdateArgs) -> Result<()> {
    let channel: Channel = args.channel.parse().map_err(|e: String| anyhow!(e))?;
    let build = Build::parse(env!("RALPHY_VERSION"));

    let cache = ralphy_release::cache_file()
        .ok_or_else(|| anyhow!("no home directory to hold the release cache"))?;
    let opts = RefreshOpts {
        force: args.force,
        ..RefreshOpts::new(&cache)
    };
    // Best-effort by construction: a failed fetch leaves the prior cache alone,
    // so the report below is "what we last knew" rather than an error.
    fetch::refresh_if_stale(&opts);
    let releases = fetch::load(&cache);

    let standing = standing(&build, &releases, channel);
    print!("{}", report(&build, channel, &standing, args.check));
    Ok(())
}

/// Render the whole report. Pure, so the wording is testable without a network,
/// a cache, or a binary of a particular version.
fn report(build: &Build, channel: Channel, standing: &Standing, check: bool) -> String {
    let mut out = format!("ralphy {} · channel {channel}\n", build.raw);
    match standing {
        Standing::Behind(gap) => {
            let plural = if gap.len() == 1 { "" } else { "s" };
            out.push_str(&format!(
                "{} newer release{plural}, newest first:\n",
                gap.len()
            ));
            for release in gap {
                out.push_str(&format!(
                    "  {:<16} {}\n",
                    release.tag_name,
                    published(release)
                ));
            }
            if !check {
                out.push_str(&manual_upgrade(&gap[0]));
            }
        }
        Standing::Level => out.push_str("up to date\n"),
        Standing::Ahead => {
            out.push_str("a development build, ahead of its tag — nothing to take");
            match build.tag.as_deref() {
                Some(tag) => out.push_str(&format!(" (tagged {tag})\n")),
                None => out.push('\n'),
            }
        }
        Standing::Unknown => {
            out.push_str("nothing to compare against");
            if fetch::offline_env() {
                out.push_str(" (RALPHY_RELEASE_OFFLINE=1)");
            }
            out.push('\n');
        }
    }
    out
}

/// The publication date, day-precision — the timestamp's time half says nothing
/// an operator acts on.
fn published(release: &Release) -> &str {
    release
        .published_at
        .as_deref()
        .and_then(|ts| ts.split('T').next())
        .unwrap_or("")
}

/// The upgrade instructions, until the self-replace lands. Kept separate so the
/// report above stays the same whichever way the operator takes the release.
fn manual_upgrade(newest: &Release) -> String {
    format!(
        "\ntake it from {}\nthen unpack it and run `ralphy install --force`\n",
        newest.html_url
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, published: &str) -> Release {
        Release {
            tag_name: tag.to_string(),
            name: None,
            published_at: Some(published.to_string()),
            html_url: format!("https://example.invalid/{tag}"),
            prerelease: true,
            draft: false,
            body: None,
        }
    }

    fn gap() -> Standing {
        Standing::Behind(vec![
            release("v0.1.0-rc.21", "2026-09-08T10:00:00Z"),
            release("v0.1.0-rc.20", "2026-09-07T10:00:00Z"),
        ])
    }

    #[test]
    fn an_unknown_channel_is_refused_by_name() {
        let args = UpdateArgs {
            check: true,
            channel: "nightly".into(),
            force: false,
        };
        let err = run(&args).expect_err("nightly is not a channel");
        assert!(
            err.to_string().contains("nightly"),
            "the refusal must name what was asked for: {err}"
        );
    }

    #[test]
    fn the_whole_gap_is_reported_newest_first_by_day() {
        let text = report(&Build::parse("v0.1.0-rc19"), Channel::Rc, &gap(), true);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "ralphy v0.1.0-rc19 · channel rc");
        assert_eq!(lines[1], "2 newer releases, newest first:");
        assert!(lines[2].starts_with("  v0.1.0-rc.21"), "{:?}", lines[2]);
        assert!(lines[2].ends_with(" 2026-09-08"), "{:?}", lines[2]);
        assert!(lines[3].starts_with("  v0.1.0-rc.20"), "{:?}", lines[3]);
    }

    #[test]
    fn a_dry_run_does_not_tell_the_operator_to_take_it() {
        let checked = report(&Build::parse("v0.1.0-rc19"), Channel::Rc, &gap(), true);
        assert!(!checked.contains("take it from"));

        let bare = report(&Build::parse("v0.1.0-rc19"), Channel::Rc, &gap(), false);
        assert!(bare.contains("take it from https://example.invalid/v0.1.0-rc.21"));
    }

    #[test]
    fn a_development_build_is_told_its_tag_not_offered_a_release() {
        let build = Build::parse("v0.1.0-rc19-18-gb2cc208");
        let text = report(&build, Channel::Rc, &Standing::Ahead, false);
        assert!(text.contains("development build"));
        assert!(
            text.contains("(tagged v0.1.0-rc19)"),
            "the operator's own tag spelling, not the normalized one: {text}"
        );
        assert!(!text.contains("take it from"));
    }

    #[test]
    fn being_level_says_so_in_one_line() {
        let text = report(
            &Build::parse("v0.1.0-rc19"),
            Channel::Rc,
            &Standing::Level,
            false,
        );
        assert_eq!(text.lines().count(), 2);
        assert!(text.ends_with("up to date\n"));
    }

    #[test]
    fn this_build_never_reports_itself_behind() {
        // Whatever RALPHY_VERSION is in the tree the test runs from, comparing
        // it against an empty release list must not produce an update offer.
        let build = Build::parse(env!("RALPHY_VERSION"));
        assert!(!matches!(
            standing(&build, &[], Channel::Rc),
            Standing::Behind(_)
        ));
    }
}
