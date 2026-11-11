use super::*;
use std::sync::Mutex;

/// Serialises tests that mutate the process-global pricing env vars, so
/// `cargo test`'s parallel runner can't race them (mirrors telegram config).
pub(super) static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Remove a scratch directory a test made. It may be gone already, and a
/// leftover directory in the temp dir fails nothing.
pub(super) fn remove_scratch(dir: &std::path::Path) {
    if let Err(e) = std::fs::remove_dir_all(dir) {
        tracing::debug!("could not remove test dir {}: {e}", dir.display());
    }
}

pub(super) fn one_million_each() -> TokenCounts {
    TokenCounts {
        input: 1_000_000,
        output: 1_000_000,
        cache_read: 1_000_000,
        cache_creation: 1_000_000,
    }
}

#[test]
fn cost_usd_arithmetic_frozen_opus_fixture() {
    // ADR-0008 D8 oracle — inline rates only; no seed involved.
    let price = ModelPrice {
        input: 15.0,
        output: 75.0,
        cache_read: 1.5,
        cache_creation: 18.75,
    };
    let table = PriceTable::from_layers(
        BTreeMap::from([("claude-opus-4-8".into(), price)]),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    );
    let cost = table
        .cost_usd("claude-opus-4-8", &one_million_each())
        .expect("fixture prices");
    assert!(
        (cost - 110.25).abs() < 1e-9,
        "frozen arithmetic must be 110.25, got {cost}"
    );
}

#[test]
fn opus_pipeline_resolves_via_synthesis_to_seed() {
    let table = PriceTable::defaults();
    let cost = table
        .cost_usd("claude-opus-4-8", &one_million_each())
        .expect("opus resolves via synthesis to seed");
    // Seed carries the former defaults rates (15/75/1.5/18.75).
    assert!(
        (cost - 110.25).abs() < 1e-9,
        "pipeline cost tracks seed rates; got {cost}"
    );
    assert_eq!(
        synthesize("claude-opus-4-8").as_deref(),
        Some("anthropic/claude-opus-4-8")
    );
}

#[test]
fn resolver_seed_overlay_override_and_unknown() {
    let seed = BTreeMap::from([(
        "anthropic/claude-opus-4-8".into(),
        ModelPrice {
            input: 15.0,
            output: 75.0,
            cache_read: 1.5,
            cache_creation: 18.75,
        },
    )]);
    let overlay = BTreeMap::from([(
        "k2p6".into(),
        ModelPrice {
            input: 0.95,
            output: 4.0,
            cache_read: 0.16,
            cache_creation: 0.95,
        },
    )]);
    let table = PriceTable::from_layers(
        BTreeMap::new(),
        BTreeMap::new(),
        seed.clone(),
        overlay.clone(),
    );
    let tokens = one_million_each();

    let opus = table
        .cost_usd("claude-opus-4-8", &tokens)
        .expect("opus via seed synthesis");
    assert!((opus - 110.25).abs() < 1e-9);

    let k2 = table.cost_usd("k2p6", &tokens).expect("k2p6 via overlay");
    assert!((k2 - (0.95 + 4.0 + 0.16 + 0.95)).abs() < 1e-9);
    assert_eq!(synthesize("k2p6"), None);

    // Seed alone, no overlay row for opus; overlay alone no seed for k2p6.
    let seed_only =
        PriceTable::from_layers(BTreeMap::new(), BTreeMap::new(), seed, BTreeMap::new());
    assert!(seed_only.cost_usd("k2p6", &tokens).is_none());
    let overlay_only =
        PriceTable::from_layers(BTreeMap::new(), BTreeMap::new(), BTreeMap::new(), overlay);
    assert!(overlay_only.cost_usd("claude-opus-4-8", &tokens).is_none());

    assert_eq!(table.cost_usd("big-pickle", &tokens), None);

    let overrides = BTreeMap::from([(
        "claude-opus-4-8".into(),
        ModelPrice {
            input: 30.0,
            output: 75.0,
            cache_read: 1.5,
            cache_creation: 18.75,
        },
    )]);
    let with_override = PriceTable::from_layers(
        overrides,
        BTreeMap::new(),
        BTreeMap::from([(
            "anthropic/claude-opus-4-8".into(),
            ModelPrice {
                input: 15.0,
                output: 75.0,
                cache_read: 1.5,
                cache_creation: 18.75,
            },
        )]),
        BTreeMap::new(),
    );
    let overridden = with_override
        .cost_usd("claude-opus-4-8", &tokens)
        .expect("override");
    assert!(
        (overridden - 125.25).abs() < 1e-9,
        "override must beat seed; got {overridden}"
    );
}

#[test]
fn a_bare_cache_key_beats_the_same_overlay_key() {
    let price = |input: f64| ModelPrice {
        input,
        output: 0.0,
        cache_read: 0.0,
        cache_creation: 0.0,
    };
    let table = PriceTable::from_layers(
        BTreeMap::new(),
        BTreeMap::from([("k2p6".into(), price(1.0))]),
        BTreeMap::new(),
        BTreeMap::from([("k2p6".into(), price(2.0))]),
    );
    let cost = table
        .cost_usd("k2p6", &one_million_each())
        .expect("k2p6 is in the cache");
    assert!((cost - 1.0).abs() < 1e-9, "cache must win; got {cost}");
}

#[test]
fn dated_model_id_falls_back_to_undated_family_price() {
    let table = PriceTable::defaults();
    let tokens = one_million_each();
    // The dated id Claude Code keys `modelUsage` by resolves to the undated
    // `claude-haiku-4-5` family price (1 + 5 + 0.1 + 1.25 = 7.35 over 1M each).
    let dated = table
        .cost_usd("claude-haiku-4-5-20251001", &tokens)
        .expect("dated haiku id resolves via family fallback");
    let undated = table
        .cost_usd("claude-haiku-4-5", &tokens)
        .expect("undated haiku is priced");
    assert!(
        (dated - undated).abs() < 1e-9,
        "dated id must price identically to its family, got {dated} vs {undated}"
    );
    // A non-date trailing segment is NOT stripped — a genuinely unknown model
    // stays unknown (never silently mispriced).
    assert_eq!(table.cost_usd("claude-haiku-4-5-turbo", &tokens), None);
    assert_eq!(strip_release_date("claude-opus-4-8"), "claude-opus-4-8");
    assert_eq!(
        strip_release_date("claude-haiku-4-5-20251001"),
        "claude-haiku-4-5"
    );
}

#[test]
fn override_file_reprices_a_model_differently_from_defaults() {
    let _g = ENV_LOCK.lock().unwrap();
    // Write a temp override that doubles opus input, point the loader at it.
    let dir = std::env::temp_dir().join(format!("ralphy-pricing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("pricing.toml");
    std::fs::write(
        &file,
        "[claude-opus-4-8]\ninput = 30.0\noutput = 75.0\n\
         cache_read = 1.5\ncache_creation = 18.75\n",
    )
    .expect("write override");
    std::env::set_var("RALPHY_PRICING_FILE", &file);

    let loaded = PriceTable::load();
    let defaults = PriceTable::defaults();
    let tokens = one_million_each();
    let loaded_cost = loaded.cost_usd("claude-opus-4-8", &tokens).unwrap();
    let default_cost = defaults.cost_usd("claude-opus-4-8", &tokens).unwrap();

    // The override doubled the input rate (15 → 30): +15 over the default.
    assert!(
        (loaded_cost - 125.25).abs() < 1e-9,
        "override reprices to 125.25, got {loaded_cost}"
    );
    assert!(
        (loaded_cost - default_cost).abs() > 1e-9,
        "the override must re-price differently from defaults (read-time projection)"
    );

    std::env::remove_var("RALPHY_PRICING_FILE");
    remove_scratch(&dir);
}

#[test]
fn offline_true_with_model_override_still_loads_overrides() {
    let _g = ENV_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("ralphy-pricing-offline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("pricing.toml");
    std::fs::write(
        &file,
        "offline = true\n\n[claude-opus-4-8]\ninput = 30.0\noutput = 75.0\n\
         cache_read = 1.5\ncache_creation = 18.75\n",
    )
    .expect("write");
    std::env::set_var("RALPHY_PRICING_FILE", &file);
    assert!(pricing_offline_from_file());
    let loaded = PriceTable::load();
    let cost = loaded
        .cost_usd("claude-opus-4-8", &one_million_each())
        .unwrap();
    assert!(
        (cost - 125.25).abs() < 1e-9,
        "offline + override must still reprice; got {cost}"
    );
    std::env::remove_var("RALPHY_PRICING_FILE");
    remove_scratch(&dir);
}

#[test]
fn synthesize_maps_each_provider_prefix() {
    assert_eq!(
        synthesize("claude-opus-4-8").as_deref(),
        Some("anthropic/claude-opus-4-8")
    );
    assert_eq!(synthesize("gpt-5.5").as_deref(), Some("openai/gpt-5.5"));
    assert_eq!(
        synthesize("gemini-3-flash").as_deref(),
        Some("google/gemini-3-flash")
    );
    assert_eq!(
        synthesize("kimi-for-coding").as_deref(),
        Some("moonshotai/kimi-for-coding")
    );
    assert_eq!(synthesize("k2p6"), None);
    assert_eq!(synthesize("composer-2.5"), None);
}

#[test]
fn cache_layer_beats_seed_via_synthesis() {
    let seed = BTreeMap::from([(
        "anthropic/claude-opus-4-8".into(),
        ModelPrice {
            input: 15.0,
            output: 75.0,
            cache_read: 1.5,
            cache_creation: 18.75,
        },
    )]);
    let cache = BTreeMap::from([(
        "anthropic/claude-opus-4-8".into(),
        ModelPrice {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_creation: 6.25,
        },
    )]);
    let table = PriceTable::from_layers(BTreeMap::new(), cache, seed, BTreeMap::new());
    let cost = table
        .cost_usd("claude-opus-4-8", &one_million_each())
        .expect("cache hit");
    // Cache rates 5+25+0.5+6.25 = 36.75, not seed's 110.25.
    assert!(
        (cost - 36.75).abs() < 1e-9,
        "cache must beat seed; got {cost}"
    );
}

#[test]
fn load_reads_disk_cache_when_ralphy_pricing_cache_set() {
    let _g = ENV_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("ralphy-pricing-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let cache_file = dir.join("models-dev.json");
    std::fs::write(
        &cache_file,
        concat!(
            r#"{"timestamp":"2026-07-23T00:00:00Z","data":{"anthropic/claude-opus-4-8":"#,
            r#"{"input":5.0,"output":25.0,"cache_read":0.5,"cache_creation":6.25}}}"#
        ),
    )
    .expect("write cache");
    std::env::set_var("RALPHY_PRICING_CACHE", &cache_file);
    // Ensure no override file interferes.
    std::env::set_var(
        "RALPHY_PRICING_FILE",
        dir.join("missing-pricing.toml").as_os_str(),
    );

    let loaded = PriceTable::load();
    let cost = loaded
        .cost_usd("claude-opus-4-8", &one_million_each())
        .expect("cache-backed opus");
    assert!(
        (cost - 36.75).abs() < 1e-9,
        "load must apply disk cache over seed; got {cost}"
    );

    std::env::remove_var("RALPHY_PRICING_CACHE");
    std::env::remove_var("RALPHY_PRICING_FILE");
    remove_scratch(&dir);
}
