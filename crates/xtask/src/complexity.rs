//! `complexity` holds the complexity of every production function (ADR-0081).
//! Rust functions are scored by their cognitive complexity, read from the
//! source text ([`cognitive`]), so every `#[cfg]` branch counts on every
//! platform. TypeScript functions are scored by oxlint's `complexity` rule
//! with `variant: "modified"`. In both, an exhaustive `switch` or `match`
//! counts once. A function over [`LIMIT`]
//! fails unless `docs/complexity-baseline.json` lists it; a listed function
//! may go down, never up, and never over the ceiling the baseline records.
//! `--bless` rewrites the baseline and keeps the ceiling it read. With `--base
//! <rev>`, the functions at [`NEAR`] to [`LIMIT`] in the files changed since
//! `<rev>` are listed, and do not fail.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::asset_pins::repo_root;
use crate::capabilities::git;

/// The highest score a function may have without a baseline entry.
const LIMIT: u32 = 10;
/// The lowest score of the near-limit list. oxlint is asked for every score
/// from here up, so one run gives both lists.
const NEAR: u32 = 7;
/// The oxlint the default `npx` runs; `.github/workflows/ci.yml` pins the
/// same version by checksum.
pub(crate) const OXLINT_VERSION: &str = "1.85.0";
/// The ceiling of a baseline that records none (ADR-0081, amendment A5). It is
/// only where the first ceiling comes from: after that, the file holds it.
const CEILING: u32 = 50;
const BASELINE: &str = "docs/complexity-baseline.json";
const UI_DIR: &str = "crates/ralphy-daemon/assets/ui";
/// Where the generated tool configs go: build output, never committed.
const WORK_DIR: &str = "target/xtask-complexity";
/// The name of a TypeScript function that has none, such as an arrow
/// function passed as an argument. A Rust closure is scored as part of the
/// function around it.
const ANONYMOUS: &str = "(anonymous)";

/// The score of one function. `path` is from the repo root, with `/`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Score {
    path: String,
    name: String,
    value: u32,
}

/// Path → function name → the values of the functions of that name, highest
/// first. A file can hold two functions of one name (`new` in two `impl`
/// blocks, two anonymous functions), so the values are compared as a sorted
/// list and a line number is never part of the identity.
type Table = BTreeMap<String, BTreeMap<String, Vec<u32>>>;

/// The committed file: one ceiling for both languages, and the functions over
/// [`LIMIT`]. The ceiling goes down by hand, when a step of A6 closes.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Baseline {
    ceiling: u32,
    functions: Table,
}

pub fn complexity_cmd(args: &[String]) -> Result<()> {
    let mut check = false;
    let mut bless = false;
    let mut base = None;
    let mut oxlint = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--check" => check = true,
            "--bless" => bless = true,
            "--base" => base = Some(crate::next_value(&mut it, "--base")?),
            "--oxlint" => oxlint = Some(crate::next_value(&mut it, "--oxlint")?),
            other => bail!("unknown argument: {other}"),
        }
    }
    if check == bless {
        bail!("usage: complexity --check | --bless [--base <rev>] [--oxlint <program>]");
    }
    let root = repo_root();
    let mut scores = rust_scores(&root)?;
    scores.extend(oxlint_scores(&root, oxlint.as_deref())?);
    scores.sort();
    if let Some(base) = base {
        print_near(&near_limit(&scores, &changed_files(&root, &base)?));
    }
    let current = table(scores.iter().filter(|s| s.value > LIMIT));
    let path = root.join(BASELINE);
    if bless {
        return bless_baseline(&path, current);
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let baseline = parse_baseline(&text).with_context(|| format!("parsing {BASELINE}"))?;
    let mut errors = compare(&baseline.functions, &current);
    let over_ceiling = ceiling_errors(baseline.ceiling, &current);
    let advice = fix_advice(!errors.is_empty(), !over_ceiling.is_empty());
    errors.extend(over_ceiling);
    if errors.is_empty() {
        println!(
            "{} function(s) over {LIMIT}, as in {BASELINE}, none over the ceiling {}",
            count(&current),
            baseline.ceiling
        );
        return Ok(());
    }
    for error in &errors {
        eprintln!("{error}");
    }
    bail!("{} complexity error(s); {advice}", errors.len())
}

/// What to do about the errors printed. `--bless` records a function that went
/// down. It never raises the ceiling, so a function over it can only be split.
fn fix_advice(compared: bool, over_ceiling: bool) -> String {
    let mut steps = Vec::new();
    if compared || over_ceiling {
        steps.push("split a function that grew along a responsibility it already has");
    }
    if compared {
        steps.push(
            "run `cargo run -p xtask -- complexity --bless` to record a function that went down",
        );
    }
    if over_ceiling {
        steps.push(
            "a function over the ceiling can only be split: --bless does not raise the ceiling",
        );
    }
    steps.join("; ")
}

/// Writes `current` to the baseline at `path`, with the ceiling it holds.
fn bless_baseline(path: &Path, current: Table) -> Result<()> {
    let existing = path
        .is_file()
        .then(|| std::fs::read_to_string(path))
        .transpose()
        .with_context(|| format!("reading {}", path.display()))?;
    let ceiling = ceiling_of(existing.as_deref()).with_context(|| format!("parsing {BASELINE}"))?;
    let baseline = Baseline {
        ceiling,
        functions: current,
    };
    std::fs::write(path, to_json(&baseline)?)
        .with_context(|| format!("writing {}", path.display()))?;
    println!(
        "{BASELINE}: {} function(s) over {LIMIT}, ceiling {ceiling}",
        count(&baseline.functions)
    );
    Ok(())
}

/// Scores every function of the production Rust files under `crates/`,
/// from the source text (ADR-0081, amendment 2026-10-12).
fn rust_scores(root: &Path) -> Result<Vec<Score>> {
    let crates = root.join("crates");
    let mut scores = Vec::new();
    for entry in
        std::fs::read_dir(&crates).with_context(|| format!("reading {}", crates.display()))?
    {
        let dir = entry.context("reading a crate entry")?.path();
        if !dir.join("Cargo.toml").is_file() {
            continue;
        }
        for file in cognitive::production_files(&dir)? {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let path = rel_path(root, &file)?;
            let found = cognitive::file_scores(&text).with_context(|| format!("parsing {path}"))?;
            scores.extend(found.into_iter().map(|(name, value)| Score {
                path: path.clone(),
                name,
                value,
            }));
        }
    }
    Ok(scores)
}

/// Runs oxlint on the workbench modules with only the complexity rule on.
/// `program` replaces the default `npx -y oxlint@<version>`; CI passes the
/// binary it pins by checksum.
fn oxlint_scores(root: &Path, program: Option<&str>) -> Result<Vec<Score>> {
    let work = root.join(WORK_DIR);
    std::fs::create_dir_all(&work).with_context(|| format!("creating {}", work.display()))?;
    let config = work.join("oxlint.json");
    let rule = serde_json::json!({
        "plugins": ["eslint"],
        "categories": { "correctness": "off" },
        "rules": { "complexity": ["warn", { "max": NEAR - 1, "variant": "modified" }] }
    });
    std::fs::write(&config, rule.to_string()).context("writing the oxlint config")?;
    let mut cmd = match program {
        Some(program) => Command::new(program),
        None => {
            // npm installs `npx` as a `.cmd` script on Windows.
            let npx = if cfg!(windows) { "npx.cmd" } else { "npx" };
            let mut cmd = Command::new(npx);
            cmd.args(["-y", &format!("oxlint@{OXLINT_VERSION}")]);
            cmd
        }
    };
    let out = cmd
        .current_dir(root)
        .arg("-c")
        .arg(&config)
        .args(["-f", "json", "--ignore-pattern", "**/vendor/**", UI_DIR])
        .output()
        .context("running oxlint")?;
    if !out.status.success() {
        bail!(
            "oxlint failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_oxlint(&String::from_utf8(out.stdout).context("oxlint output is not UTF-8")?)
}

/// The complexity diagnostics in oxlint's JSON report. A message of another
/// form fails, so a new oxlint wording cannot drop functions in silence.
fn parse_oxlint(report: &str) -> Result<Vec<Score>> {
    static MESSAGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:async )?function(?: `([^`]+)`)? has a complexity of (\d+)\.")
            .expect("a valid literal regex")
    });
    let start = report.find('{').context("no JSON in the oxlint output")?;
    let report: Value =
        serde_json::from_str(&report[start..]).context("parsing the oxlint report")?;
    let diagnostics = report["diagnostics"]
        .as_array()
        .context("no diagnostics list in the oxlint report")?;
    let mut scores = Vec::new();
    for d in diagnostics {
        let text = d["message"].as_str().unwrap_or_default();
        if d["code"] != "eslint(complexity)" {
            bail!("oxlint reported another rule: {text}");
        }
        let caps = MESSAGE
            .captures(text)
            .with_context(|| format!("unknown oxlint complexity message {text:?}"))?;
        scores.push(Score {
            path: slashes(
                d["filename"]
                    .as_str()
                    .with_context(|| format!("no file in oxlint message {text:?}"))?,
            ),
            name: caps.get(1).map_or(ANONYMOUS, |m| m.as_str()).to_string(),
            value: caps[2].parse().context("the score is not a number")?,
        });
    }
    Ok(scores)
}

/// The errors of `current` against `baseline`, one line each.
fn compare(baseline: &Table, current: &Table) -> Vec<String> {
    let empty = BTreeMap::new();
    let paths: BTreeSet<&String> = baseline.keys().chain(current.keys()).collect();
    let mut errors = Vec::new();
    for path in paths {
        let before = baseline.get(path).unwrap_or(&empty);
        let after = current.get(path).unwrap_or(&empty);
        let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
        for name in names {
            let was = before.get(name).map_or(&[][..], Vec::as_slice);
            let now = after.get(name).map_or(&[][..], Vec::as_slice);
            errors.extend(compare_values(&format!("{path}: {name}"), was, now));
        }
    }
    errors
}

/// One error for each function that scores more than `ceiling`.
fn ceiling_errors(ceiling: u32, current: &Table) -> Vec<String> {
    let mut errors = Vec::new();
    for (path, names) in current {
        for (name, values) in names {
            for value in values.iter().filter(|v| **v > ceiling) {
                errors.push(format!(
                    "{path}: {name}: {value} is over the ceiling {ceiling}"
                ));
            }
        }
    }
    errors
}

/// Both lists are sorted, highest first, and compared position by position.
fn compare_values(at: &str, was: &[u32], now: &[u32]) -> Vec<String> {
    let mut errors = Vec::new();
    for (i, value) in now.iter().enumerate() {
        match was.get(i) {
            None => errors.push(format!("{at}: {value}, over the limit of {LIMIT}")),
            Some(old) if value > old => errors.push(format!("{at}: {old} -> {value}")),
            Some(old) if value < old => {
                errors.push(format!("{at}: {old} -> {value}, update the baseline"));
            }
            Some(_) => {}
        }
    }
    for old in was.iter().skip(now.len()) {
        errors.push(format!(
            "{at}: {old} -> {LIMIT} or less, update the baseline"
        ));
    }
    errors
}

fn table<'a>(scores: impl Iterator<Item = &'a Score>) -> Table {
    let mut table = Table::new();
    for s in scores {
        let values = table
            .entry(s.path.clone())
            .or_default()
            .entry(s.name.clone())
            .or_default();
        values.push(s.value);
        values.sort_unstable_by(|a, b| b.cmp(a));
    }
    table
}

fn count(table: &Table) -> usize {
    table
        .values()
        .flat_map(|names| names.values())
        .map(Vec::len)
        .sum()
}

fn to_json(baseline: &Baseline) -> Result<String> {
    let mut out = serde_json::to_string_pretty(baseline).context("serializing the baseline")?;
    out.push('\n');
    Ok(out)
}

fn parse_baseline(text: &str) -> Result<Baseline> {
    serde_json::from_str(text).context(
        "reading the ceiling and the functions; a baseline in the old format has no \
         `ceiling`: run `cargo run -p xtask -- complexity --bless` to migrate it",
    )
}

/// The ceiling `--bless` writes: the one in the existing file, or [`CEILING`]
/// when there is no file or it records none (the format before A5). It is
/// never computed from the scores, so a bless cannot raise it.
fn ceiling_of(existing: Option<&str>) -> Result<u32> {
    let Some(text) = existing else {
        return Ok(CEILING);
    };
    let value: Value = serde_json::from_str(text).context("the file is not JSON")?;
    match value.get("ceiling") {
        None => Ok(CEILING),
        Some(ceiling) => ceiling
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .context("the ceiling is not a number"),
    }
}

/// The scores from [`NEAR`] to [`LIMIT`] in `files`.
fn near_limit<'a>(scores: &'a [Score], files: &BTreeSet<String>) -> Vec<&'a Score> {
    scores
        .iter()
        .filter(|s| (NEAR..=LIMIT).contains(&s.value) && files.contains(&s.path))
        .collect()
}

fn print_near(near: &[&Score]) {
    if near.is_empty() {
        return;
    }
    println!("near the limit of {LIMIT} in the changed files (not an error):");
    for s in near {
        println!("  {}: {} {}", s.path, s.name, s.value);
    }
}

/// The files changed since the merge base with `base`, committed or not,
/// and the new files git does not ignore.
fn changed_files(root: &Path, base: &str) -> Result<BTreeSet<String>> {
    let merge_base = git(root, &["merge-base", base, "HEAD"])
        .with_context(|| format!("finding the merge base with {base}"))?;
    let changed = git(root, &["diff", "--name-only", merge_base.trim()])?;
    let new = git(root, &["ls-files", "--others", "--exclude-standard"])?;
    Ok(changed.lines().chain(new.lines()).map(slashes).collect())
}

/// `path` relative to `root`, with `/` separators on every platform.
fn rel_path(root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .with_context(|| format!("{} is not under {}", path.display(), root.display()))?;
    Ok(slashes(&rel.to_string_lossy()))
}

fn slashes(path: &str) -> String {
    path.replace('\\', "/")
}

mod cognitive;

#[cfg(test)]
mod tests;
