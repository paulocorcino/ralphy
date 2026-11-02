//! Reads the string literals of a Rust fragment and decides which of them are pins.

/// Every string literal in a fragment that `keep` accepts as a pin, in source
/// order.
pub(super) fn string_literals(fragment: &str) -> Vec<String> {
    let mut out = Vec::new();
    for lit in raw_literals(fragment) {
        keep(&mut out, lit);
    }
    out
}

/// Every non-empty string literal of a Rust fragment, with no pin filtering.
///
/// Rust has two spellings and they need different scanners. `"…"` ends at the
/// first unescaped quote; `r#"…"#` ends at the matching `"#` and treats an
/// embedded `"` as content. Scanning a raw string as a plain one cuts it at its
/// first inner quote — `r#"class="fence-item""#` was recorded as the pin
/// `class=` — and 69 raw-string elements sit in this file's pin tables, so the
/// text this tool prints was wrong for every one of them.
pub(super) fn raw_literals(fragment: &str) -> Vec<String> {
    let bytes: Vec<char> = fragment.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let Some((lit, next)) = literal_at(&bytes, i) else {
            i += 1;
            continue;
        };
        if !lit.is_empty() {
            out.push(lit);
        }
        i = next;
    }
    out
}

/// The string literal that opens at `at`, raw or plain: its body, and the
/// index after it. `None` when no literal opens there, which includes an `r`
/// that opens no raw string.
fn literal_at(bytes: &[char], at: usize) -> Option<(String, usize)> {
    match bytes[at] {
        'r' => raw_string(bytes, at),
        '"' => Some(plain_string(bytes, at)),
        _ => None,
    }
}

/// A raw string that opens at the `r` at `at`: its body, and the index after
/// its closing quote and hashes. `None` when `r` opens no raw string or the
/// string never closes.
fn raw_string(bytes: &[char], at: usize) -> Option<(String, usize)> {
    // A raw string opens with `r`, any number of `#`, then a quote, and
    // closes with a quote followed by the SAME number of `#`.
    let (hashes, quote) = raw_opener(bytes, at)?;
    let close: String = std::iter::once('"')
        .chain(std::iter::repeat_n('#', hashes))
        .collect();
    let rest: String = bytes[quote + 1..].iter().collect();
    let end = rest.find(&close)?;
    Some((rest[..end].to_string(), quote + 1 + end + close.len()))
}

/// The number of `#` after the `r` at `at`, and the index of the quote that
/// follows them. `None` when no quote follows.
fn raw_opener(bytes: &[char], at: usize) -> Option<(usize, usize)> {
    let mut hashes = 0;
    let mut j = at + 1;
    while j < bytes.len() && bytes[j] == '#' {
        hashes += 1;
        j += 1;
    }
    (j < bytes.len() && bytes[j] == '"').then_some((hashes, j))
}

/// A plain string whose opening quote is at `open`: its text with the escapes
/// dropped, and the index after the closing quote (or the end of input).
fn plain_string(bytes: &[char], open: usize) -> (String, usize) {
    let mut lit = String::new();
    let mut escaped = false;
    let mut i = open + 1;
    while i < bytes.len() {
        let c = bytes[i];
        i += 1;
        if escaped {
            lit.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => break,
            _ => lit.push(c),
        }
    }
    (lit, i)
}

/// Record a literal if it is a PIN rather than an assertion message or a path.
///
/// The word-count heuristic this replaced dropped real pins — eight of them in
/// this repo, including `this.changesError = msg || "";` and
/// `replaying = connOpts.id != null` — because a fragment of JS has spaces in
/// it exactly like a sentence does. Length is the honest discriminator: a
/// failure message in this codebase is a wrapped sentence, a pin is a fragment.
/// It is still a heuristic, and the count is an estimate of the contract by
/// shape rather than an audit of individual strings.
pub(super) fn keep(out: &mut Vec<String>, lit: String) {
    if lit.is_empty() || lit.len() >= 120 {
        return;
    }
    // An `include_str!` argument is how a test NAMES an asset, not a claim about
    // one. Counting it added nine phantom claims, three of them from a test this
    // very branch added.
    if lit.starts_with("../") && lit.trim_start_matches("../").starts_with("assets/ui/") {
        return;
    }
    // A bare format placeholder — `{name}`, `{path}` — is the message's, not a
    // claim about asset text.
    if lit.starts_with('{') && lit.ends_with('}') && !lit.contains(' ') {
        return;
    }
    // Prose: a message long enough to wrap, with no code punctuation in it.
    let wordy = lit.split_whitespace().count() >= 5;
    // Deliberately NOT '.' or '#': a failure message routinely names a file
    // ("index.html must link both favicon forms") or an issue, and those are
    // prose. These five are punctuation prose does not carry.
    let codey = lit.contains(['(', '{', '=', ';', '<', '[', ':']);
    if wordy && !codey {
        return;
    }
    out.push(lit);
}

pub(super) fn squeeze(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(96)
        .collect()
}
