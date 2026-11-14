use std::fmt;

/// Why a template could not be filled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillError {
    /// The template names a placeholder that no argument fills.
    Missing(String),
    /// An argument matches no placeholder of the template.
    Extra(String),
}

impl fmt::Display for FillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "no value for the placeholder `{name}`"),
            Self::Extra(name) => write!(f, "the template has no placeholder `{name}`"),
        }
    }
}

impl std::error::Error for FillError {}

/// The `{name}` placeholders of a template, in order of appearance.
#[must_use]
pub fn placeholders(template: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some((_, after)) = rest.split_once('{') {
        let Some((name, tail)) = after.split_once('}') else {
            break;
        };
        names.push(name);
        rest = tail;
    }
    names
}

/// Replaces every `{name}` with its argument.
///
/// # Errors
/// A placeholder with no argument, or an argument with no placeholder, is refused.
pub fn fill(template: &str, args: &[(&str, &str)]) -> Result<String, FillError> {
    let names = placeholders(template);
    if let Some((extra, _)) = args.iter().find(|(name, _)| !names.contains(name)) {
        return Err(FillError::Extra((*extra).to_owned()));
    }
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after)) = rest.split_once('{') {
        let Some((name, tail)) = after.split_once('}') else {
            break;
        };
        let Some((_, value)) = args.iter().find(|(n, _)| *n == name) else {
            return Err(FillError::Missing(name.to_owned()));
        };
        out.push_str(before);
        out.push_str(value);
        rest = tail;
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_placeholders_in_order() {
        assert_eq!(placeholders("a {x} b {y} {x}"), ["x", "y", "x"]);
        assert_eq!(placeholders("none"), Vec::<&str>::new());
    }

    #[test]
    fn replaces_every_placeholder() {
        let out = fill("{a} and {b} and {a}", &[("a", "1"), ("b", "2")]);
        assert_eq!(out.as_deref(), Ok("1 and 2 and 1"));
    }

    #[test]
    fn a_value_is_not_filled_again() {
        let out = fill("{a} {b}", &[("a", "{b}"), ("b", "x")]);
        assert_eq!(out.as_deref(), Ok("{b} x"));
    }

    #[test]
    fn a_missing_argument_is_named() {
        assert_eq!(
            fill("{a} {b}", &[("a", "1")]),
            Err(FillError::Missing("b".to_owned()))
        );
    }

    #[test]
    fn an_extra_argument_is_named() {
        assert_eq!(
            fill("{a}", &[("a", "1"), ("z", "2")]),
            Err(FillError::Extra("z".to_owned()))
        );
    }
}
