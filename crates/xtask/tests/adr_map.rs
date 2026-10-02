//! ADRs keep the format that docs/adr/TEMPLATE.md sets. From ADR-0068 on,
//! every decision file has a `Status:` and a `Kind:` line from the closed sets
//! of the template, and a structural ADR fills its Compliance section. Older
//! ADRs predate the template and are not read. Companion notes (a spike's
//! validation record) are not decisions and are not read either.
//!
//! docs/ARCHITECTURE.md is the map of the structural decisions: every ADR it
//! cites exists, and every structural ADR from 0068 on is cited there.

use std::collections::BTreeSet;
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
    let statuses: Vec<String> = template
        .lines()
        .skip_while(|line| !line.starts_with("Status — exactly one of:"))
        .nth(1)
        .expect("TEMPLATE.md lists the Status set on the line after its heading")
        .split('|')
        .map(|s| s.trim().replace("NNNN", "0001"))
        .collect();
    let kind_entry = Regex::new(r"^  ([a-z]+)  ").expect("the pattern is a valid regex literal");
    let kinds: Vec<String> = template
        .lines()
        .skip_while(|line| !line.starts_with("Kind — exactly one of:"))
        .take_while(|line| !line.starts_with("Protects"))
        .filter_map(|line| kind_entry.captures(line).map(|c| c[1].to_string()))
        .collect();
    assert_eq!(statuses.len(), 5, "{statuses:?}");
    assert_eq!(
        kinds,
        ["structural", "feature", "vendor", "process"],
        "the Kind set in TEMPLATE.md changed; update the Kind pattern in this test"
    );
    for status in &statuses {
        for kind in &kinds {
            let adr = format!("Status: {status}\nKind: {kind}\n## Compliance\n- D1: x\n");
            assert!(
                format_errors("t.md", &adr).is_empty(),
                "the template allows `{status}` / `{kind}`, the check refuses it"
            );
        }
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

    let no_heading = format_errors("h.md", "Status: proposed\nKind: structural\n");
    assert!(
        no_heading.len() == 1 && no_heading[0].contains("Compliance"),
        "{no_heading:#?}"
    );

    let long_comment = "Status: proposed\nKind: structural\n\n## Compliance\n\n<!--\n- D1: an example\n  inside the comment\n-->\n";
    let errors = format_errors("m.md", long_comment);
    assert!(
        errors.len() == 1 && errors[0].contains("Compliance"),
        "{errors:#?}"
    );
    let after_comment =
        "Status: proposed\nKind: structural\n\n## Compliance\n\n<!-- c --> - D1: x\n";
    assert!(format_errors("a.md", after_comment).is_empty());

    let missing = format_errors("n.md", "# T\n");
    assert!(
        missing.len() == 2 && missing[0].contains("no Status") && missing[1].contains("no Kind"),
        "{missing:#?}"
    );
}

#[test]
fn the_architecture_map_cites_real_adrs_and_every_structural_one() {
    let arch_path = adr_dir().join("../ARCHITECTURE.md");
    let arch = read(&arch_path);
    let errors = map_errors(&arch, &adr_files(&adr_dir()));
    assert!(
        errors.is_empty(),
        "docs/ARCHITECTURE.md is out of step with docs/adr:\n{}",
        errors.join("\n")
    );
}

#[test]
fn map_errors_catch_a_missing_and_an_uncited_adr() {
    let adrs = vec![
        (1, "0001-a.md".to_string(), "Kind: structural\n".to_string()),
        (
            70,
            "0070-b.md".to_string(),
            "Kind: structural\n".to_string(),
        ),
        (71, "0071-c.md".to_string(), "Kind: feature\n".to_string()),
        (
            72,
            "0072-d-validation.md".to_string(),
            "Kind: structural\n".to_string(),
        ),
    ];
    let errors = map_errors("see ADR-0001 and ADR-0999", &adrs);
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("0999")), "{errors:#?}");
    assert!(errors.iter().any(|e| e.contains("0070")), "{errors:#?}");

    let links = map_errors("ADR-0070 [a](adr/0070-b.md) [b](adr/0070-gone.md)", &adrs);
    assert!(
        links.len() == 1 && links[0].contains("0070-gone.md"),
        "{links:#?}"
    );
}

/// What the map gets wrong about the ADRs: a citation of an ADR that does not
/// exist, a link to a missing file, or a structural ADR from 0068 on that the
/// map does not cite.
fn map_errors(arch: &str, adrs: &[(u32, String, String)]) -> Vec<String> {
    let cite = Regex::new(r"ADR-(\d{4})").expect("the pattern is a valid regex literal");
    let link =
        Regex::new(r"adr/(\d{4}-[A-Za-z0-9-]+\.md)").expect("the pattern is a valid regex literal");
    let decisions: Vec<&(u32, String, String)> = adrs
        .iter()
        .filter(|(_, name, _)| !is_companion(name))
        .collect();

    let mut errors = Vec::new();
    let mut cited = BTreeSet::new();
    for c in cite.captures_iter(arch) {
        let number: u32 = c[1]
            .parse()
            .expect("four ASCII digits always parse as a u32");
        if cited.insert(number) && !decisions.iter().any(|(n, _, _)| *n == number) {
            errors.push(format!("cites ADR-{number:04}, which does not exist"));
        }
    }
    for c in link.captures_iter(arch) {
        let target = &c[1];
        if !adrs.iter().any(|(_, name, _)| name == target) {
            errors.push(format!("links adr/{target}, which does not exist"));
        }
    }
    for (number, _, text) in decisions {
        let structural = text
            .lines()
            .any(|line| line.trim_end() == "Kind: structural");
        if *number >= FIRST_TEMPLATED && structural && !cited.contains(number) {
            errors.push(format!("structural ADR-{number:04} is not cited"));
        }
    }
    errors
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
