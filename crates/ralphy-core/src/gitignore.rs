//! Keep run artifacts self-contained: make the target repo ignore `.ralphy/` so
//! scratch and logs never leak into commits.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;

/// Ensure the repo's `.gitignore` ignores `.ralphy/`. Returns `true` if the file
/// was modified, `false` if the entry was already present.
pub fn ensure_ralphy_ignored(repo_root: &Path) -> Result<bool> {
    let path = repo_root.join(".gitignore");
    let mut text = if path.exists() {
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    let re = Regex::new(r"(?m)^\s*\.ralphy/?\s*$").expect("valid regex");
    if re.is_match(&text) {
        return Ok(false);
    }

    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# Ralphy run artifacts (scratch, logs, per-run plans)\n.ralphy/\n");
    fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ralphy-gi-{}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
            name
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn appends_when_missing() {
        let dir = tmp("missing");
        assert!(ensure_ralphy_ignored(&dir).unwrap());
        let body = fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(body.contains(".ralphy/"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn noop_when_present() {
        let dir = tmp("present");
        fs::write(dir.join(".gitignore"), "target/\n.ralphy/\n").unwrap();
        assert!(!ensure_ralphy_ignored(&dir).unwrap());
        fs::remove_dir_all(&dir).ok();
    }
}
