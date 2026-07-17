use bolt_v2::{
    bolt_v3_providers::polymarket::economics::{
        FeeRoundingMode, NtFeeProjection, PolymarketEconomicsAdapter,
        PolymarketEconomicsAdapterConfig, PolymarketEconomicsError, PolymarketFormulaPolicy,
        PolymarketMarketInfoSnapshot,
    },
    economics::{
        EconomicComponentId, FormulaId, LiquidityRoleAssumption, RoutingAttachment,
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
            fee_round_decimal_places: 6,
            fee_rounding_mode: FeeRoundingMode::MidpointAwayFromZero,
        },
    }
}

fn snapshot(fees_enabled: bool, exponent: u32) -> PolymarketMarketInfoSnapshot {
    PolymarketMarketInfoSnapshot::from_json(&format!(
        r#"{{
            "snapshotId":"market-snapshot",
            "sourceAtNs":90,
            "fetchedAtNs":95,
            "validUntilNs":110,
            "feesEnabled":{fees_enabled},
            "fd":{{"r":0.07,"e":{exponent},"to":true}},
            "builder":{{"profileId":"builder-profile","makerRateBps":30,"takerRateBps":10}}
        }}"#
    ))
    .unwrap()
}

#[test]
fn taker_formula_matches_authoritative_price_shaped_example() {
    let adapter = PolymarketEconomicsAdapter::try_new(config(), snapshot(true, 1), None).unwrap();
    let mut request = canonical_fixture_request();
    request.planned_fill_legs[0].quantity = decimal("100");

    let components = adapter.quote_components(&request).unwrap();
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].point_effect.amount(), decimal("-1.75"));
    assert_eq!(components[0].point_effect.unit().as_str(), "pUSD");
}

#[test]
fn taker_only_platform_fee_does_not_hide_attached_maker_builder_charge() {
    let adapter = PolymarketEconomicsAdapter::try_new(config(), snapshot(true, 1), None).unwrap();
    let mut request = canonical_fixture_request();
    request.liquidity_role = LiquidityRoleAssumption::GuaranteedMaker;
    request.planned_fill_legs[0].quantity = decimal("200");
    request.routing.attached_charge = Some(RoutingAttachment {
        attachment_id: RoutingAttachmentId::new("builder-profile").unwrap(),
    });

    let components = adapter.quote_components(&request).unwrap();
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].point_effect.amount(), decimal("-0.30"));
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
        PolymarketEconomicsAdapter::try_new(config(), snapshot(true, 2), None),
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
fn market_info_parser_rejects_missing_or_unknown_economics_shape() {
    let missing = r#"{
        "snapshotId":"market-snapshot","sourceAtNs":90,"fetchedAtNs":95,
        "validUntilNs":110,"feesEnabled":true,"builder":null
    }"#;
    let snapshot = PolymarketMarketInfoSnapshot::from_json(missing).unwrap();
    assert!(matches!(
        PolymarketEconomicsAdapter::try_new(config(), snapshot, None),
        Err(PolymarketEconomicsError::MissingFeeDescriptor)
    ));

    let unknown = r#"{
        "snapshotId":"market-snapshot","sourceAtNs":90,"fetchedAtNs":95,
        "validUntilNs":110,"feesEnabled":false,"fd":null,"builder":null,
        "unreviewedField":1
    }"#;
    assert_eq!(
        PolymarketMarketInfoSnapshot::from_json(unknown),
        Err(PolymarketEconomicsError::InvalidMarketInfo)
    );
}
