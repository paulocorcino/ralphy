use super::*;

const MODELS: &str = include_str!("../../tests/fixtures/models.txt");

#[test]
fn the_catalog_parses_the_thirteen_ids_of_agy_models() {
    let models = parse(MODELS);
    assert_eq!(models.len(), 13);
    assert_eq!(models[0].id, "gemini-4-argon-medium");
    assert_eq!(models[0].label, "Gemini 4 Argon (Medium)");
    assert_eq!(models[12].id, "gpt-oss-120b-medium");
    assert_eq!(models[12].label, "GPT-OSS 120B (Medium)");
}

#[test]
fn the_catalog_skips_lines_that_are_not_a_model() {
    let text = "Fetching available models...\r\n\tno id\r\nid-only\t\r\nm-1\tModel 1\r\n";
    assert_eq!(
        parse(text),
        vec![ModelEntry {
            id: "m-1".into(),
            label: "Model 1".into()
        }]
    );
}
