//! The ast-grep exceptions (sgconfig.yml): each `ast-grep-ignore` names its
//! rule and says why, and the number of them only goes down. A bare one hides
//! every rule on its line, or the whole file when it is on the first line.

use super::*;

/// The `ast-grep-ignore` directives in the Rust sources. Lower it in the same
/// change that removes one: the count is exact, so it cannot grow back.
const AST_GREP_IGNORES: usize = 0;

/// `(file:line, names its rule and a reason)` of each directive: a comment
/// whose first word is `ast-grep-ignore`.
fn directives() -> Vec<(String, bool)> {
    let directive = Regex::new(r"(//[/!]?|#)\s*`?ast-grep-ignore`?").expect("a fixed pattern");
    let complete =
        Regex::new(r"ast-grep-ignore`?:\s*[\w,\s-]+?\s--\s+\S").expect("a fixed pattern");
    let mut files = Vec::new();
    skipped_tests::rust_sources(&workspace_root().join("crates"), &mut files);
    let mut found = Vec::new();
    for file in files {
        for (i, line) in read(&file).lines().enumerate() {
            if directive.is_match(line) {
                found.push((
                    format!("{}:{}", file.display(), i + 1),
                    complete.is_match(line),
                ));
            }
        }
    }
    found
}

#[test]
fn every_ast_grep_ignore_names_its_rule_and_why() {
    let bare: Vec<String> = directives()
        .into_iter()
        .filter(|(_, complete)| !complete)
        .map(|(at, _)| at)
        .collect();
    assert!(
        bare.is_empty(),
        "a directive names its rule and why \
         (`ast-grep-ignore: <rule-id> -- <why>`), found: {bare:?}"
    );
}

#[test]
fn ast_grep_ignores_do_not_grow() {
    let found = directives();
    assert_eq!(
        found.len(),
        AST_GREP_IGNORES,
        "the ast-grep exceptions changed; a new one is the last resort, and a \
         removed one lowers AST_GREP_IGNORES: {found:?}"
    );
}
