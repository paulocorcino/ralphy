//! The run values of the wording catalog: `assets/wording/run.json`, read once.

use std::sync::OnceLock;

use ralphy_wording::{DomainFile, ValueSet, ValueWording};

use crate::runstate::{snapshot, IssueStatus};
use crate::ui::FinishOutcome;

const RUN_JSON: &str = include_str!("../../../assets/wording/run.json");

fn run_file() -> &'static DomainFile {
    static FILE: OnceLock<DomainFile> = OnceLock::new();
    FILE.get_or_init(|| {
        DomainFile::parse(RUN_JSON).expect("run.json parses: the catalog_files test reads it")
    })
}

fn entry<T: ValueSet>(value: T) -> &'static ValueWording {
    run_file()
        .values
        .get(T::SET)
        .and_then(|set| set.get(value.key()))
        .expect("every value has an entry: the wording tests match the set both ways")
}

impl ValueSet for IssueStatus {
    const SET: &'static str = "issue-status";

    fn all() -> &'static [Self] {
        &[
            Self::Planning,
            Self::Executing,
            Self::Planned,
            Self::Done,
            Self::Skipped,
            Self::Blocked,
            Self::Infeasible,
            Self::NeedsSplit,
            Self::NonGreen,
            Self::Hitl,
        ]
    }

    fn key(self) -> &'static str {
        snapshot::status_wire(self)
    }
}

impl ValueSet for FinishOutcome {
    const SET: &'static str = "run-outcome";

    fn all() -> &'static [Self] {
        &[
            Self::Done,
            Self::Blocked,
            Self::Timeout,
            Self::Stuck,
            Self::Limit,
        ]
    }

    fn key(self) -> &'static str {
        self.label()
    }
}

impl IssueStatus {
    /// The text, icon and fallback of this status in the catalog.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the console and Telegram read it in a later change"
        )
    )]
    pub(crate) fn wording(self) -> &'static ValueWording {
        entry(self)
    }
}

impl FinishOutcome {
    /// The text, icon and fallback of this outcome in the catalog.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the console reads it in a later change")
    )]
    pub(crate) fn wording(self) -> &'static ValueWording {
        entry(self)
    }
}

#[cfg(test)]
mod tests;
