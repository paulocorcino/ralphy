//! A collected verb that answers with the CLI's own text (a Query or Mutate
//! verb) shows that text to the workbench cut apart by guesswork. A verb whose
//! reply carries a failure is in `MOVED_VERBS` and leaves the count (#685).
//! The count may only go down: a verb that moves lowers the baseline.

use super::*;

/// Query and Mutate verbs not yet in `MOVED_VERBS`, measured at the tip of #685.
const CLI_TEXT_VERBS: usize = 27;

/// The `Verb::X` names listed in the arms of `effect_class` that end in
/// `EffectClass::<class>`, for each class in `classes`.
fn class_verbs(src: &str, classes: &[&str]) -> BTreeSet<String> {
    let (_, body) = src
        .split_once("fn effect_class")
        .expect("effect_class exists");
    let arm = Regex::new(r"((?:\s*\|?\s*Verb::\w+)+)\s*=>\s*EffectClass::(\w+)").expect("fixed");
    let name = Regex::new(r"Verb::(\w+)").expect("fixed");
    let mut verbs = BTreeSet::new();
    for caps in arm.captures_iter(body) {
        if classes.contains(&&caps[2]) {
            verbs.extend(name.captures_iter(&caps[1]).map(|n| n[1].to_string()));
        }
    }
    verbs
}

/// The verbs named in the `MOVED_VERBS` list.
fn moved_verbs(src: &str) -> BTreeSet<String> {
    let line = Regex::new(r"const MOVED_VERBS[^=]*=\s*&\[([^\]]*)\]").expect("fixed");
    let name = Regex::new(r"Verb::(\w+)").expect("fixed");
    let list = line.captures(src).expect("MOVED_VERBS exists");
    name.captures_iter(&list[1])
        .map(|n| n[1].to_string())
        .collect()
}

#[test]
fn the_verb_counter_reads_the_arms_and_the_moved_list() {
    let dispatch =
        "fn effect_class(self) {\n match self {\n Verb::A\n | Verb::B => EffectClass::Query,\n \
                    Verb::C => EffectClass::Mutate,\n Verb::D => EffectClass::Write,\n }\n}";
    let failure = "const MOVED_VERBS: &[dispatch::Verb] = &[dispatch::Verb::C];";
    let counted = class_verbs(dispatch, &["Query", "Mutate"]);
    let moved = moved_verbs(failure);
    assert_eq!(counted.len(), 3);
    assert_eq!(counted.difference(&moved).count(), 2);
}

#[test]
fn failure_cli_text_verbs_match_the_baseline() {
    let root = workspace_root().join("crates/ralphy-daemon/src");
    let counted = class_verbs(&read(&root.join("dispatch.rs")), &["Query", "Mutate"]);
    let moved = moved_verbs(&read(&root.join("routes/ws_command/oneshot/failure.rs")));
    assert!(
        moved.is_subset(&counted),
        "MOVED_VERBS names a verb that is not Query or Mutate"
    );
    assert_eq!(
        counted.difference(&moved).count(),
        CLI_TEXT_VERBS,
        "a verb moved to a failure reply (lower CLI_TEXT_VERBS) or a new one was added (do not)"
    );
}
