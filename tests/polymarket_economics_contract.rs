use bolt_v2::{
    bolt_v3_providers::polymarket::economics::{
        FeeRoundingMode, NtFeeProjection, PolymarketEconomicsAdapter,
        PolymarketEconomicsAdapterConfig, PolymarketEconomicsError, PolymarketFormulaPolicy,
        PolymarketMarketInfoSnapshot, PolymarketSnapshotMetadata,
    },
    economics::{
        EconomicComponentId, FormulaId, LiquidityRoleAssumption, PlannedFillLeg, RoutingAttachment,
        RoutingAttachmentId, SourceId, VenueEconomicsAdapter, validate_and_aggregate_quote,
    },
};

use super::economics_support::{canonical_fixture_request, decimal, native_unit};

fn config() -> PolymarketEconomicsAdapterConfig {
    PolymarketEconomicsAdapterConfig {
        collateral_unit: native_unit("pUSD"),
        platform_component_id: EconomicComponentId::new("platform-fee").unwrap(),
        platform_formula_id: FormulaId::new("platform-formula").unwrap(),
        platform_rate_factor_id: FormulaId::new("platform-rate").unwrap(),
        builder_component_id: EconomicComponentId::new("builder-fee").unwrap(),
        builder_formula_id: FormulaId::new("builder-formula").unwrap(),
        builder_rate_factor_id: FormulaId::new("builder-rate").unwrap(),
        source_id: SourceId::new("clob-market-info").unwrap(),
        formula: PolymarketFormulaPolicy {
            fee_round_decimal_places: 5,
            fee_rounding_mode: FeeRoundingMode::MidpointAwayFromZero,
        },
    }
}

#[test]
fn nonlinear_fee_is_rounded_and_summed_per_planned_fill_level() {
    let adapter = PolymarketEconomicsAdapter::try_new(config(), snapshot(true, 1), None).unwrap();
    let mut request = canonical_fixture_request();
    request.planned_fill_legs = vec![
        PlannedFillLeg {
            price: decimal("0.60"),
            quantity: decimal("5"),
        },
        PlannedFillLeg {
            price: decimal("0.80"),
            quantity: decimal("5"),
        },
    ];

    let components = adapter.quote_components(&request).unwrap();

    assert_eq!(
        components[0].point_estimate.effect().unwrap().amount(),
        decimal("-0.14000")
    );
    assert_ne!(
        components[0].point_estimate.effect().unwrap().amount(),
        decimal("-0.14700")
    );
}

fn snapshot(fees_enabled: bool, exponent: u32) -> PolymarketMarketInfoSnapshot {
    let economics = if fees_enabled {
        format!(r#","mbf":1000,"tbf":1000,"fd":{{"r":0.07,"e":{exponent},"to":true}}"#)
    } else {
        String::new()
    };
    PolymarketMarketInfoSnapshot::from_wire_json(
        metadata(),
        &format!(
            r#"{{"r":{{}},"t":[{{"t":"token-yes","o":"Yes"}}],"mos":5,"mts":0.001,"ibce":true{economics}}}"#
        ),
    )
    .unwrap()
}

fn metadata() -> PolymarketSnapshotMetadata {
    PolymarketSnapshotMetadata {
        snapshot_id: "market-snapshot".to_string(),
        source_at_ns: 90,
        fetched_at_ns: 95,
        valid_until_ns: 110,
    }
}

#[test]
fn taker_formula_matches_authoritative_price_shaped_example() {
    let adapter = PolymarketEconomicsAdapter::try_new(config(), snapshot(true, 1), None).unwrap();
    let mut request = canonical_fixture_request();
    request.planned_fill_legs[0].quantity = decimal("100");

    let components = adapter.quote_components(&request).unwrap();
    assert_eq!(components.len(), 1);
    assert_eq!(
        components[0].point_estimate.effect().unwrap().amount(),
        decimal("-1.75")
    );
    assert_eq!(
        components[0]
            .point_estimate
            .effect()
            .unwrap()
            .unit()
            .as_str(),
        "pUSD"
    );
}

#[test]
fn attached_builder_charge_without_profile_authority_fails_closed() {
    let adapter = PolymarketEconomicsAdapter::try_new(config(), snapshot(true, 1), None).unwrap();
    let mut request = canonical_fixture_request();
    request.liquidity_role = LiquidityRoleAssumption::GuaranteedMaker;
    request.planned_fill_legs[0].quantity = decimal("200");
    request.routing.attached_charge = Some(RoutingAttachment {
        attachment_id: RoutingAttachmentId::new("builder-profile").unwrap(),
    });

    assert_eq!(
        adapter.quote_components(&request),
        Err(PolymarketEconomicsError::MissingBuilderDescriptor)
    );
}

#[test]
fn authoritative_fee_free_snapshot_emits_no_zero_component() {
    let adapter = PolymarketEconomicsAdapter::try_new(config(), snapshot(false, 1), None).unwrap();
    let request = canonical_fixture_request();
    let estimate = adapter.quote(&request).unwrap();
    assert!(estimate.components.is_empty());
    let quote = validate_and_aggregate_quote(&request, estimate, &[]).unwrap();
    assert!(quote.components().is_empty());
    assert!(quote.core_total().is_zero());
}

#[test]
fn unsupported_descriptor_and_projection_disagreement_fail_closed() {
    assert!(matches!(
        PolymarketEconomicsAdapter::try_new(config(), snapshot(true, 3), None),
        Err(PolymarketEconomicsError::UnsupportedExponent)
    ));

    let disagreement = PolymarketEconomicsAdapter::try_new(
        config(),
        snapshot(true, 1),
        Some(NtFeeProjection {
            fees_enabled: true,
            rate: decimal("0.08"),
            exponent: 1,
            taker_only: true,
        }),
    );
    assert!(matches!(
        disagreement,
        Err(PolymarketEconomicsError::NtProjectionDisagreement)
    ));
}

#[test]
fn documented_exponent_two_descriptor_applies_the_exponent_to_the_price_shape() {
    let adapter = PolymarketEconomicsAdapter::try_new(config(), snapshot(true, 2), None).unwrap();
    let mut request = canonical_fixture_request();
    request.planned_fill_legs[0].quantity = decimal("100");

    let components = adapter.quote_components(&request).unwrap();
    assert_eq!(
        components[0].point_estimate.effect().unwrap().amount(),
        decimal("-0.43750")
    );
}

#[test]
fn market_info_parser_rejects_missing_or_unknown_economics_shape() {
    let missing = r#"{
        "r":{},"t":[{"t":"token-yes","o":"Yes"}],"mos":5,"mts":0.001,
        "mbf":1000,"tbf":1000,"ibce":true
    }"#;
    let snapshot = PolymarketMarketInfoSnapshot::from_wire_json(metadata(), missing).unwrap();
    assert!(matches!(
        PolymarketEconomicsAdapter::try_new(config(), snapshot, None),
        Err(PolymarketEconomicsError::InvalidMarketInfo)
    ));

    let unknown = r#"{
        "r":{},"t":[{"t":"token-yes","o":"Yes"}],"mos":5,"mts":0.001,
        "ibce":true,
        "unreviewedField":1
    }"#;
    assert_eq!(
        PolymarketMarketInfoSnapshot::from_wire_json(metadata(), unknown),
        Err(PolymarketEconomicsError::InvalidMarketInfo)
    );
}

#[test]
fn market_info_rejects_base_fee_values_outside_the_governed_descriptor_shape() {
    let contradictory = r#"{
        "r":{},"t":[{"t":"token-yes","o":"Yes"}],"mos":5,"mts":0.001,
        "mbf":999,"tbf":1000,"ibce":true,
        "fd":{"r":0.07,"e":1,"to":true}
    }"#;
    let snapshot = PolymarketMarketInfoSnapshot::from_wire_json(metadata(), contradictory).unwrap();

    assert!(matches!(
        PolymarketEconomicsAdapter::try_new(config(), snapshot, None),
        Err(PolymarketEconomicsError::InvalidMarketInfo)
    ));
}

#[test]
fn governed_live_market_info_captures_parse_fee_bearing_and_fee_free_shapes() {
    let fee_bearing =
        include_str!("fixtures/bolt_v3/boundary_evidence/polymarket-market-info-fee-bearing.json");
    let adapter = PolymarketEconomicsAdapter::try_new(
        config(),
        PolymarketMarketInfoSnapshot::from_wire_json(metadata(), fee_bearing).unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(
        adapter
            .quote_components(&canonical_fixture_request())
            .unwrap()
            .len(),
        1
    );

    let fee_free =
        include_str!("fixtures/bolt_v3/boundary_evidence/polymarket-market-info-fee-free.json");
    let adapter = PolymarketEconomicsAdapter::try_new(
        config(),
        PolymarketMarketInfoSnapshot::from_wire_json(metadata(), fee_free).unwrap(),
        None,
    )
    .unwrap();
    assert!(
        adapter
            .quote_components(&canonical_fixture_request())
            .unwrap()
            .is_empty()
    );
}
