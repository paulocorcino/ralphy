use super::*;

const PLAN_WITH_BOTH: &str = "\
# Plan for #2

## Steps
- [x] done

## Handoff

- **Delivered**: fixtures + Setup-Lab.ps1 (abc1234)
- **Environment facts & traps**: image 2ede8a0e does not process INVENTORY out of the box
- **Commands that work**: docker compose up -d; curl -I http://localhost:8080
- **Residue**: Setup-Lab.ps1 never ran clean-slate; run `docker compose down -v` then the script

## Plan friction

- the plan treated the lab as a given precondition; it was 70% of the work

## Notes & decisions
some note
";

#[test]
fn close_report_carries_handoff_and_friction() {
    let report = close_report(PLAN_WITH_BOTH).expect("report present");
    assert!(report.starts_with("## Handoff"));
    assert!(report.contains("Setup-Lab.ps1 never ran clean-slate"));
    assert!(report.contains("## Plan friction"));
    assert!(report.contains("70% of the work"));
    // Neighbouring sections must not leak in.
    assert!(!report.contains("Notes & decisions"));
    assert!(!report.contains("- [x] done"));
}

#[test]
fn close_report_with_only_friction_keeps_friction_heading() {
    let md = "# Plan\n\n## Plan friction\n- none\n";
    let report = close_report(md).expect("report present");
    assert!(report.starts_with("## Plan friction"));
    assert!(report.contains("- none"));
}

#[test]
fn close_report_absent_sections_is_none() {
    assert_eq!(close_report("# Plan\n\n## Steps\n- [x] x\n"), None);
}

#[test]
fn close_report_blank_sections_is_none() {
    assert_eq!(
        close_report("# Plan\n\n## Handoff\n\n\n## Steps\n- [x] x\n"),
        None
    );
}

#[test]
fn infeasible_reason_reads_feasible_section_with_verdict_tail() {
    let md = "# Plan for #3\n\n## Feasible: no\nThe issue bundles six PRD tasks; split into \
        W1-T01..T06.\n\n## Steps\n";
    let reason = infeasible_reason(md).expect("reason present");
    assert!(reason.contains("bundles six PRD tasks"));
}

#[test]
fn infeasible_reason_absent_is_none() {
    assert_eq!(infeasible_reason("# Plan\n\n## Steps\n"), None);
}

#[test]
fn is_bundle_reason_detects_the_word_case_insensitively() {
    assert!(is_bundle_reason(
        "This issue is a **bundle**: six PRD tasks map to this number."
    ));
    assert!(is_bundle_reason("The issue is a Bundle of W2-T01..T05."));
    assert!(!is_bundle_reason(
        "No acceptance criteria and no verifiable done condition."
    ));
}

#[test]
fn is_bundle_verdict_reads_the_heading_before_the_prose() {
    // A feasible verdict whose prose denies being a bundle.
    let yes = "# Plan\n\n## Feasible: yes\nNot a bundle: diagnosis §11 maps this work to #511 \
        alone.\n\n## Steps\n- [x] a\n";
    assert!(!is_bundle_verdict(yes));
    let no = "# Plan\n\n## Feasible: no\nThe issue bundles six PRD tasks; split into \
        W1-T01..T06.\n\n## Steps\n";
    assert!(is_bundle_verdict(no));
    let generic = "# Plan\n\n## Feasible: no\nNo verifiable done condition.\n\n## Steps\n";
    assert!(!is_bundle_verdict(generic));
}

#[test]
fn find_handoff_comment_picks_last_match() {
    let comments = vec![
        "## Acceptance ledger\n- **[verified]** x".to_owned(),
        "## Handoff\n\n- **Delivered**: v1".to_owned(),
        "just chatter".to_owned(),
        "## Handoff\n\n- **Delivered**: v2 (re-run)".to_owned(),
    ];
    let found = find_handoff_comment(&comments).expect("handoff present");
    assert!(found.contains("v2 (re-run)"));
}

#[test]
fn find_handoff_comment_none_when_absent() {
    let comments = vec!["## Acceptance ledger\nstuff".to_owned()];
    assert_eq!(find_handoff_comment(&comments), None);
}

#[test]
fn render_handoffs_file_lists_each_dependency() {
    let entries = vec![
        (2u64, "## Handoff\n\n- **Delivered**: lab".to_owned()),
        (5u64, "## Handoff\n\n- **Delivered**: schema".to_owned()),
    ];
    let file = render_handoffs_file(&entries).expect("file content");
    assert!(file.contains("## From #2"));
    assert!(file.contains("## From #5"));
}

#[test]
fn render_handoffs_file_empty_is_none() {
    assert_eq!(render_handoffs_file(&[]), None);
}

#[test]
fn knowledge_note_extracts_env_facts_and_commands_only() {
    let note = knowledge_note(PLAN_WITH_BOTH).expect("note present");
    assert!(note.contains("**Environment facts & traps**"));
    assert!(note.contains("does not process INVENTORY"));
    assert!(note.contains("**Commands that work**"));
    assert!(note.contains("docker compose up -d"));
    // Issue-specific entries stay out.
    assert!(!note.contains("**Delivered**"));
    assert!(!note.contains("**Residue**"));
    // Neighbouring sections must not leak in.
    assert!(!note.contains("Plan friction"));
}

#[test]
fn knowledge_note_keeps_sub_lines_of_a_block() {
    let md = "\
## Handoff

- **Delivered**: stuff (abc1234)
- **Environment facts & traps**:
  - proxy strips the TAG header; pass it via query instead
  - schema rejects empty DEVICEID
- **Residue**: none
";
    let note = knowledge_note(md).expect("note present");
    assert!(note.contains("proxy strips the TAG header"));
    assert!(note.contains("schema rejects empty DEVICEID"));
    assert!(!note.contains("**Residue**"));
}

#[test]
fn knowledge_note_accepts_standalone_bold_headings() {
    // The shape a real executor produced: sub-headings as standalone bold
    // lines (no `- ` bullet), sub-bullets and code fences underneath.
    let md = "\
## Handoff

**Delivered**:
- `go.mod` + `go.sum`: module scaffold (commit 3fcf415)

**Environment facts & traps**:
- Host port 8080 is occupied by Docker Desktop's own backend. Always use 8088.
- CGO_ENABLED=0 + GOOS=linux is mandatory; alpine cannot run a CGO binary.

**Commands that work**:
```
docker compose build server && docker compose up -d server
curl http://localhost:8088/
```

**Residue**:
- base image not digest-pinned

## Plan friction

- the plan specified port 8080; Docker Desktop claimed it
";
    let note = knowledge_note(md).expect("note present");
    assert!(note.contains("Host port 8080 is occupied"));
    assert!(note.contains("CGO_ENABLED=0"));
    assert!(note.contains("docker compose build server"));
    assert!(note.contains("curl http://localhost:8088/"));
    assert!(!note.contains("**Delivered**"));
    assert!(!note.contains("**Residue**"));
    assert!(!note.contains("digest-pinned"));
    assert!(!note.contains("Plan friction"));
}

#[test]
fn knowledge_note_none_without_durable_entries() {
    let md = "## Handoff\n\n- **Delivered**: docs only\n- **Residue**: none\n";
    assert_eq!(knowledge_note(md), None);
    assert_eq!(knowledge_note("# Plan\n\n## Steps\n- [x] x\n"), None);
}

#[test]
fn knowledge_used_reads_inline_tail_of_bullet_form() {
    let md = "\
## Handoff

- **Delivered**: the fix (abc1234)
- **Knowledge used**: \"Host port 8080 is occupied\" bullet from KNOWLEDGE.md
";
    assert_eq!(
        knowledge_used(md),
        Some(vec![
            "\"Host port 8080 is occupied\" bullet from KNOWLEDGE.md".to_owned()
        ])
    );
}

#[test]
fn knowledge_used_reads_sub_bullets_of_standalone_bold_form() {
    let md = "\
## Handoff

**Delivered**:
- the fix (abc1234)

**Knowledge used**:
- \"Toolchain & platform\" — cargo test needs docker up first
- handoffs.md #5: schema rejects empty DEVICEID

## Plan friction

- none
";
    assert_eq!(
        knowledge_used(md),
        Some(vec![
            "\"Toolchain & platform\" — cargo test needs docker up first".to_owned(),
            "handoffs.md #5: schema rejects empty DEVICEID".to_owned(),
        ])
    );
}

#[test]
fn knowledge_used_explicit_none_is_empty_list() {
    let md = "## Handoff\n\n- **Knowledge used**: none\n";
    assert_eq!(knowledge_used(md), Some(Vec::new()));
    let backticked = "## Handoff\n\n- **Knowledge used**: `none`\n";
    assert_eq!(knowledge_used(backticked), Some(Vec::new()));
}

#[test]
fn knowledge_used_absent_key_or_handoff_is_none() {
    // PLAN_WITH_BOTH predates the field — the caller warns on this.
    assert_eq!(knowledge_used(PLAN_WITH_BOTH), None);
    assert_eq!(knowledge_used("# Plan\n\n## Steps\n- [x] x\n"), None);
}

#[test]
fn knowledge_used_malformed_degrades_to_empty_list() {
    // Key present but no inline tail and no sub-bullets: tolerate, don't
    // error — the signal for this close is simply "nothing cited".
    let md = "## Handoff\n\n- **Knowledge used**\n- **Residue**: none\n";
    assert_eq!(knowledge_used(md), Some(Vec::new()));
}

#[test]
fn knowledge_note_does_not_leak_a_following_knowledge_used_block() {
    // Pins the HANDOFF_KEYS addition: before it, a trailing `Knowledge
    // used` block was swallowed by whichever known block preceded it.
    let md = "\
## Handoff

- **Commands that work**: docker compose up -d
- **Knowledge used**: \"proxy strips the TAG header\" bullet
";
    let note = knowledge_note(md).expect("note present");
    assert!(note.contains("docker compose up -d"));
    assert!(!note.contains("Knowledge used"));
    assert!(!note.contains("proxy strips the TAG header"));
}
