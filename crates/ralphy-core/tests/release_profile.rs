use std::fs;

#[test]
fn workspace_cargo_toml_has_release_profile_keys() {
    let cargo_toml_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    let content =
        fs::read_to_string(cargo_toml_path).expect("workspace Cargo.toml should be readable");

    assert!(
        content.contains("strip = true"),
        "[profile.release] must contain `strip = true`"
    );
    assert!(
        content.contains(r#"lto = "thin""#),
        "[profile.release] must contain `lto = \"thin\"`"
    );
    assert!(
        content.contains("codegen-units = 1"),
        "[profile.release] must contain `codegen-units = 1`"
    );
}
