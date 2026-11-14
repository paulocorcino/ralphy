//! The rule for a relative import path of a workbench module, as the served
//! tree names it. It holds no swc type, so `src/tests.rs` compiles it too.

/// Where a workbench module finds a wording catalog in the source tree.
const CATALOG_SOURCE: &str = "../../../../assets/wording/";
/// Where `build.rs` copies the catalogs in the served tree.
const CATALOG_SERVED: &str = "./wording/";

/// The served path of the import `path`, or `None` when the path stays as it
/// is. `json` says the import carries `with { type: "json" }`. A relative path
/// names a `.ts` file, a `.js` file under `./vendor/`, or a wording catalog
/// (ADR-0075 D2, D5; ADR-0078 D9).
pub(crate) fn served_import(path: &str, json: bool) -> Result<Option<String>, String> {
    if !(path.starts_with("./") || path.starts_with("../")) {
        return Ok(None);
    }
    if let Some(file) = path
        .strip_prefix(CATALOG_SOURCE)
        .filter(|f| is_catalog_file(f))
    {
        if !json {
            return Err(format!(
                "the import \"{path}\" must carry with {{ type: \"json\" }}"
            ));
        }
        return Ok(Some(format!("{CATALOG_SERVED}{file}")));
    }
    if json || has_extension(path, "json") {
        return Err(format!(
            "the import \"{path}\" is JSON, and a module imports JSON only from \
             {CATALOG_SOURCE}<domain>.json"
        ));
    }
    if path.starts_with("./vendor/") && has_extension(path, "js") {
        return Ok(None);
    }
    match path.strip_suffix(".ts") {
        Some(stem) => Ok(Some(format!("{stem}.js"))),
        None => Err(format!(
            "the import \"{path}\" must name a .ts file or a .js file under ./vendor/"
        )),
    }
}

/// Whether the file of `path` ends in `.<ext>`, in any letter case.
fn has_extension(path: &str, ext: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// A file name `<domain>.json`, with no folder in it.
fn is_catalog_file(file: &str) -> bool {
    file.strip_suffix(".json")
        .is_some_and(|domain| !domain.is_empty() && !domain.contains(['/', '\\', '.']))
}

#[cfg(test)]
mod tests {
    use super::served_import;

    #[test]
    fn a_catalog_import_is_served_from_the_wording_folder() {
        assert_eq!(
            served_import("../../../../assets/wording/run.json", true),
            Ok(Some("./wording/run.json".to_owned()))
        );
    }

    #[test]
    fn a_catalog_import_without_the_json_attribute_is_refused() {
        assert!(served_import("../../../../assets/wording/run.json", false).is_err());
    }

    #[test]
    fn a_json_import_outside_the_wording_folder_is_refused() {
        for path in [
            "./data.json",
            "../../../../assets/other/run.json",
            "../../../../assets/wording/deep/run.json",
        ] {
            assert!(served_import(path, true).is_err(), "{path} must be refused");
            assert!(
                served_import(path, false).is_err(),
                "{path} must be refused"
            );
        }
    }

    #[test]
    fn a_module_import_keeps_its_rule() {
        assert_eq!(
            served_import("./wb-api.ts", false),
            Ok(Some("./wb-api.js".to_owned()))
        );
        assert_eq!(served_import("./vendor/alpine.js", false), Ok(None));
        assert_eq!(served_import("alpinejs", false), Ok(None));
        assert!(served_import("./wb-api.js", false).is_err());
    }
}
