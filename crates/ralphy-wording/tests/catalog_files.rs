//! Every file of the wording catalog parses and names its own file stem as its domain.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

use std::path::PathBuf;

use ralphy_wording::DomainFile;

fn catalog_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/wording")
}

// pin: not a pin: it parses the catalog data —
// the classifier takes the path in an `expect` message for source text.
#[test]
fn every_catalog_file_parses_and_names_its_domain() {
    let entries = std::fs::read_dir(catalog_dir()).expect("assets/wording is readable");
    let mut seen = 0;
    for entry in entries {
        let path = entry.expect("a directory entry is readable").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let json = std::fs::read_to_string(&path).expect("a catalog file is readable");
        let file = DomainFile::parse(&json).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        assert_eq!(file.domain.as_str(), stem, "{}", path.display());
        seen += 1;
    }
    assert!(seen > 0, "assets/wording holds no catalog file");
}
