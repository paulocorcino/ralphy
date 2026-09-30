//! ADRs keep the format that docs/adr/TEMPLATE.md sets. From ADR-0068 on,
//! every decision file has a `Status:` and a `Kind:` line from the closed sets
//! of the template, and a structural ADR fills its Compliance section. Older
//! ADRs predate the template and are not read. Companion notes (a spike's
//! validation record) are not decisions and are not read either.

use std::path::{Path, PathBuf};

use regex::Regex;

/// The first ADR number the template applies to.
const FIRST_TEMPLATED: u32 = 68;

#[test]
fn new_adrs_have_a_closed_status_and_kind() {
    let adrs = adr_files(&adr_dir());
    assert!(
        adrs.iter().any(|(number, _, _)| *number >= FIRST_TEMPLATED),
        "expected ADRs from {FIRST_TEMPLATED:04} on in {}",
        adr_dir().display()
    );
    let errors: Vec<String> = adrs
        .iter()
        .filter(|(number, name, _)| *number >= FIRST_TEMPLATED && !is_companion(name))
        .flat_map(|(_, name, text)| format_errors(name, text))
        .collect();
    assert!(
        errors.is_empty(),
        "ADRs that do not follow docs/adr/TEMPLATE.md:\n{}",
        errors.join("\n")
    );
}

#[test]
fn the_closed_sets_match_the_template() {
    let template = read(&adr_dir().join("TEMPLATE.md"));
    assert!(
        template.lines().any(|line| line.trim()
            == "proposed | accepted | deferred | rejected | superseded by ADR-NNNN"),
        "the Status set in TEMPLATE.md changed; update the Status pattern in this test"
    );
    for kind in ["structural", "feature", "vendor", "process"] {
        assert!(
            template
                .lines()
                .any(|line| line.starts_with(&format!("  {kind}  "))),
            "the Kind `{kind}` is no longer in TEMPLATE.md; update the Kind pattern in this test"
        );
    }
}

#[test]
fn format_errors_catch_a_bad_status_kind_and_an_empty_compliance() {
    let good = "# T\n\nStatus: accepted\nKind: structural\n\n## Compliance\n\n- D1: checked.\n";
    assert!(format_errors("good.md", good).is_empty());

    let status = format_errors("s.md", "Status: accepted (x)\nKind: feature\n");
    assert!(
        status.len() == 1 && status[0].contains("Status"),
        "{status:#?}"
    );

    let kind = format_errors("k.md", "Status: proposed\nKind: structure\n");
    assert!(kind.len() == 1 && kind[0].contains("Kind"), "{kind:#?}");

    let empty = "Status: proposed\nKind: structural\n\n## Compliance\n\n<!-- c -->\n\n## Amendment\n\n- D1: x\n";
    let errors = format_errors("c.md", empty);
    assert!(
        errors.len() == 1 && errors[0].contains("Compliance"),
        "{errors:#?}"
    );

    let superseded = "Status: superseded by ADR-0070\nKind: process\n";
    assert!(format_errors("x.md", superseded).is_empty());
}

/// What is wrong with the Status, Kind and Compliance of one ADR.
fn format_errors(name: &str, text: &str) -> Vec<String> {
    let status =
        Regex::new(r"^Status: (proposed|accepted|deferred|rejected|superseded by ADR-\d{4})$")
            .expect("the pattern is a valid regex literal");
    let kind = Regex::new(r"^Kind: (structural|feature|vendor|process)$")
        .expect("the pattern is a valid regex literal");
    let first = |prefix: &str| text.lines().find(|line| line.starts_with(prefix));

    let mut errors = Vec::new();
    match first("Status:") {
        Some(line) if status.is_match(line.trim_end()) => {}
        Some(line) => errors.push(format!(
            "{name}: `{line}` is not a Status from the template"
        )),
        None => errors.push(format!("{name}: no Status line")),
    }
    match first("Kind:") {
        Some(line) if kind.is_match(line.trim_end()) => {
            if line.trim_end() == "Kind: structural" && !has_compliance(text) {
                errors.push(format!(
                    "{name}: a structural ADR needs a non-empty ## Compliance section"
                ));
            }
        }
        Some(line) => errors.push(format!("{name}: `{line}` is not a Kind from the template")),
        None => errors.push(format!("{name}: no Kind line")),
    }
    errors
}

/// True when `## Compliance` has a line that is not blank and not inside an
/// HTML comment, before the next `## ` heading.
fn has_compliance(text: &str) -> bool {
    let mut lines = text
        .lines()
        .skip_while(|line| line.trim_end() != "## Compliance");
    if lines.next().is_none() {
        return false;
    }
    let mut in_comment = false;
    for line in lines {
        if line.starts_with("## ") {
            return false;
        }
        let mut rest = line.trim();
        while !rest.is_empty() {
            if in_comment {
                match rest.find("-->") {
                    Some(end) => {
                        rest = rest[end + 3..].trim();
                        in_comment = false;
                    }
                    None => rest = "",
                }
            } else {
                match rest.find("<!--") {
                    Some(0) => {
                        rest = &rest[4..];
                        in_comment = true;
                    }
                    Some(_) | None => return true,
                }
            }
        }
    }
    false
}

/// `(number, file name, text)` of each `NNNN-*.md` file in `dir`.
fn adr_files(dir: &Path) -> Vec<(u32, String, String)> {
    let numbered = Regex::new(r"^(\d{4})-.*\.md$").expect("the pattern is a valid regex literal");
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    let mut out = Vec::new();
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", dir.display()))
            .path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if let Some(c) = numbered.captures(&name) {
            let number = c[1]
                .parse()
                .expect("four ASCII digits always parse as a u32");
            out.push((number, name, read(&path)));
        }
    }
    out.sort();
    out
}

/// A spike's validation record that sits next to its ADR (docs/adr/README.md).
fn is_companion(name: &str) -> bool {
    name.ends_with("-validation.md") || name.ends_with("-revalidation.md")
}

fn adr_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/adr")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}
