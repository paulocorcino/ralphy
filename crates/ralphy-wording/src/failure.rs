use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::TextDomain;

/// The words of a failure, filled: the cause and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureText {
    pub cause: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remedy: Option<String>,
}

/// A failure as it crosses a door: a code, the facts, and the words already filled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub domain: TextDomain,
    pub code: String,
    pub details: BTreeMap<String, String>,
    pub text: FailureText,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text.cause)
    }
}

impl std::error::Error for Failure {}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRE: &str = r#"{"domain":"working-tree","code":"nothing-staged","details":{},
        "text":{"cause":"nothing is staged","remedy":"Stage a file first."}}"#;

    #[test]
    fn the_wire_form_round_trips() {
        let failure: Failure = serde_json::from_str(WIRE).expect("the wire form reads");
        assert_eq!(failure.domain, TextDomain::WorkingTree);
        assert_eq!(failure.code, "nothing-staged");
        assert_eq!(failure.text.remedy.as_deref(), Some("Stage a file first."));
        let value = serde_json::to_value(&failure).expect("it writes");
        let expected: serde_json::Value = serde_json::from_str(WIRE).expect("json");
        assert_eq!(value, expected);
        let bare = r#"{"domain":"run","code":"c","details":{"a":"b"},"text":{"cause":"x"}}"#;
        let failure: Failure = serde_json::from_str(bare).expect("remedy may be absent");
        assert_eq!(failure.text.remedy, None);
        let value = serde_json::to_value(&failure).expect("it writes");
        assert_eq!(
            value,
            serde_json::from_str::<serde_json::Value>(bare).expect("json")
        );
    }

    #[test]
    fn anyhow_carries_the_failure() {
        let failure: Failure = serde_json::from_str(WIRE).expect("the wire form reads");
        // anyhow::Error::new takes exactly an Error + Send + Sync + 'static; the ratchet
        // `wording_depends_only_on_serde` keeps anyhow out of this crate, even as a dev dependency.
        let error: Box<dyn std::error::Error + Send + Sync + 'static> = Box::new(failure.clone());
        assert_eq!(error.downcast_ref::<Failure>(), Some(&failure));
        assert_eq!(error.to_string(), "nothing is staged");
    }
}
