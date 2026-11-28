//! Helpers for the source pins of the adapter tests: they read a file's
//! production code as text. Built only with the `source-scan` feature, which
//! the adapters turn on in their dev-dependencies.

/// The production half of a source file: the text before its first
/// `#[cfg(test)]` line whose next non-empty line starts with `mod `. An
/// item-level `#[cfg(test)]` (a test-only helper above production code) is not
/// the cut, so a source scan still reads the code after it.
pub fn production_text(src: &str) -> &str {
    let mut offset = 0;
    let mut lines = src.split_inclusive('\n');
    while let Some(line) = lines.next() {
        if line.trim() == "#[cfg(test)]"
            && lines
                .clone()
                .find(|next| !next.trim().is_empty())
                .is_some_and(|next| next.trim_start().starts_with("mod "))
        {
            return src.split_at(offset).0;
        }
        offset += line.len();
    }
    src
}

/// Production code of `src` without comment lines and with all whitespace
/// removed, so a pin matches the call and not its layout.
pub fn code_of(src: &str) -> String {
    production_text(src)
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .flat_map(str::split_whitespace)
        .collect()
}

/// The body of the first function whose header starts with `header` (for
/// example `"fnexecute("` in [`code_of`] text), braces matched. `None` when no
/// function starts with `header` or its braces are not balanced.
pub fn find_fn_body<'a>(code: &'a str, header: &str) -> Option<&'a str> {
    let start = code.find(header)?;
    let open = start + code.get(start..)?.find('{')?;
    let mut depth = 0;
    for (i, c) in code.get(open..)?.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return code.get(open..=open + i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_text_reads_past_a_test_item() {
        let src = "use a;\r\n\
                   #[cfg(test)]\r\n\
                   fn helper() {}\r\n\
                   fn shipped() {}\r\n\
                   #[cfg(test)]\r\n\
                   \r\n\
                   mod tests;\r\n";
        assert_eq!(
            production_text(src),
            "use a;\r\n#[cfg(test)]\r\nfn helper() {}\r\nfn shipped() {}\r\n",
            "the cut is the test module, so the code after a test item is kept"
        );
    }

    #[test]
    fn production_text_drops_the_test_module() {
        let src = "fn shipped() {}\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n";
        assert_eq!(
            production_text(src),
            "fn shipped() {}\n",
            "the inline test module is not production"
        );
        let no_tests = "fn shipped() {}\n#[cfg(unix)]\nmod unix;\n";
        assert_eq!(
            production_text(no_tests),
            no_tests,
            "a file without a test module is read whole"
        );
    }
}
