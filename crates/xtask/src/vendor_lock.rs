//! `vendor-lock <out>` turns the vendored-library manifest
//! (`crates/ralphy-daemon/assets/ui/vendor/manifest.json`) into a minimal npm
//! `package-lock.json`, so `osv-scanner scan --lockfile` can check the
//! vendored versions against the advisory database (ADR-0072 D12). The
//! manifest stays the one record of the versions; this file is derived and
//! never committed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::asset_pins::repo_root;

const MANIFEST: &str = "crates/ralphy-daemon/assets/ui/vendor/manifest.json";

pub fn vendor_lock_cmd(args: &[String]) -> Result<()> {
    let [out] = args else {
        anyhow::bail!("usage: vendor-lock <out>");
    };
    let manifest_path = repo_root().join(MANIFEST);
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;
    let lock = lockfile(&manifest)?;
    let out = PathBuf::from(out);
    write(&out, &lock)?;
    println!("wrote {}", out.display());
    Ok(())
}

fn write(out: &Path, lock: &Value) -> Result<()> {
    let body = serde_json::to_string_pretty(lock).context("serializing the lockfile")?;
    std::fs::write(out, body + "\n").with_context(|| format!("writing {}", out.display()))
}

/// One `node_modules/<npm>` package per library with an npm name. A library
/// with `"npm": null` has no registry entry, so the scan cannot check it.
fn lockfile(manifest: &Value) -> Result<Value> {
    let libraries = manifest["libraries"]
        .as_array()
        .context("the manifest has no `libraries` array")?;
    let mut packages = Map::new();
    packages.insert(
        String::new(),
        json!({ "name": "ralphy-vendored-ui", "version": "0.0.0" }),
    );
    for lib in libraries {
        let Some(npm) = lib["npm"].as_str() else {
            continue;
        };
        let version = lib["version"]
            .as_str()
            .with_context(|| format!("library {npm} has no version"))?;
        packages.insert(format!("node_modules/{npm}"), json!({ "version": version }));
    }
    Ok(json!({
        "name": "ralphy-vendored-ui",
        "version": "0.0.0",
        "lockfileVersion": 3,
        "requires": true,
        "packages": packages,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_lock_lists_each_npm_library() {
        let manifest = json!({"libraries": [
            {"name": "xterm.js", "npm": "@xterm/xterm", "version": "6.0.0",
             "source": "s", "files": []},
            {"name": "local", "npm": null, "version": "1.0.0",
             "source": "s", "files": []},
        ]});
        let lock = lockfile(&manifest).expect("a valid manifest");
        assert_eq!(lock["lockfileVersion"], 3);
        let packages = lock["packages"].as_object().expect("packages");
        assert_eq!(packages["node_modules/@xterm/xterm"]["version"], "6.0.0");
        assert_eq!(packages.len(), 2, "the root and one library: {packages:?}");
        assert!(packages.contains_key(""));
    }

    #[test]
    fn a_library_with_no_version_is_an_error() {
        let manifest = json!({"libraries": [{"npm": "x"}]});
        let err = lockfile(&manifest).expect_err("no version");
        assert!(err.to_string().contains("has no version"), "{err}");
    }

    #[test]
    fn the_committed_manifest_makes_a_lockfile() {
        let text = std::fs::read_to_string(repo_root().join(MANIFEST)).expect("manifest");
        let manifest: Value = serde_json::from_str(&text).expect("manifest JSON");
        let lock = lockfile(&manifest).expect("lockfile");
        let packages = lock["packages"].as_object().expect("packages");
        assert!(packages.len() > 10, "{packages:?}");
    }
}
