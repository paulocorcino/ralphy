//! No menu or key sink takes a template string.

use super::*;

/// The `innerHTML` writes of `src` that take a template string with a value
/// in it, or a bare variable: each is a sink that would parse a file name as
/// markup.
fn template_html_sinks(src: &str) -> Vec<String> {
    let template = regex::Regex::new(r"innerHTML\s*=\s*`[^`]*\$\{").expect("valid regex");
    let variable = regex::Regex::new(r"innerHTML\s*=\s*[A-Za-z_$][\w$]*\s*;").expect("valid regex");
    template
        .find_iter(src)
        .chain(variable.find_iter(src))
        .map(|m| m.as_str().to_string())
        .collect()
}

/// The context menu and the console key bar build their icons as elements
/// and their text with `textContent`: no `innerHTML` there takes a template
/// string with a value in it, or a variable.
#[test]
fn no_menu_or_key_sink_takes_a_template_string() {
    // The matcher sees both shapes, across a line break too.
    assert_eq!(template_html_sinks("b.innerHTML = `<i>${x}</i>`;").len(), 1);
    assert_eq!(
        template_html_sinks("h.innerHTML =\n  `<i class=\"${a}\">`;").len(),
        1
    );
    assert_eq!(template_html_sinks("b.innerHTML = text;").len(), 1);
    assert!(template_html_sinks("b.innerHTML = '<i class=\"bi bi-x\"></i>';").is_empty());
    assert!(template_html_sinks("menu.innerHTML = \"\";").is_empty());
    // The whole console family, and the whole shell family: the shell,
    // `wb-files.ts`, `wb-move-dialog.ts` and the two consoles tab modules build menus.
    for (name, src) in [CONSOLE_FAMILY, SHELL_FAMILY].concat() {
        let sinks = template_html_sinks(src);
        assert!(sinks.is_empty(), "{name}: {sinks:?}");
    }
}
