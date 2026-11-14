use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::TextDomain;

/// The wording of one value of a closed set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueWording {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Plain-text form of the icon, for a channel that cannot show it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// Context for the writer: JSON has no comments.
    pub note: String,
}

/// The content of `assets/wording/<text-domain>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainFile {
    pub domain: TextDomain,
    /// Set name, then value key, then the wording.
    pub values: BTreeMap<String, BTreeMap<String, ValueWording>>,
}

impl DomainFile {
    /// Reads a domain file; a domain outside `TextDomain` or an unknown field is an error.
    ///
    /// # Errors
    /// Returns the JSON error text when the document does not match the form.
    pub fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("reading a wording domain file: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"domain": "run", "values": {"issue-status": {"done":
        {"text": "done", "icon": "x", "fallback": "[ok]", "note": "n"}}}}"#;

    #[test]
    fn parses_a_domain_file() {
        let file = DomainFile::parse(GOOD).expect("a good file parses");
        assert_eq!(file.domain, TextDomain::Run);
        let done = &file.values["issue-status"]["done"];
        assert_eq!(done.text, "done");
        assert_eq!(done.icon.as_deref(), Some("x"));
        assert_eq!(done.fallback.as_deref(), Some("[ok]"));
    }

    #[test]
    fn icon_and_fallback_are_optional_but_note_is_not() {
        let ok = r#"{"domain": "run", "values": {"s": {"k": {"text": "t", "note": "n"}}}}"#;
        assert!(DomainFile::parse(ok).is_ok());
        let no_note = r#"{"domain": "run", "values": {"s": {"k": {"text": "t"}}}}"#;
        assert!(DomainFile::parse(no_note).is_err());
    }

    #[test]
    fn refuses_a_domain_outside_the_text_domains() {
        let bad = GOOD.replace("\"run\"", "\"cli\"");
        assert!(DomainFile::parse(&bad).is_err());
    }

    #[test]
    fn refuses_an_unknown_field() {
        let top = GOOD.replace("\"values\"", "\"extra\": 1, \"values\"");
        assert!(DomainFile::parse(&top).is_err());
        let inner = GOOD.replace("\"note\"", "\"color\": \"red\", \"note\"");
        assert!(DomainFile::parse(&inner).is_err());
    }
}
