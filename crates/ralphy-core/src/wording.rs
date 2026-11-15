//! The working-tree failures of the wording catalog: `assets/wording/working-tree.json`.

use std::sync::OnceLock;

use ralphy_wording::{failure_of, DomainFile, Failure, FailureSet, TextDomain};

const WORKING_TREE_JSON: &str = include_str!("../../../assets/wording/working-tree.json");

/// Why a change to the working tree or its index was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingTreeFailure {
    NothingStaged,
    EmptyCommitMessage,
}

impl FailureSet for WorkingTreeFailure {
    const DOMAIN: TextDomain = TextDomain::WorkingTree;

    fn all() -> &'static [Self] {
        &[Self::NothingStaged, Self::EmptyCommitMessage]
    }

    fn code(self) -> &'static str {
        match self {
            Self::NothingStaged => "nothing-staged",
            Self::EmptyCommitMessage => "empty-commit-message",
        }
    }
}

fn working_tree_file() -> &'static DomainFile {
    static FILE: OnceLock<DomainFile> = OnceLock::new();
    FILE.get_or_init(|| {
        DomainFile::parse(WORKING_TREE_JSON)
            .expect("working-tree.json parses: the catalog_files test reads it")
    })
}

impl WorkingTreeFailure {
    /// The failure with its words from the catalog.
    ///
    /// # Panics
    /// Never: `working_tree_failures_match_the_catalog` proves every code has an entry.
    pub fn failure(self) -> Failure {
        failure_of(working_tree_file(), self, &[])
            .expect("every code has an entry: the wording tests match the set both ways")
    }
}

#[cfg(test)]
mod tests {
    use ralphy_wording::check_failures;

    use super::*;

    #[test]
    fn working_tree_failures_match_the_catalog() {
        assert_eq!(
            check_failures::<WorkingTreeFailure>(working_tree_file()),
            Ok(())
        );
    }

    #[test]
    fn nothing_staged_reads_its_entry() {
        let failure = WorkingTreeFailure::NothingStaged.failure();
        assert_eq!(failure.text.cause, "nothing is staged");
        assert_eq!(failure.text.remedy.as_deref(), Some("Stage a file first."));
    }
}
