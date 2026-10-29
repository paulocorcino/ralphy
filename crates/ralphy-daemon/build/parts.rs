//! The rule that puts parts into a page (ADR-0080 D1-D3).
//!
//! A part is a top-level `*.part.html` file. A page names it with one line
//! that holds only `<!-- part: <name>.part.html -->`. The build script
//! (`build/ui.rs`) and the daemon's tests both compile this file, because
//! cargo never runs the unit tests of a build script. It has no swc and no file
//! I/O, so both can include it.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};

const PART_SUFFIX: &str = ".part.html";
const MARKER_OPEN: &str = "<!-- part:";
const MARKER_CLOSE: &str = "-->";

/// What one line of a page says about parts.
enum Line<'a> {
    Text,
    /// A marker line, with the file name it names.
    Marker(&'a str),
    /// The line holds `<!-- part:` but is not a marker alone on its line.
    Mixed,
}

fn classify(line: &str) -> Line<'_> {
    let bare = line.trim();
    if !bare.contains(MARKER_OPEN) {
        return Line::Text;
    }
    let name = bare
        .strip_prefix(MARKER_OPEN)
        .and_then(|rest| rest.strip_suffix(MARKER_CLOSE))
        .map(str::trim)
        .filter(|name| name.ends_with(PART_SUFFIX) && !name.contains(char::is_whitespace));
    match name {
        Some(name) => Line::Marker(name),
        None => Line::Mixed,
    }
}

/// The pages to serve, with each marker line replaced by the text of its part.
///
/// `files` holds every top-level `.html` file of `assets/ui/` by name, pages
/// and parts. The result holds the pages only: a part is never served (D2).
pub fn put_together(files: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let (parts, pages): (BTreeMap<_, _>, BTreeMap<_, _>) = files
        .into_iter()
        .partition(|(name, _)| name.ends_with(PART_SUFFIX));
    for (name, text) in &parts {
        check_part(name, text)?;
    }
    let mut used = BTreeSet::new();
    let mut served = BTreeMap::new();
    for (name, text) in &pages {
        let built = fill_page(name, text, &parts, &mut used)?;
        served.insert(name.clone(), built);
    }
    if let Some(unused) = parts.keys().find(|name| !used.contains(name.as_str())) {
        bail!("no page names the part {unused}");
    }
    Ok(served)
}

/// A part does not name another part (D3).
fn check_part(name: &str, text: &str) -> Result<()> {
    if text.contains(MARKER_OPEN) {
        bail!("the part {name} holds a marker: a part does not name another part");
    }
    Ok(())
}

fn fill_page<'a>(
    page: &str,
    text: &'a str,
    parts: &'a BTreeMap<String, String>,
    used: &mut BTreeSet<&'a str>,
) -> Result<String> {
    let mut built = String::with_capacity(text.len());
    let mut named_here = BTreeSet::new();
    for line in text.split_inclusive('\n') {
        match classify(line) {
            Line::Text => built.push_str(line),
            Line::Mixed => bail!(
                "{page} has `{MARKER_OPEN}` with other text on its line: a marker is alone on its line"
            ),
            Line::Marker(name) => {
                let Some((key, part)) = parts.get_key_value(name) else {
                    bail!("{page} names the part {name}, which does not exist");
                };
                if !named_here.insert(name) {
                    bail!("{page} names the part {name} twice");
                }
                used.insert(key.as_str());
                built.push_str(part);
            }
        }
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> BTreeMap<String, String> {
        list.iter()
            .map(|(name, text)| (name.to_string(), text.to_string()))
            .collect()
    }

    fn error_of(list: &[(&str, &str)]) -> String {
        match put_together(files(list)) {
            Ok(served) => panic!("expected an error, got the pages {:?}", served.keys()),
            Err(e) => format!("{e:#}"),
        }
    }

    #[test]
    fn a_marker_line_becomes_the_text_of_its_part() {
        let served = put_together(files(&[
            (
                "index.html",
                "<body>\n  <!-- part: a.part.html -->\n</body>\n",
            ),
            ("a.part.html", "<p>one</p>\n<p>two</p>\n"),
        ]))
        .unwrap();
        assert_eq!(
            served["index.html"],
            "<body>\n<p>one</p>\n<p>two</p>\n</body>\n"
        );

        // A Windows checkout ends its lines with CRLF; the marker is still alone.
        let crlf = put_together(files(&[
            (
                "index.html",
                "<body>\r\n<!-- part: a.part.html -->\r\n</body>\r\n",
            ),
            ("a.part.html", "<p>one</p>\r\n"),
        ]))
        .unwrap();
        assert_eq!(crlf["index.html"], "<body>\r\n<p>one</p>\r\n</body>\r\n");
    }

    #[test]
    fn a_marker_that_names_a_missing_part_fails() {
        let e = error_of(&[("index.html", "<!-- part: gone.part.html -->\n")]);
        assert!(e.contains("index.html"), "{e}");
        assert!(e.contains("gone.part.html"), "{e}");
    }

    #[test]
    fn a_part_no_page_names_fails() {
        let e = error_of(&[
            ("index.html", "<body></body>\n"),
            ("lonely.part.html", "<p></p>\n"),
        ]);
        assert!(e.contains("lonely.part.html"), "{e}");
    }

    #[test]
    fn a_part_a_page_names_twice_fails() {
        let e = error_of(&[
            (
                "index.html",
                "<!-- part: a.part.html -->\n<!-- part: a.part.html -->\n",
            ),
            ("a.part.html", "<p></p>\n"),
        ]);
        assert!(e.contains("index.html"), "{e}");
        assert!(e.contains("a.part.html"), "{e}");
    }

    #[test]
    fn a_marker_inside_a_part_fails() {
        let e = error_of(&[
            ("index.html", "<!-- part: a.part.html -->\n"),
            ("a.part.html", "<!-- part: b.part.html -->\n"),
            ("b.part.html", "<p></p>\n"),
        ]);
        assert!(e.contains("a.part.html"), "{e}");
    }

    #[test]
    fn a_marker_that_shares_its_line_fails() {
        let e = error_of(&[
            ("index.html", "<div><!-- part: a.part.html --></div>\n"),
            ("a.part.html", "<p></p>\n"),
        ]);
        assert!(e.contains("index.html"), "{e}");
        assert!(e.contains("alone"), "{e}");
    }

    #[test]
    fn a_part_is_left_out_of_the_served_tree() {
        let served = put_together(files(&[
            ("index.html", "<!-- part: a.part.html -->\n"),
            ("other.html", "<!-- part: a.part.html -->\n"),
            ("a.part.html", "<p></p>\n"),
        ]))
        .unwrap();
        let names: Vec<&str> = served.keys().map(String::as_str).collect();
        assert_eq!(names, ["index.html", "other.html"]);
    }
}
