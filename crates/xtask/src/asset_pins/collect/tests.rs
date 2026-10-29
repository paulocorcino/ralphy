use super::*;

fn pins_of(text: &str) -> Vec<Pin> {
    let mut out = Vec::new();
    collect("src/tests/page.rs", text, &mut out);
    out
}

/// The tests read a page through a constant that holds the built page, not
/// through `include_str!` of its source file. A claim made through the
/// constant is still a claim about that page.
#[test]
fn a_built_page_constant_is_attributed_to_its_page() {
    let pins = pins_of(
        r#"
fn the_shell_has_a_rail() {
    let html = super::INDEX_HTML;
    assert!(html.contains("id=\"rail\""));
}

fn the_popup_has_a_title() {
    let page = crate::tests::DETACHED_HTML;
    assert!(page.contains("<title>"));
}

fn the_fence_has_a_stage() {
    let page = super::DETACHED_FENCE_HTML;
    assert!(page.contains("id=\"stage\""));
}
"#,
    );
    let assets: Vec<Option<&str>> = pins.iter().map(|p| p.asset.as_deref()).collect();
    assert_eq!(
        assets,
        [
            Some("index.html"),
            Some("detached.html"),
            Some("detached-fence.html")
        ]
    );
}
