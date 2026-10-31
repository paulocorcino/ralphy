use super::*;
use crate::dispatch::EffectClass;
use serde_json::json;

mod changes;
mod git;
mod spawn;

#[test]
fn argv_error_keeps_its_text() {
    assert_eq!(
        ArgvError::BadParam("agent").to_string(),
        "invalid run param: agent"
    );
}
