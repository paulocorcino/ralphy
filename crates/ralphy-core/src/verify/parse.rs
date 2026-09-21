//! The `## Verify` plan section, read into a [`VerifySpec`] (ADR-0011).

use regex::Regex;

/// The parsed `## Verify` plan section (ADR-0011).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifySpec {
    /// `none` on its own line — the planner judged nothing is machine-verifiable.
    /// The only explicit opt-out; it skips the settings fallback.
    None,
    /// One or more commands, each tokenized into an argv.
    Commands(Vec<Vec<String>>),
    /// The section is present but malformed — a markdown checklist masquerading as
    /// commands (a leading `-`/`*`/`+` bullet, `- [ ]` checkbox, or backtick-wrapped
    /// command), or a backslash-escaped quote the tokenizer cannot honor (`\"`/`\'`).
    /// Tokenizing the former would spawn a bogus `-`/`` ` `` program (#181); the latter
    /// mis-splits into a garbage argv the gate spawn-fails on (#268). Either way the
    /// real command would never run, so it resolves to this arm instead. Carries a
    /// clear, operator-facing error naming the malformed line(s).
    Invalid(String),
    /// Section absent or present-but-empty — a planner omission. The runner falls
    /// back to `settings.json` `verify.command`.
    Unspecified,
}

/// Parse the `## Verify` section of a plan markdown into a [`VerifySpec`].
///
/// One command per line, code-fence-tolerant (` ``` ` lines are ignored), with
/// quote-aware argv tokenization so `sh -c "cargo test"` survives as three
/// tokens. A lone `none` line (case-insensitive) is the explicit opt-out. An
/// absent or whitespace-only section is [`VerifySpec::Unspecified`]. A section
/// authored as a markdown checklist (a leading bullet, checkbox, or
/// backtick-wrapped command) is rejected as [`VerifySpec::Invalid`] rather than
/// tokenized into a bogus `-`/`` ` `` program that spawn-fails (#181), as is a line
/// with a backslash-escaped quote (`\"`/`\'`) the tokenizer cannot honor (#268).
pub fn parse_verify(md: &str) -> VerifySpec {
    let heading_re = Regex::new(r"(?im)^##\s+Verify\s*$").expect("valid regex");
    let section = crate::markdown::section_after_heading(md, &heading_re);

    let lines: Vec<&str> = section
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("```"))
        .collect();

    if lines.is_empty() {
        return VerifySpec::Unspecified;
    }
    // `none` is only an opt-out when it stands alone — a `none` among real
    // commands is tokenized like any other line (and would simply fail to run).
    if lines.len() == 1 && lines[0].eq_ignore_ascii_case("none") {
        return VerifySpec::None;
    }

    // Reject a markdown checklist before tokenizing: a leading `-`/`*`/`+` bullet,
    // `- [ ]` checkbox, or backtick-wrapped command would tokenize its marker into a
    // bogus `-`/`` ` `` `argv[0]` the gate spawn-fails on, so the real command never
    // runs (#181). Enforces ADR-0011's "one bare command per line" contract.
    let malformed: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| looks_like_markdown_list(l))
        .collect();
    if !malformed.is_empty() {
        return VerifySpec::Invalid(malformed_error(&malformed));
    }

    // Reject a backslash-escaped quote before tokenizing: [`tokenize`] does not honor
    // `\"`/`\'`, so a nested escaped quote (`sh -c "test \"$x\" = y"`) is mis-split
    // into a garbage argv the gate spawn-runs, failing with a confusing shell syntax
    // error that also spends the repair budget (#268). Catch it here with a clear,
    // actionable message rather than let it fail opaquely at runtime.
    let escaped: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| has_escaped_quote(l))
        .collect();
    if !escaped.is_empty() {
        return VerifySpec::Invalid(escaped_quote_error(&escaped));
    }

    let commands: Vec<Vec<String>> = lines
        .iter()
        .map(|l| tokenize(l))
        .filter(|argv| !argv.is_empty())
        .collect();

    if commands.is_empty() {
        VerifySpec::Unspecified
    } else {
        VerifySpec::Commands(commands)
    }
}

/// Whether a `## Verify` line is markdown prose rather than a bare command: a
/// leading list bullet (`-`/`*`/`+`), which also covers a `- [ ]` checkbox, or a
/// leading backtick (a backtick-wrapped command). A real command never begins with
/// any of these — `argv[0]` is a program name, not a flag or a fence — so their
/// presence is an unambiguous authoring mistake (#181).
fn looks_like_markdown_list(line: &str) -> bool {
    matches!(line.chars().next(), Some('-' | '*' | '+' | '`'))
}

/// The operator-facing error for a malformed `## Verify` section: state the
/// contract and quote each offending line so the plan author can find and fix it.
fn malformed_error(malformed: &[&str]) -> String {
    let offenders = malformed
        .iter()
        .map(|l| format!("`{l}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "`## Verify` must be one bare command per line, not a markdown list \
         (no `-`/`*`/`+` bullet, `- [ ]` checkbox, or backtick-wrapped command). \
         Offending line(s): {offenders}"
    )
}

/// Whether a `## Verify` line contains a backslash-escaped quote that [`tokenize`]
/// will misread (#268). The tokenizer does not honor backslash escapes, so a `\"`
/// inside a double-quoted region — or a `\'` inside a single-quoted region, or
/// either outside quotes — is taken as a real quote toggle, splitting the line into
/// a garbage argv. Walking tokenize's own quote state machine, this flags the first
/// backslash that escapes a quote the tokenizer would otherwise toggle. A backslash
/// before any non-quote char (a Windows path `C:\foo`) or an escaped backslash
/// (`\\`) is left alone, so real commands are not false-flagged.
fn has_escaped_quote(line: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_backslash = false;
    for ch in line.chars() {
        if prev_backslash {
            prev_backslash = false;
            // A quote the tokenizer would toggle, reached via `\`: the author meant
            // an escaped literal quote the tokenizer cannot honor.
            if (ch == '"' && !in_single) || (ch == '\'' && !in_double) {
                return true;
            }
            // The char was escaped (a literal — including a literal `\`): it neither
            // toggles a quote nor starts a new escape.
            continue;
        }
        match ch {
            '\\' => prev_backslash = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            _ => {}
        }
    }
    false
}

/// The operator-facing error for a `## Verify` line whose backslash-escaped quote the
/// tokenizer cannot honor (#268): the gate runs argv with no shell, so name the
/// offending line(s) and point at the two shapes that DO tokenize cleanly — single
/// outer quotes or a single bare command (e.g. a `python -c` one-liner).
fn escaped_quote_error(offenders: &[&str]) -> String {
    let list = offenders
        .iter()
        .map(|l| format!("`{l}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "`## Verify` runs each command as argv with no shell, and its tokenizer does \
         not honor backslash-escaped quotes (`\\\"`/`\\'`). Rewrite with single outer \
         quotes (e.g. `sh -c 'test \"$x\" = y'`) or as one bare command per line (e.g. \
         a `python -c \"...\"` one-liner). Offending line(s): {list}"
    )
}

/// Split one command line into argv tokens, honoring single and double quotes so
/// an argument with spaces (`sh -c "cargo test"`) stays one token. Whitespace
/// outside quotes separates tokens; an unterminated quote closes at end of line
/// (best-effort — the parser never fails). No shell metacharacter handling: this
/// is argv tokenization, not a shell.
pub fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut has_token = false;

    for ch in line.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                has_token = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                has_token = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if has_token {
                    tokens.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(cur);
    }
    tokens
}

#[cfg(test)]
mod tests;
