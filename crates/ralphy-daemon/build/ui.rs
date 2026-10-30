//! The served workbench tree (ADR-0075 D3): `assets/ui/` copied to
//! `$OUT_DIR/ui`, with each `.ts` file written as a `.js` file whose types are
//! removed. `lib.rs` embeds the output with `include_dir!`.
//!
//! The types are replaced by spaces (`swc_ts_fast_strip`, `StripOnly` mode, the
//! engine Node uses to run `.ts`), so every line and column of the served file
//! is the line and column of the source: an error in the browser points at the
//! `.ts` line. A relative import path then changes from `.ts` to `.js`, which is
//! the same length, so the columns still hold.
//!
//! A top-level `.html` file is not copied: the pages are written with their
//! parts put in (`parts.rs`, ADR-0080), and a part is never written.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use swc_common::errors::Handler;
use swc_common::sync::Lrc;
use swc_common::{FileName, Globals, SourceMap, GLOBALS};
use swc_ecma_ast::{EsVersion, ModuleDecl, ModuleItem, Str};
use swc_ecma_parser::{parse_file_as_module, EsSyntax, Syntax};
use swc_ts_fast_strip::{operate, Mode, Options};

use crate::parts;

/// Source files that type-check the tree and are not served.
fn is_type_only(name: &str) -> bool {
    name == "tsconfig.json" || name.ends_with(".d.ts")
}

/// A source file `copy_dir` leaves for another step: a type-only file is not
/// served, and a top-level `.html` file is a page or a part, which `put_pages`
/// writes.
fn is_left_out(name: &str, top: bool) -> bool {
    is_type_only(name) || (top && name.ends_with(".html"))
}

/// Build `out` from `src`: an exact copy, except that a `.ts` file becomes a
/// `.js` file and a page has its parts put in (ADR-0080 D1). `out` is emptied
/// first, so a deleted source leaves no file.
pub fn build(src: &Path, out: &Path) -> Result<()> {
    if out.exists() {
        fs::remove_dir_all(out).with_context(|| format!("empty {}", out.display()))?;
    }
    copy_dir(src, out, true)?;
    put_pages(src, out)
}

/// Write the pages of the top level of `src` with their parts put in. A part
/// is read and never written (ADR-0080 D2).
fn put_pages(src: &Path, out: &Path) -> Result<()> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let entry = entry.with_context(|| format!("read an entry of {}", src.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_file() && name.ends_with(".html") {
            let text = fs::read_to_string(entry.path())
                .with_context(|| format!("read {}", entry.path().display()))?;
            files.insert(name, text);
        }
    }
    for (name, text) in parts::put_together(files).context("put the parts into the pages")? {
        fs::write(out.join(&name), text).with_context(|| format!("write {name}"))?;
    }
    Ok(())
}

fn copy_dir(src: &Path, out: &Path, top: bool) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let entry = entry.with_context(|| format!("read an entry of {}", src.display()))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            copy_dir(&path, &out.join(&name), false)?;
        } else if is_left_out(&name, top) {
            continue;
        } else if let Some(stem) = name.strip_suffix(".ts") {
            let js = format!("{stem}.js");
            if src.join(&js).exists() {
                bail!(
                    "{} and {js} both exist: one module has one source",
                    path.display()
                );
            }
            let code =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            let built = strip(&name, code)
                .with_context(|| format!("remove the types of {}", path.display()))?;
            fs::write(out.join(&js), built).with_context(|| format!("write {js}"))?;
        } else {
            fs::copy(&path, out.join(&name)).with_context(|| format!("copy {}", path.display()))?;
        }
    }
    Ok(())
}

/// One `.ts` module as JavaScript, line for line.
fn strip(name: &str, code: String) -> Result<String> {
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_emitter_writer(Box::new(std::io::stderr()), Some(cm.clone()));
    let options = Options {
        module: Some(true),
        filename: Some(name.to_string()),
        mode: Mode::StripOnly,
        ..Default::default()
    };
    let stripped = GLOBALS
        .set(&Globals::new(), || operate(&cm, &handler, code, options))
        .map_err(|e| anyhow::anyhow!("{e:?}"))?
        .code;
    rewrite_imports(name, stripped)
}

/// Change each relative import path from `.ts` to `.js`. A relative path must
/// name a `.ts` file (ADR-0075 D2), because Node runs the tests on the `.ts`
/// source and the browser loads the `.js` output.
fn rewrite_imports(name: &str, code: String) -> Result<String> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(Lrc::new(FileName::Custom(name.to_string())), code.clone());
    let mut recovered = Vec::new();
    let module = parse_file_as_module(
        &fm,
        Syntax::Es(EsSyntax::default()),
        EsVersion::latest(),
        None,
        &mut recovered,
    )
    .map_err(|e| anyhow::anyhow!("parse the stripped module: {:?}", e.kind()))?;
    if let Some(e) = recovered.first() {
        bail!("parse the stripped module: {:?}", e.kind());
    }
    let base = fm.start_pos.0;
    let mut out = code.into_bytes();
    let sources = module.body.iter().filter_map(|item| match item {
        ModuleItem::ModuleDecl(ModuleDecl::Import(d)) => Some(&*d.src),
        ModuleItem::ModuleDecl(ModuleDecl::ExportAll(d)) => Some(&*d.src),
        ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(d)) => d.src.as_deref(),
        ModuleItem::ModuleDecl(_) | ModuleItem::Stmt(_) => None,
    });
    for src in sources {
        let (lo, hi) = span_of(src, base);
        // The literal with its quotes: `"./wb-hosts.ts"`.
        let literal = std::str::from_utf8(&out[lo..hi]).context("an import path is not UTF-8")?;
        let path = &literal[1..literal.len() - 1];
        if !(path.starts_with("./") || path.starts_with("../")) {
            continue;
        }
        // A vendored file is served as it is, so its path stays (ADR-0075 D5).
        if path.starts_with("./vendor/") && path.ends_with(".js") {
            continue;
        }
        if !path.ends_with(".ts") {
            bail!("the import {literal} must name a .ts file or a .js file under ./vendor/");
        }
        out[hi - 3..hi - 1].copy_from_slice(b"js");
    }
    String::from_utf8(out).context("the rewritten module is not UTF-8")
}

fn span_of(src: &Str, base: u32) -> (usize, usize) {
    (
        (src.span.lo.0 - base) as usize,
        (src.span.hi.0 - base) as usize,
    )
}
