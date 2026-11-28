//! The vendored files the binary stores gzipped: `$OUT_DIR/ui/vendor` is moved
//! to `$OUT_DIR/ui-gz/vendor`, each file as its gzip body, when gzip saves a
//! tenth of it or more. `lib.rs` embeds that tree as `UI_GZ`, the daemon sends
//! the body as it is to a browser that admits gzip, and inflates it otherwise.
//! Our own files stay as they are, so every reader of their bytes is unchanged.
//!
//! Measured on 2026-11-27: `vendor/` is 18.8 MB, and 5.96 MB once stored this
//! way; compressing it takes 2.7 s with flate2 built at `opt-level = 0`, the
//! release profile of a build script.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;

/// Move each file of `ui/vendor` that gzip shrinks enough to the same path
/// under `gz`, as its gzip body. `gz` is emptied first and always exists.
pub(crate) fn store(ui: &Path, gz: &Path) -> Result<()> {
    if gz.exists() {
        fs::remove_dir_all(gz).with_context(|| format!("empty {}", gz.display()))?;
    }
    fs::create_dir_all(gz).with_context(|| format!("create {}", gz.display()))?;
    let vendor = ui.join("vendor");
    if vendor.is_dir() {
        store_dir(&vendor, &gz.join("vendor"))?;
    }
    Ok(())
}

fn store_dir(src: &Path, out: &Path) -> Result<()> {
    for entry in fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let path = entry
            .with_context(|| format!("read an entry of {}", src.display()))?
            .path();
        let target = out.join(path.file_name().context("an entry has a file name")?);
        if path.is_dir() {
            store_dir(&path, &target)?;
        } else {
            store_file(&path, &target)?;
        }
    }
    Ok(())
}

fn store_file(path: &Path, target: &Path) -> Result<()> {
    let raw = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let body = gzip(&raw).with_context(|| format!("gzip {}", path.display()))?;
    if !worth_storing(raw.len(), body.len()) {
        return Ok(());
    }
    let dir = target.parent().context("a stored file has a folder")?;
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    fs::write(target, body).with_context(|| format!("write {}", target.display()))?;
    fs::remove_file(path).with_context(|| format!("remove {}", path.display()))
}

/// The gzip body of `raw`. The header carries no time and no name, so the
/// same bytes give the same body on every build.
fn gzip(raw: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut enc = GzEncoder::new(Vec::with_capacity(raw.len() / 3), Compression::default());
    enc.write_all(raw)?;
    enc.finish()
}

/// A file is stored gzipped when that saves a tenth of it or more: a font or
/// an image that is compressed already stays as it is.
fn worth_storing(raw: usize, gzipped: usize) -> bool {
    gzipped.saturating_mul(10) <= raw.saturating_mul(9)
}
