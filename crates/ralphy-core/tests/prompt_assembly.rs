//! Anti-drift gate for the plan prompt variants (issues #71, #75).
//!
//! The five plan prompt artifacts (`prompt.plan.md`, `prompt.plan.codex.md`,
//! `prompt.plan.kimi.md`, `prompt.plan.opencode.md`, `prompt.plan.staged.md`) are
//! ASSEMBLED from one
//! canonical template plus a small per-variant overlay under
//! `assets/prompts/plan/`. The adapters keep embedding the assembled artifacts
//! via `include_str!` — this test re-runs the assembly and fails if any
//! artifact no longer matches template + overlay, i.e. if the shared prose was
//! edited in one artifact instead of the template.
//!
//! To change a prompt: edit `assets/prompts/plan/template.md` (shared prose) or
//! `assets/prompts/plan/overlay.<variant>.md` (variant block), then regenerate
//! the artifacts with:
//!
//! ```sh
//! RALPHY_REGEN_PROMPTS=1 cargo test -p ralphy-core --test prompt_assembly
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const SLOTS: [&str; 8] = [
    "execution-model",
    "self-review-step",
    "self-review-guidance",
    "ledger-example",
    "planning-mode-intro",
    "skill-invocation",
    "stages-section",
    "mode-rules",
];

const VARIANTS: [(&str, &str); 5] = [
    ("claude", "prompt.plan.md"),
    ("codex", "prompt.plan.codex.md"),
    ("kimi", "prompt.plan.kimi.md"),
    ("opencode", "prompt.plan.opencode.md"),
    ("staged", "prompt.plan.staged.md"),
];

fn prompts_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/prompts"))
}

/// Parse an overlay file into slot-name → verbatim content. Slots are delimited
/// by `<!-- slot: name -->` marker lines; content is everything (line endings
/// included) between one marker and the next. An empty slot (two adjacent
/// markers) is a valid, deliberately-absent vendor block.
fn parse_overlay(overlay: &str) -> BTreeMap<String, String> {
    let mut slots = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut buf = String::new();
    for line in overlay.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix("<!-- slot:")
            .and_then(|r| r.strip_suffix("-->"))
        {
            if let Some(prev) = current.take() {
                slots.insert(prev, std::mem::take(&mut buf));
            }
            current = Some(name.trim().to_string());
        } else if current.is_some() {
            buf.push_str(line);
        } else {
            panic!("overlay content before the first slot marker: {line:?}");
        }
    }
    if let Some(prev) = current {
        slots.insert(prev, buf);
    }
    slots
}

/// Substitute each `{{slot}}` placeholder line (the placeholder plus its own
/// line ending) with the overlay's verbatim content for that slot.
fn assemble(template: &str, slots: &BTreeMap<String, String>) -> String {
    let mut out = template.to_string();
    for name in SLOTS {
        let content = slots
            .get(name)
            .unwrap_or_else(|| panic!("overlay is missing slot {name:?}"));
        let crlf = format!("{{{{{name}}}}}\r\n");
        let lf = format!("{{{{{name}}}}}\n");
        if out.contains(&crlf) {
            out = out.replace(&crlf, content);
        } else if out.contains(&lf) {
            out = out.replace(&lf, content);
        } else {
            panic!("template has no {{{{{name}}}}} placeholder line");
        }
    }
    assert!(
        !out.contains("{{"),
        "assembled prompt still carries an unsubstituted placeholder"
    );
    out
}

/// The four checked-in plan prompt artifacts must be exactly what template +
/// overlay assemble to. A shared-prose edit made in one artifact (instead of in
/// `plan/template.md`) diverges from the assembly and fails here; so does an
/// edited template whose artifacts were not regenerated.
#[test]
fn plan_prompt_artifacts_match_template_plus_overlays() {
    let dir = prompts_dir();
    let template = fs::read_to_string(dir.join("plan/template.md"))
        .expect("assets/prompts/plan/template.md must exist");

    let regen = std::env::var_os("RALPHY_REGEN_PROMPTS").is_some();
    for (variant, artifact) in VARIANTS {
        let overlay_path = dir.join(format!("plan/overlay.{variant}.md"));
        let overlay = fs::read_to_string(&overlay_path)
            .unwrap_or_else(|e| panic!("{} must exist: {e}", overlay_path.display()));
        let slots = parse_overlay(&overlay);
        assert_eq!(
            slots.len(),
            SLOTS.len(),
            "overlay.{variant}.md must define exactly the {} known slots, found: {:?}",
            SLOTS.len(),
            slots.keys().collect::<Vec<_>>()
        );
        let assembled = assemble(&template, &slots);

        let artifact_path = dir.join(artifact);
        if regen {
            fs::write(&artifact_path, &assembled)
                .unwrap_or_else(|e| panic!("cannot write {}: {e}", artifact_path.display()));
            continue;
        }
        let on_disk = fs::read_to_string(&artifact_path)
            .unwrap_or_else(|e| panic!("{} must exist: {e}", artifact_path.display()));
        assert_eq!(
            assembled, on_disk,
            "{artifact} drifted from plan/template.md + plan/overlay.{variant}.md — \
             edit the template/overlay sources and regenerate with \
             `RALPHY_REGEN_PROMPTS=1 cargo test -p ralphy-core --test prompt_assembly` \
             (never edit the assembled artifact directly)"
        );
    }
}

/// Every plan artifact must carry the consolidated-spec authority rule (ADR-0017,
/// Part D): when a comment carries the `ralphy:consolidated-spec` marker it is the
/// authoritative spec over the body. Pins the contract into all four vendor
/// variants so no adapter's planner ranks the consolidation as secondary chatter.
#[test]
fn plan_prompt_names_consolidated_spec_marker() {
    let dir = prompts_dir();
    for (_, artifact) in VARIANTS {
        let text = fs::read_to_string(dir.join(artifact))
            .unwrap_or_else(|e| panic!("{artifact} must exist: {e}"));
        assert!(
            text.contains("ralphy:consolidated-spec"),
            "{artifact} must name the consolidated-spec marker"
        );
        assert!(
            text.contains("authoritative spec"),
            "{artifact} must state the marked comment is the authoritative spec"
        );
    }
}

/// The variant-specific surface is ONLY the named slots: every overlay must
/// define all of them and nothing else, so a new divergence cannot sneak in as
/// an extra ad-hoc slot without widening this list deliberately.
#[test]
fn overlays_define_exactly_the_known_slots() {
    let dir = prompts_dir();
    for (variant, _) in VARIANTS {
        let overlay = fs::read_to_string(dir.join(format!("plan/overlay.{variant}.md")))
            .expect("overlay must exist");
        let slots = parse_overlay(&overlay);
        let names: Vec<&str> = slots.keys().map(String::as_str).collect();
        let mut expected: Vec<&str> = SLOTS.to_vec();
        expected.sort_unstable();
        assert_eq!(
            names, expected,
            "overlay.{variant}.md slot set diverged from the canonical list"
        );
    }
}
