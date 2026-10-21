//! The vendored libraries match their manifest.

use super::*;

/// Every problem `vendor/manifest.json` has against `files` (path relative to
/// `vendor/`, embedded bytes). Empty when each file has exactly one entry
/// whose SHA-256 matches, and each entry names a file that exists.
fn check_manifest(files: &[(String, Vec<u8>)], manifest: &serde_json::Value) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let mut problems = Vec::new();
    let mut entries: std::collections::BTreeMap<String, String> = Default::default();
    let libraries = manifest["libraries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if libraries.is_empty() {
        problems.push("the manifest lists no libraries".to_string());
    }
    for lib in &libraries {
        for key in ["name", "version", "source"] {
            if lib[key].as_str().is_none_or(str::is_empty) {
                problems.push(format!("{lib}: `{key}` is missing"));
            }
        }
        for f in lib["files"].as_array().into_iter().flatten() {
            let (Some(path), Some(sha)) = (f["path"].as_str(), f["sha256"].as_str()) else {
                problems.push(format!("{f}: an entry needs `path` and `sha256`"));
                continue;
            };
            if entries.insert(path.to_string(), sha.to_string()).is_some() {
                problems.push(format!("{path}: listed twice in the manifest"));
            }
        }
    }
    for (path, bytes) in files {
        let actual: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        match entries.remove(path) {
            None => problems.push(format!("{path}: not in the manifest")),
            Some(sha) if sha != actual => problems.push(format!(
                "{path}: sha256 is {actual}, the manifest says {sha}"
            )),
            Some(_) => {}
        }
    }
    for path in entries.keys() {
        problems.push(format!("{path}: in the manifest, but not vendored"));
    }
    problems
}

/// Each vendored file is the one its manifest entry names: a changed byte or
/// a file nobody recorded fails here, so a library bump also updates the
/// version and source the vulnerability scan reads (ADR-0072 D12).
#[test]
fn vendored_files_match_the_manifest() {
    fn walk(dir: &include_dir::Dir<'_>, out: &mut Vec<(String, Vec<u8>)>) {
        for f in dir.files() {
            let path = f.path().to_string_lossy().replace('\\', "/");
            let rel = path.strip_prefix("vendor/").unwrap_or(&path).to_string();
            if rel != "manifest.json" {
                out.push((rel, f.contents().to_vec()));
            }
        }
        for d in dir.dirs() {
            walk(d, out);
        }
    }
    let manifest: serde_json::Value = serde_json::from_slice(
        UI.get_file("vendor/manifest.json")
            .expect("vendor/manifest.json is embedded")
            .contents(),
    )
    .expect("vendor/manifest.json is JSON");
    let mut files = Vec::new();
    walk(
        UI.get_dir("vendor").expect("vendor/ is embedded"),
        &mut files,
    );
    assert!(files.len() > 100, "the walk found the vendored files");
    let problems = check_manifest(&files, &manifest);
    assert!(problems.is_empty(), "{}", problems.join("\n"));

    // Negative controls: the checker sees a changed byte and an extra file.
    let mut flipped = files.clone();
    flipped[0].1[0] ^= 1;
    let problems = check_manifest(&flipped, &manifest);
    assert!(
        problems.len() == 1 && problems[0].starts_with(&format!("{}: sha256 is", flipped[0].0)),
        "{problems:?}"
    );
    let mut extra = files.clone();
    extra.push(("stray.js".to_string(), b"x".to_vec()));
    assert_eq!(
        check_manifest(&extra, &manifest),
        vec!["stray.js: not in the manifest".to_string()]
    );
    let missing = &files[1..];
    assert_eq!(
        check_manifest(missing, &manifest),
        vec![format!("{}: in the manifest, but not vendored", files[0].0)]
    );
}

/// The vendored Alpine is the version its manifest entry names. The hash
/// check cannot see this: the npm tarballs of alpinejs 3.14.0 to 3.14.9 ship
/// a `dist/module.esm.min.js` built from 3.13.10 (measured on 3.14.1 and
/// 3.14.9), which throws `u is not a function` when an `x-show` element is
/// hidden twice inside a parent that is being hidden (issue #579).
#[test]
fn vendored_alpine_is_the_manifest_version() {
    let manifest: serde_json::Value = serde_json::from_slice(
        UI.get_file("vendor/manifest.json")
            .expect("vendor/manifest.json is embedded")
            .contents(),
    )
    .expect("vendor/manifest.json is JSON");
    let lib = manifest["libraries"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|l| l["npm"] == "alpinejs")
        .expect("the manifest lists alpinejs");
    let path = lib["files"][0]["path"]
        .as_str()
        .expect("the alpinejs entry names its file");
    let text = UI
        .get_file(format!("vendor/{path}"))
        .and_then(|f| f.contents_utf8())
        .unwrap_or_else(|| panic!("vendor/{path} is embedded UTF-8"));
    let built = text
        .split_once("version: \"")
        .or_else(|| text.split_once("version:\""))
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(v, _)| v);
    assert_eq!(built, lib["version"].as_str(), "vendor/{path}");
}
