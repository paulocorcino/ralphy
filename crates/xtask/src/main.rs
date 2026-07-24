//! Repo maintenance tasks run out-of-band (never at build time), establishing
//! the `cargo run -p xtask -- <cmd>` pattern for this workspace.
//!
//! `refresh-seed` keeps the offline pricing floor
//! (`assets/pricing/models-dev-seed.json`) current without hand-editing it: it
//! fetches the live models.dev catalog, and for each id already in the seed,
//! updates its cost from upstream where models.dev publishes one — preserving
//! (never dropping or adding) the id set, so vendor spellings the catalog does
//! not carry (Copilot's dotted ids, the CLI's Gemini forms, `kimi-for-coding`)
//! survive. It owns `seed.json` wholesale and never touches the human-owned
//! `slug-overlay.json` (ADR-0034 A3: one owner per file). The result is written
//! deterministically (sorted keys) so its diff is reviewable, and a scheduled CI
//! job opens a PR only when the seed actually changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The providers Ralphy drives — the subset the seed covers, matching the
/// resolver's provider-prefix synthesis (`claude-*`→anthropic, `gpt-*`→openai,
/// `gemini-*`→google, `kimi-*`→moonshotai). The seed holds only these; the
/// refresh never widens it.
const DRIVEN_PROVIDERS: &[&str] = &["anthropic", "openai", "google", "moonshotai"];

/// Official models.dev catalog endpoint. Mirrors `ralphy-cli`'s
/// `pricing::fetch::DEFAULT_MODELS_DEV_URL`; the constant is `pub(crate)` there
/// and this is out-of-band tooling in a separate crate, so it is restated rather
/// than shared behind a new public surface (`anti-over-abstraction`).
const MODELS_DEV_URL: &str = "https://models.dev/api.json";

// Generous, out-of-band timeouts — this is CI/maintenance, not the run hot path.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// The models.dev-shaped seed document: `provider → { models: { id → { cost } } }`.
/// A `BTreeMap` gives sorted, deterministic key ordering on re-serialization.
type SeedDoc = BTreeMap<String, SeedProvider>;

#[derive(Deserialize, Serialize)]
struct SeedProvider {
    models: BTreeMap<String, SeedModel>,
}

#[derive(Deserialize, Serialize)]
struct SeedModel {
    cost: Cost,
}

/// A per-1M price row in the raw models.dev shape (upstream `cache_write`, not the
/// loader's normalized `cache_creation`). Absent cache fields stay absent — the
/// loader maps a missing cache field to `0.0`.
#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
struct Cost {
    input: f64,
    output: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_write: Option<f64>,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("refresh-seed") => refresh_seed_cmd(&args[1..]),
        _ => {
            eprintln!(
                "usage: cargo run -p xtask -- refresh-seed \
                 [--url <models.dev url>] [--seed <path>] [--live-file <path>]"
            );
            std::process::exit(2);
        }
    }
}

fn refresh_seed_cmd(args: &[String]) -> Result<()> {
    let mut url = MODELS_DEV_URL.to_string();
    let mut seed_path: Option<PathBuf> = None;
    let mut live_file: Option<PathBuf> = None;

    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--url" => url = next_value(&mut it, "--url")?,
            "--seed" => seed_path = Some(PathBuf::from(next_value(&mut it, "--seed")?)),
            "--live-file" => live_file = Some(PathBuf::from(next_value(&mut it, "--live-file")?)),
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    let seed_path = seed_path.unwrap_or_else(default_seed_path);
    let live: Value = match live_file {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading live file {}", path.display()))?;
            serde_json::from_str(&text).context("parsing live models.dev file")?
        }
        None => fetch_live(&url)?,
    };

    let current = std::fs::read_to_string(&seed_path)
        .with_context(|| format!("reading seed {}", seed_path.display()))?;
    let refreshed = refresh(&current, &live)?;

    if refreshed == current {
        println!("seed unchanged: {}", seed_path.display());
    } else {
        std::fs::write(&seed_path, refreshed.as_bytes())
            .with_context(|| format!("writing seed {}", seed_path.display()))?;
        println!("seed refreshed: {}", seed_path.display());
    }
    Ok(())
}

/// Refresh each id already present in the seed from `live`, preserving the id set
/// and re-emitting deterministically (sorted keys, trailing newline). Pure: the
/// unit tests drive it with in-memory fixtures.
fn refresh(seed_json: &str, live: &Value) -> Result<String> {
    let mut seed: SeedDoc = serde_json::from_str(seed_json).context("parsing seed JSON")?;

    for &provider in DRIVEN_PROVIDERS {
        let Some(prov) = seed.get_mut(provider) else {
            continue;
        };
        let live_models = live.get(provider).and_then(|p| p.get("models"));
        for (id, model) in prov.models.iter_mut() {
            if let Some(cost) = live_models
                .and_then(|m| m.get(id))
                .and_then(|m| m.get("cost"))
                .and_then(usable_cost)
            {
                model.cost = cost;
            }
        }
    }

    let mut out = serde_json::to_string_pretty(&seed).context("serializing seed")?;
    out.push('\n');
    Ok(out)
}

/// A live cost object → `Cost`, or `None` when unusable: a missing/non-numeric
/// input or output, or a `$0` input+output (the subscription trap). Mirrors the
/// loader's `ingest_models_dev` drop rule so the floor never lists a free major.
fn usable_cost(cost: &Value) -> Option<Cost> {
    let input = num(cost.get("input"))?;
    let output = num(cost.get("output"))?;
    if input == 0.0 && output == 0.0 {
        return None;
    }
    Some(Cost {
        input,
        output,
        cache_read: num(cost.get("cache_read")),
        cache_write: num(cost.get("cache_write")),
    })
}

fn num(v: Option<&Value>) -> Option<f64> {
    v?.as_f64()
}

fn next_value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String> {
    it.next()
        .cloned()
        .with_context(|| format!("missing value for {flag}"))
}

/// `<workspace root>/assets/pricing/models-dev-seed.json`, located from this
/// crate's compile-time manifest dir (`<root>/crates/xtask`).
fn default_seed_path() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("xtask crate lives at <root>/crates/xtask");
    root.join("assets/pricing/models-dev-seed.json")
}

fn fetch_live(url: &str) -> Result<Value> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();
    let body = agent
        .get(url)
        .call()
        .with_context(|| format!("fetching {url}"))?
        .into_string()
        .context("reading models.dev body")?;
    serde_json::from_str(&body).context("parsing models.dev JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SEED: &str = r#"{
      "anthropic": {
        "models": {
          "claude-opus-4-8": { "cost": { "input": 15.0, "output": 75.0, "cache_read": 1.5, "cache_write": 18.75 } },
          "claude-4.6-opus": { "cost": { "input": 5.0, "output": 25.0, "cache_read": 0.5, "cache_write": 6.25 } }
        }
      },
      "openai": {
        "models": {
          "gpt-5.5": { "cost": { "input": 5.0, "output": 30.0, "cache_read": 0.5, "cache_write": 5.0 } }
        }
      }
    }"#;

    fn parse(s: &str) -> SeedDoc {
        serde_json::from_str(s).expect("valid seed")
    }

    #[test]
    fn refreshes_present_ids_and_preserves_absent_ones() {
        // Live re-prices opus lower and omits the Copilot dotted form entirely.
        let live = json!({
            "anthropic": { "models": {
                "claude-opus-4-8": { "cost": { "input": 5, "output": 25, "cache_read": 0.5, "cache_write": 6.25 } }
            }},
            "openai": { "models": {
                "gpt-5.5": { "cost": { "input": 5, "output": 30, "cache_read": 0.5, "cache_write": 5.0 } }
            }}
        });
        let out = parse(&refresh(SEED, &live).unwrap());

        // Present upstream → updated from live.
        let opus = &out["anthropic"].models["claude-opus-4-8"].cost;
        assert_eq!(opus.input, 5.0);
        assert_eq!(opus.output, 25.0);
        // Absent upstream → kept verbatim (the id survives the refresh).
        let dotted = &out["anthropic"].models["claude-4.6-opus"].cost;
        assert_eq!(dotted.input, 5.0);
        assert_eq!(dotted.output, 25.0);
        assert!(out["anthropic"].models.contains_key("claude-4.6-opus"));
    }

    #[test]
    fn never_adds_new_live_ids() {
        let live = json!({
            "anthropic": { "models": {
                "claude-brand-new": { "cost": { "input": 1, "output": 2 } }
            }}
        });
        let out = parse(&refresh(SEED, &live).unwrap());
        assert!(
            !out["anthropic"].models.contains_key("claude-brand-new"),
            "the refresh preserves the seed's id set, never widening it"
        );
    }

    #[test]
    fn zero_cost_live_row_does_not_overwrite_the_seed() {
        // A subscription-billed ($0) upstream row must not zero out a priced major.
        let live = json!({
            "openai": { "models": {
                "gpt-5.5": { "cost": { "input": 0, "output": 0 } }
            }}
        });
        let out = parse(&refresh(SEED, &live).unwrap());
        let gpt = &out["openai"].models["gpt-5.5"].cost;
        assert_eq!(gpt.input, 5.0, "$0 upstream is treated as unpriced, kept");
    }

    #[test]
    fn output_is_deterministic_and_idempotent() {
        let live = json!({ "anthropic": { "models": {} } });
        let once = refresh(SEED, &live).unwrap();
        let twice = refresh(&once, &live).unwrap();
        assert_eq!(once, twice, "a re-run over unchanged data is a no-op");
        assert!(once.ends_with('\n'), "trailing newline for a clean diff");
        // Sorted keys: providers come out alphabetically regardless of input order.
        assert!(
            once.find("\"anthropic\"").unwrap() < once.find("\"openai\"").unwrap(),
            "provider keys must be sorted"
        );
    }

    #[test]
    fn non_driven_provider_in_live_is_ignored() {
        let live = json!({
            "someothervendor": { "models": {
                "gpt-5.5": { "cost": { "input": 1, "output": 1 } }
            }}
        });
        // gpt-5.5 lives under openai in the seed; a same-named model under an
        // unrelated provider must not leak into it.
        let out = parse(&refresh(SEED, &live).unwrap());
        assert_eq!(out["openai"].models["gpt-5.5"].cost.input, 5.0);
    }
}
