use ralphy_core::Effort;

#[test]
fn effort_parses_orders_and_round_trips_the_core_lexicon() {
    let spellings = ["low", "medium", "high", "xhigh", "max"];
    let parsed = spellings
        .iter()
        .map(|spelling| spelling.parse::<Effort>().expect("valid core effort"))
        .collect::<Vec<_>>();

    assert!(parsed.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        parsed.iter().map(ToString::to_string).collect::<Vec<_>>(),
        spellings
    );
    assert_eq!(
        parsed.iter().map(AsRef::<str>::as_ref).collect::<Vec<_>>(),
        spellings
    );

    for invalid in ["none", "minimal", "hihg"] {
        assert!(invalid.parse::<Effort>().is_err(), "accepted {invalid}");
    }
}

#[test]
fn effort_serializes_as_its_canonical_string() {
    let high: Effort = serde_json::from_str("\"high\"").expect("deserialize effort");
    assert_eq!(high, Effort::High);
    assert_eq!(serde_json::to_string(&high).unwrap(), "\"high\"");
    assert!(serde_json::from_str::<Effort>("\"none\"").is_err());
}
