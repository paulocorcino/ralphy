use crate::DomainFile;

/// A closed set of values that a domain file words, one entry per value.
pub trait ValueSet: Copy + 'static {
    /// The name of the set in the domain file.
    const SET: &'static str;
    fn all() -> &'static [Self];
    /// The wire name of the value, the key of its entry.
    fn key(self) -> &'static str;
}

/// Checks that the file has an entry for each value of `T` and no other entry.
///
/// # Errors
/// Returns one line for each value with no entry and each entry with no value.
pub fn check_values<T: ValueSet>(file: &DomainFile) -> Result<(), Vec<String>> {
    let Some(entries) = file.values.get(T::SET) else {
        return Err(vec![format!("the file has no set `{}`", T::SET)]);
    };
    let mut problems = Vec::new();
    for value in T::all() {
        if !entries.contains_key(value.key()) {
            problems.push(format!("{}: value `{}` has no entry", T::SET, value.key()));
        }
    }
    for key in entries.keys() {
        if !T::all().iter().any(|v| v.key() == key) {
            problems.push(format!("{}: entry `{key}` has no value", T::SET));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    enum Pet {
        Cat,
        Dog,
    }

    impl ValueSet for Pet {
        const SET: &'static str = "pet";
        fn all() -> &'static [Self] {
            &[Self::Cat, Self::Dog]
        }
        fn key(self) -> &'static str {
            match self {
                Self::Cat => "cat",
                Self::Dog => "dog",
            }
        }
    }

    fn file(keys: &[&str]) -> DomainFile {
        let entries: Vec<String> = keys
            .iter()
            .map(|k| format!(r#""{k}": {{"text": "t", "note": "n"}}"#))
            .collect();
        let json = format!(
            r#"{{"domain": "run", "values": {{"pet": {{{}}}}}}}"#,
            entries.join(",")
        );
        DomainFile::parse(&json).expect("the test file parses")
    }

    #[test]
    fn a_matching_file_passes() {
        assert!(check_values::<Pet>(&file(&["cat", "dog"])).is_ok());
    }

    #[test]
    fn names_a_value_with_no_entry() {
        let problems = check_values::<Pet>(&file(&["cat"])).expect_err("dog is missing");
        assert_eq!(problems, ["pet: value `dog` has no entry"]);
    }

    #[test]
    fn names_an_entry_with_no_value() {
        let problems =
            check_values::<Pet>(&file(&["cat", "dog", "emu"])).expect_err("emu is extra");
        assert_eq!(problems, ["pet: entry `emu` has no value"]);
    }

    #[test]
    fn a_missing_set_is_reported() {
        let json = r#"{"domain": "run", "values": {}}"#;
        let empty = DomainFile::parse(json).expect("parses");
        assert!(check_values::<Pet>(&empty).is_err());
    }
}
