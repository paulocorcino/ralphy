use super::*;

#[test]
fn skills_are_materialized_into_the_owned_root() {
    let root = tempfile::tempdir().unwrap();
    let skills = root.path().join("skills");
    std::fs::create_dir_all(skills.join("stale")).unwrap();

    materialize(root.path()).unwrap();

    let reviewer = skills.join("reviewer").join("SKILL.md");
    assert!(
        std::fs::read_to_string(&reviewer).is_ok_and(|body| body.starts_with("---")),
        "{} must hold the skill",
        reviewer.display()
    );
    let mut embedded = 0;
    for skill in SKILLS.dirs() {
        let name = skill.path().file_name().unwrap();
        assert!(
            skills.join(name).join("SKILL.md").is_file(),
            "{name:?} missing"
        );
        embedded += 1;
    }
    assert!(embedded >= 3, "only {embedded} skills embedded");
    assert!(!skills.join("stale").exists(), "an old copy stays");
}
