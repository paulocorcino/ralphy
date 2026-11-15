//! The workbench `CAUSE` table in `wb-fail.ts` maps a reason code to its words.
//! A row is a code whose text the daemon has not moved into the catalog yet
//! (#685). The count may only go down: a row that moves lowers the baseline.

use super::*;

/// The rows of `CAUSE`, measured at the integrated tip of #685.
const CAUSE_ROWS: usize = 23;

/// The `key: "text"` rows of `const CAUSE`, read through the `ui-copy` lexer
/// so a comment or a string that looks like a row is not counted.
fn cause_rows(src: &str) -> usize {
    let toks = lex::lex(src, 1);
    let rest = toks
        .iter()
        .skip_while(|t| !matches!(&t.tok, lex::Tok::Ident(n) if n == "CAUSE"))
        .skip_while(|t| !matches!(&t.tok, lex::Tok::Punct("{")))
        .skip(1)
        .take_while(|t| !matches!(&t.tok, lex::Tok::Punct("}")))
        .map(|t| &t.tok);
    let mut count = 0;
    let (mut key, mut colon) = (false, false);
    for tok in rest {
        match tok {
            lex::Tok::Str(_) if colon => {
                count += 1;
                (key, colon) = (false, false);
            }
            lex::Tok::Ident(_) | lex::Tok::Str(_) => {
                (key, colon) = (true, false);
            }
            lex::Tok::Punct(":") if key => colon = true,
            lex::Tok::Punct(_) | lex::Tok::Tpl(_) | lex::Tok::Other => {
                (key, colon) = (false, false);
            }
        }
    }
    count
}

#[test]
fn the_cause_counter_reads_only_the_rows_of_the_table() {
    let src = concat!(
        "const OWN = { transport: \"x\" };
// a: \"b\"
",
        "const CAUSE: Record<string, string> = {
  exists: \"a\",
  \"not found\": \"b\",
};
",
        "const AFTER = { z: \"c\" };
"
    );
    assert_eq!(cause_rows(src), 2);
}

#[test]
fn failure_cause_rows_match_the_baseline() {
    let path = workspace_root().join(UI_DIR).join("wb-fail.ts");
    assert_eq!(
        cause_rows(&read(&path)),
        CAUSE_ROWS,
        "a row moved into the catalog (lower CAUSE_ROWS) or a new one was added (do not)"
    );
}
