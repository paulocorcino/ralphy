use super::*;
use crate::pricing::RunAgents;

fn million(model: &str) -> UsageLite {
    UsageLite {
        input: 1_000_000,
        model: Some(model.into()),
        ..Default::default()
    }
}

fn floor_price(model: &str) -> f64 {
    PriceTable::defaults()
        .cost_usd(
            model,
            &ralphy_pricing::TokenCounts {
                input: 1_000_000,
                ..Default::default()
            },
        )
        .expect("the floor prices the model")
}

#[test]
fn the_meter_prices_an_agy_id_by_its_base_id() {
    let pt = PriceTable::defaults();
    let agents = RunAgents::new("antigravity", "antigravity");
    let plan = million("gemini-3.8-flash-low");
    let exec = million("gemini-3.8-flash-low");

    let m = meter_for(&pt, &agents, Some(&plan), &exec);

    let expected = 2.0 * floor_price("gemini-3.8-flash");
    assert!(
        (m.usd.expect("priced") - expected).abs() < 1e-9,
        "{:?}",
        m.usd
    );
    assert!(!m.partial, "no `+?` for a priced agy id");
}

#[test]
fn a_run_without_antigravity_keeps_an_id_suffix_in_the_meter() {
    let pt = PriceTable::defaults();
    let agents = RunAgents::new("claude", "claude");
    let exec = million("gemini-3.8-flash-high");

    let m = meter_for(&pt, &agents, None, &exec);

    assert_eq!(m.usd, None);
    assert!(m.partial);
}
