use super::*;

const REPORT: &str = include_str!("../../tests/fixtures/quota_report.json");
const EXHAUSTED: &str = include_str!("../../tests/fixtures/quota_report_exhausted.json");

#[test]
fn the_quota_reset_comes_from_the_exhausted_bucket_of_the_models_group() {
    assert_eq!(
        reset_time(EXHAUSTED, Some("gemini-3.8-flash-low")).as_deref(),
        Some("2026-11-12T16:32:40Z")
    );
    assert_eq!(
        reset_time(EXHAUSTED, Some("claude-sonnet-5-5-low")).as_deref(),
        Some("2026-11-12T17:13:52Z")
    );
    assert_eq!(
        reset_time(EXHAUSTED, Some("gpt-oss-120b-medium")).as_deref(),
        Some("2026-11-12T17:13:52Z")
    );
}

#[test]
fn no_exhausted_bucket_gives_no_reset() {
    assert_eq!(reset_time(REPORT, Some("gemini-3.8-flash-low")), None);
    assert_eq!(reset_time(REPORT, Some("claude-opus-5-5-low")), None);
}

#[test]
fn a_spent_bucket_of_another_group_gives_no_reset() {
    assert_eq!(reset_time(EXHAUSTED, Some("gemini-4-argon-medium")), None);
}

#[test]
fn the_latest_of_several_spent_buckets_is_the_reset() {
    let both = EXHAUSTED.replace(
        r#""remaining_fraction":0.9955034852027893"#,
        r#""remaining_fraction":0"#,
    );
    assert_eq!(
        reset_time(&both, Some("gemini-3.8-flash-low")).as_deref(),
        Some("2026-11-19T11:32:40Z")
    );
}

#[test]
fn no_model_or_an_unreadable_answer_gives_no_reset() {
    assert_eq!(reset_time(EXHAUSTED, None), None);
    assert_eq!(reset_time(EXHAUSTED, Some("other-model")), None);
    assert_eq!(reset_time("not json", Some("gemini-3.8-flash-low")), None);
    assert_eq!(reset_time("{}", Some("gemini-3.8-flash-low")), None);
}

#[test]
fn the_quota_argv_reads_the_owned_root_with_no_model() {
    let args = quota_args(Path::new("/r"));
    assert_eq!(
        args,
        ["--gemini_dir=/r", "-p", "/quota", "--output-format", "json"]
    );
}
