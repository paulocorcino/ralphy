//! The vendored libraries match their manifest.

use super::*;

/// Every problem `vendor/manifest.json` has against `files` (path relative to
/// `vendor/`, embedded bytes). Empty when each file has exactly one entry
/// whose SHA-256 matches, and each entry names a file that exists.
fn check_manifest(files: &[(String, Vec<u8>)], manifest: &serde_json::Value) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let mut problems = Vec::new();
    let mut entries = std::collections::BTreeMap::<String, String>::new();
    let libraries = manifest["libraries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if libraries.is_empty() {
        problems.push("the manifest lists no libraries".to_owned());
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
            if entries.insert(path.to_owned(), sha.to_owned()).is_some() {
                problems.push(format!("{path}: listed twice in the manifest"));
            }
        }
    }
    for (path, bytes) in files {
        let actual = Sha256::digest(bytes)
            .iter()
            .fold(String::new(), |mut hex, b| {
                write!(hex, "{b:02x}").expect(WRITE_TO_STRING);
                hex
            });
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

fn vendor_manifest() -> serde_json::Value {
    let bytes = embedded_bytes("vendor/manifest.json").expect("vendor/manifest.json is embedded");
    serde_json::from_slice(&bytes).expect("vendor/manifest.json is JSON")
}

/// Each vendored file is the one its manifest entry names: a changed byte or
/// a file nobody recorded fails here, so a library bump also updates the
/// version and source the vulnerability scan reads (ADR-0072 D12).
// pin: every vendored file is the one its manifest entry names —
// it compares bytes to hashes; no behavior reads the manifest.
#[test]
fn vendored_files_match_the_manifest() {
    let manifest = vendor_manifest();
    // The bytes as served, inflated where the binary stores them gzipped.
    let mut files: Vec<(String, Vec<u8>)> = embedded_ui_paths()
        .into_iter()
        .filter_map(|path| {
            let rel = path.strip_prefix("vendor/")?;
            (rel != "manifest.json").then(|| (rel.to_owned(), embedded_bytes(&path)))
        })
        .map(|(rel, bytes)| (rel, bytes.expect("a listed path is embedded")))
        .collect();
    files.sort();
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
    extra.push(("stray.js".to_owned(), b"x".to_vec()));
    assert_eq!(
        check_manifest(&extra, &manifest),
        vec!["stray.js: not in the manifest".to_owned()]
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
// pin: the vendored Alpine is the version its manifest names —
// the stale build runs fine until a nested x-show, which no test renders.
#[test]
fn vendored_alpine_is_the_manifest_version() {
    let manifest = vendor_manifest();
    let lib = manifest["libraries"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|l| l["npm"] == "alpinejs")
        .expect("the manifest lists alpinejs");
    let path = lib["files"][0]["path"]
        .as_str()
        .expect("the alpinejs entry names its file");
    let text = embedded_text(&format!("vendor/{path}"))
        .unwrap_or_else(|| panic!("vendor/{path} is embedded UTF-8"));
    let built = text
        .split_once("version: \"")
        .or_else(|| text.split_once("version:\""))
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(v, _)| v);
    assert_eq!(built, lib["version"].as_str(), "vendor/{path}");
}
