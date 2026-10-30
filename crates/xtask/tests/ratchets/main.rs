//! Ratchets over coupling that exists today and must not grow (ADR-0068).
//! Each baseline is exact: a count that goes down lowers its baseline in the
//! same change, so the ratchet never leaves room to grow back.
//!
//! The spawn ratchet sees only a literal program name:
//! `Command::new("git")`. A spawn through a variable (`find_program("ssh")`,
//! `Command::new(&program)`) is not seen and is reviewed in the PR.
//!
//! The forge ratchet reads `github::` paths as text. It does not see an alias
//! (`github as gh`), a root re-export (`ralphy_core::GhTracker`) or a name
//! after a nested group (`github::{a::{b}, c}`).
//!
//! The `any` ratchet reads the workbench modules through the `ui-copy` lexer,
//! so a comment or a string that says `any` is not counted. It counts what
//! oxlint's `typescript/no-explicit-any` reports, file by file (#613).
//!
//! The workbench coupling ratchets (#623) read the modules through the same
//! lexer: the uses of `WBConsole` in `wb-notes.ts`, the uses of the shell's
//! `_flashAction` in the `wb-*.ts` modules, and the one owner of each column
//! paint function.
//!
//! The peer reply pin is not a count. It ties the page types that read a
//! reply a peer daemon wrote to the peer protocol version (#653), through the
//! same lexer, so a comment or a layout change does not move it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use regex::Regex;

// The `ui-copy` lexer, compiled here a third time: xtask has no `[lib]`, and
// one lexer means the ratchet and `ui-copy` drop comments and strings by the
// same rule. The ratchets read only part of its API.
#[allow(dead_code)]
#[path = "../../src/ui_copy/lex.rs"]
mod lex;

mod peer_reply_pin;
mod rust_coupling;
mod skipped_tests;
mod ts_scan;
mod type_escapes;
mod workbench_coupling;
mod workflows_locked;
use ts_scan::{chained_casts, double_casts, websocket_sites};

/// The served workbench modules, from the repo root. `vendor/` and
/// `ui-tests/` are not read.
const UI_DIR: &str = "crates/ralphy-daemon/assets/ui";

/// The `.ts` modules directly under `ui`: `vendor/` is not read.
fn ts_modules(ui: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(ui).unwrap_or_else(|e| panic!("reading {}: {e}", ui.display()));
    let mut modules = Vec::new();
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", ui.display()))
            .path();
        let is_ts = path.extension().is_some_and(|ext| ext == "ts");
        if path.is_file() && is_ts {
            modules.push(path);
        }
    }
    assert!(
        modules.len() > 10,
        "expected the workbench modules under {}, found {}",
        ui.display(),
        modules.len()
    );
    modules
}

/// One line per key whose count is not the baseline's: a key the baseline
/// does not have, or a count that moved in either direction. A baseline is
/// exact, so a count that went down cannot grow back later.
fn ratchet_errors<K: Ord>(
    actual: &BTreeMap<K, usize>,
    baseline: &BTreeMap<K, usize>,
    new: impl Fn(&K, usize) -> String,
    changed: impl Fn(&K, usize, usize) -> String,
) -> Vec<String> {
    let mut errors = Vec::new();
    for (key, count) in actual {
        if !baseline.contains_key(key) {
            errors.push(new(key, *count));
        }
    }
    for (key, expected) in baseline {
        let found = actual.get(key).copied().unwrap_or(0);
        if found != *expected {
            errors.push(changed(key, *expected, found));
        }
    }
    errors
}

/// The tokens of `type name … ;` or `interface name … { … }`, from the
/// keyword: `export` and `declare` are not part of the shape.
fn type_declaration(toks: &[lex::Token], name: &str) -> Option<std::ops::Range<usize>> {
    let starts_statement = |k: usize| {
        k == 0
            || toks[k - 1].is(";")
            || toks[k - 1].is("}")
            || matches!(toks[k - 1].ident(), Some("export" | "declare"))
    };
    let start = (0..toks.len().saturating_sub(1)).find(|&k| {
        matches!(toks[k].ident(), Some("type" | "interface"))
            && toks[k + 1].ident() == Some(name)
            && starts_statement(k)
    })?;
    let interface = toks[start].ident() == Some("interface");
    let mut depth = 0usize;
    for (k, t) in toks.iter().enumerate().skip(start) {
        if t.is("(") || t.is("[") || t.is("{") {
            depth += 1;
        } else if t.is(")") || t.is("]") || t.is("}") {
            depth = depth.saturating_sub(1);
            if interface && depth == 0 && t.is("}") {
                return Some(start..k + 1);
            }
        } else if !interface && depth == 0 && t.is(";") {
            return Some(start..k + 1);
        }
    }
    None
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}
