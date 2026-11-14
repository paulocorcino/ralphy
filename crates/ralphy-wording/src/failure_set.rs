use crate::{fill, placeholders, DomainFile, Failure, FailureText, TextDomain};

/// A closed set of failure codes that one domain file words.
pub trait FailureSet: Copy + 'static {
    const DOMAIN: TextDomain;
    fn all() -> &'static [Self];
    /// The wire name of the code, the key of its entry.
    fn code(self) -> &'static str;
    /// The names of the details this code carries; the wording may use only these.
    fn detail_names(self) -> &'static [&'static str] {
        &[]
    }
}

/// Builds the failure for `code`, with the cause and the remedy filled from `details`.
///
/// # Errors
/// The file has no entry for the code, or a placeholder has no detail.
pub fn failure_of<T: FailureSet>(
    file: &DomainFile,
    code: T,
    details: &[(&str, &str)],
) -> Result<Failure, String> {
    let name = code.code();
    let entry = file
        .failures
        .get(name)
        .ok_or_else(|| format!("the wording of failure `{name}` has no entry"))?;
    let fill_one = |template: &str| {
        let used: Vec<(&str, &str)> = details
            .iter()
            .filter(|(key, _)| placeholders(template).contains(key))
            .copied()
            .collect();
        fill(template, &used).map_err(|e| format!("failure `{name}`: {e}"))
    };
    let cause = fill_one(&entry.cause)?;
    let remedy = entry.remedy.as_deref().map(fill_one).transpose()?;
    Ok(Failure {
        domain: T::DOMAIN,
        code: name.to_owned(),
        details: details
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        text: FailureText { cause, remedy },
    })
}

/// Checks that the file words each code of `T` and no other, that each placeholder is a
/// detail of its code, and that no live code is in `retired`.
///
/// # Errors
/// Returns one line for each problem.
pub fn check_failures<T: FailureSet>(file: &DomainFile) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    if file.domain != T::DOMAIN {
        problems.push(format!(
            "the file is for the domain `{}`",
            file.domain.as_str()
        ));
    }
    for code in T::all() {
        check_code(file, *code, &mut problems);
    }
    for name in file.failures.keys() {
        if !T::all().iter().any(|c| c.code() == name) {
            problems.push(format!("entry `{name}` has no failure"));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

fn check_code<T: FailureSet>(file: &DomainFile, code: T, problems: &mut Vec<String>) {
    let name = code.code();
    let Some(entry) = file.failures.get(name) else {
        problems.push(format!("failure `{name}` has no entry"));
        return;
    };
    let texts = std::iter::once(entry.cause.as_str()).chain(entry.remedy.as_deref());
    for holder in texts.flat_map(placeholders) {
        if !code.detail_names().contains(&holder) {
            problems.push(format!("failure `{name}`: `{{{holder}}}` is not a detail"));
        }
    }
    if file.retired.iter().any(|r| r == name) {
        problems.push(format!("failure `{name}` is live and also retired"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    enum Sample {
        Plain,
        Named,
    }

    impl FailureSet for Sample {
        const DOMAIN: TextDomain = TextDomain::WorkingTree;
        fn all() -> &'static [Self] {
            &[Self::Plain, Self::Named]
        }
        fn code(self) -> &'static str {
            match self {
                Self::Plain => "plain",
                Self::Named => "named",
            }
        }
        fn detail_names(self) -> &'static [&'static str] {
            match self {
                Self::Plain => &[],
                Self::Named => &["path"],
            }
        }
    }

    fn file(entries: &str, retired: &str) -> DomainFile {
        let json = format!(
            r#"{{"domain": "working-tree", "values": {{}},
            "failures": {{{entries}}}, "retired": [{retired}]}}"#
        );
        DomainFile::parse(&json).expect("the test file parses")
    }

    const BOTH: &str = r#""plain": {"cause": "c", "note": "n"},
        "named": {"cause": "no {path}", "remedy": "Fix {path}.", "note": "n"}"#;

    #[test]
    fn failure_of_fills_the_wording() {
        let f = file(BOTH, "");
        let failure = failure_of(&f, Sample::Named, &[("path", "a.rs")]).expect("fills");
        assert_eq!(failure.text.cause, "no a.rs");
        assert_eq!(failure.text.remedy.as_deref(), Some("Fix a.rs."));
        assert_eq!(failure.details["path"], "a.rs");
        assert_eq!(failure.domain, TextDomain::WorkingTree);
        assert_eq!(failure.code, "named");
        assert!(failure_of(&f, Sample::Named, &[]).is_err());
        let plain = failure_of(&f, Sample::Plain, &[]).expect("fills");
        assert_eq!(plain.text.remedy, None);
    }

    #[test]
    fn check_failures_reports_both_sides() {
        assert!(check_failures::<Sample>(&file(BOTH, "")).is_ok());
        let only_plain = r#""plain": {"cause": "c", "note": "n"}"#;
        let problems = check_failures::<Sample>(&file(only_plain, "")).expect_err("named");
        assert_eq!(problems, ["failure `named` has no entry"]);
        let extra = format!(r#"{BOTH}, "ghost": {{"cause": "c", "note": "n"}}"#);
        let problems = check_failures::<Sample>(&file(&extra, "")).expect_err("ghost");
        assert_eq!(problems, ["entry `ghost` has no failure"]);
        let bad = BOTH.replace("no {path}", "no {file}");
        assert!(check_failures::<Sample>(&file(&bad, "")).is_err());
    }

    #[test]
    fn a_retired_code_is_never_live() {
        let problems =
            check_failures::<Sample>(&file(BOTH, r#""plain", "old""#)).expect_err("plain is live");
        assert_eq!(problems, ["failure `plain` is live and also retired"]);
    }
}
