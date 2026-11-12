use super::*;

#[test]
fn a_base_model_id_gets_the_effort_flag() {
    assert_eq!(
        effort_flag(Some("claude-sonnet-5-5"), Some("high")),
        Some("high")
    );
    assert_eq!(
        effort_flag(Some("gemini-3.8-flash"), Some("low")),
        Some("low")
    );
    assert_eq!(effort_flag(Some("gemini-3.8-flash"), None), None);
}

#[test]
fn a_suffixed_model_id_gets_no_effort_flag() {
    for id in [
        "gemini-3.8-flash-low",
        "claude-opus-5-5-medium",
        "gemini-4-argon-high",
        "gpt-oss-120b-medium",
    ] {
        assert_eq!(effort_flag(Some(id), Some("high")), None, "{id}");
    }
}

#[test]
fn price_key_strips_the_effort_suffix() {
    assert_eq!(price_key("gemini-3.8-flash-low"), "gemini-3.8-flash");
    assert_eq!(price_key("gpt-oss-120b-medium"), "gpt-oss-120b");
    assert_eq!(price_key("claude-opus-5-5-high"), "claude-opus-5-5");
    assert_eq!(price_key("gemini-4-argon"), "gemini-4-argon");
}
