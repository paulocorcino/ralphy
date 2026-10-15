use super::*;

const QUOTA_LINE: &str = "ActionRequiredError: You've hit your usage limit";

#[test]
fn a_failed_turn_end_with_no_reason_is_an_error_without_a_sentence() {
    let fold = fold_cursor_stream(r#"{"type":"turn_ended","status":"error"}"#);
    assert!(fold.saw_turn_end);
    assert!(fold.is_error);
    assert_eq!(fold.vendor_error, None);
}

#[test]
fn a_bare_quota_line_does_not_replace_the_reason_of_a_turn_end() {
    let stream = format!(
        "{}\n{QUOTA_LINE}\n",
        r#"{"type":"turn_ended","status":"error","error":"first sentence"}"#
    );
    let fold = fold_cursor_stream(&stream);
    assert_eq!(fold.vendor_error.as_deref(), Some("first sentence"));
}
