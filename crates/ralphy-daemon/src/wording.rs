//! The daemon failures of the wording catalog: `assets/wording/daemon.json`.

use std::sync::OnceLock;

use ralphy_wording::{failure_of, DomainFile, Failure, FailureSet, TextDomain};

const DAEMON_JSON: &str = include_str!("../../../assets/wording/daemon.json");

/// Why a command the daemon ran failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonFailure {
    /// A `ralphy` command failed with an error that has no failure of its own.
    RalphyFailed,
}

impl FailureSet for DaemonFailure {
    const DOMAIN: TextDomain = TextDomain::Daemon;

    fn all() -> &'static [Self] {
        &[Self::RalphyFailed]
    }

    fn code(self) -> &'static str {
        match self {
            Self::RalphyFailed => "ralphy-failed",
        }
    }

    fn detail_names(self) -> &'static [&'static str] {
        match self {
            Self::RalphyFailed => &["cause"],
        }
    }
}

fn daemon_file() -> &'static DomainFile {
    static FILE: OnceLock<DomainFile> = OnceLock::new();
    FILE.get_or_init(|| {
        DomainFile::parse(DAEMON_JSON).expect("daemon.json parses: the wording tests read it")
    })
}

impl DaemonFailure {
    /// The failure with its words from the catalog and `details` as its facts.
    ///
    /// # Panics
    /// Never: `daemon_failures_match_the_catalog` proves every code has an entry
    /// whose placeholders are details of the code.
    pub fn failure(self, details: &[(&str, &str)]) -> Failure {
        failure_of(daemon_file(), self, details)
            .expect("every code has an entry: the wording tests match the set both ways")
    }
}

#[cfg(test)]
mod tests {
    use ralphy_wording::check_failures;

    use super::*;

    #[test]
    fn daemon_failures_match_the_catalog() {
        assert_eq!(check_failures::<DaemonFailure>(daemon_file()), Ok(()));
    }

    #[test]
    fn ralphy_failed_reads_its_entry() {
        let failure = DaemonFailure::RalphyFailed.failure(&[("cause", "boom")]);
        assert_eq!(failure.text.cause, "the ralphy command failed");
        assert_eq!(
            failure.text.remedy.as_deref(),
            Some("The daemon log has the reason.")
        );
        assert_eq!(failure.details["cause"], "boom");
    }
}
