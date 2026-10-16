//! The edges of the HTML and JavaScript scanners: input that is cut short or
//! shaped in a way the page rarely has.

use super::tests::{rows, seen};

#[test]
fn a_declaration_is_skipped_whole() {
    assert_eq!(
        seen(&rows(
            "index.html",
            r#"<!DOCTYPE title="Skipped"><p>Page</p>"#
        )),
        vec![("text", "Page")]
    );
}

#[test]
fn a_script_with_a_src_is_not_scanned_and_the_page_goes_on() {
    let html = "<script src=\"a.js\">el.textContent = \"In body\";</script><p>After</p>";
    assert_eq!(seen(&rows("index.html", html)), vec![("text", "After")]);
}

#[test]
fn a_script_that_never_closes_is_scanned_to_the_end_of_input() {
    let html = "<p>Before</p><script>el.textContent = \"Open\";";
    assert_eq!(
        seen(&rows("index.html", html)),
        vec![("text", "Before"), ("js:text-content", "Open")]
    );
}

#[test]
fn an_unquoted_attribute_value_ends_at_the_next_space() {
    assert_eq!(
        seen(&rows(
            "index.html",
            "<button title=Close class=x>X</button>"
        )),
        vec![("title", "Close"), ("text", "X")]
    );
}

#[test]
fn a_block_comment_that_never_closes_hides_the_rest_of_the_script() {
    let js = "el.textContent = \"Shown\"; /* never closed el.title = \"Hidden\";";
    assert_eq!(
        seen(&rows("wb-a.js", js)),
        vec![("js:text-content", "Shown")]
    );
}
