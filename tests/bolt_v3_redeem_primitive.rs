use bolt_v2::bolt_v3_providers::polymarket::redemption::{MECHANICALLY_ENABLED, validate_profile};

#[test]
fn standard_and_negative_risk_fixtures() {
    let standard: toml::Value =
        toml::from_str(include_str!("fixtures/bolt_v3/redeem/standard.toml")).unwrap();
    let negative: toml::Value =
        toml::from_str(include_str!("fixtures/bolt_v3/redeem/negative-risk.toml")).unwrap();
    assert_eq!(standard["market_mode"].as_str(), Some("standard"));
    assert_eq!(negative["market_mode"].as_str(), Some("negative_risk"));
    for fixture in [&standard, &negative] {
        let edges = fixture["scaled_integer_edges"].as_array().unwrap();
        assert_eq!(edges.first().and_then(toml::Value::as_str), Some("0"));
        assert_eq!(edges.get(1).and_then(toml::Value::as_str), Some("1"));
        assert_eq!(
            edges.last().and_then(toml::Value::as_str),
            Some("340282366920938463463374607431768211455")
        );
    }
}

#[test]
fn primitive_is_mechanically_disabled() {
    validate_profile().unwrap();
    assert!(!MECHANICALLY_ENABLED);
}
