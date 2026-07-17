use bolt_v2::{
    bolt_v3_providers::hyperliquid::economics::{
        BlockedUnsupported, HyperliquidEconomicsAdapter, HyperliquidEconomicsAdapterConfig,
        HyperliquidEconomicsError, HyperliquidFormulaPolicy, HyperliquidProductEconomicsSnapshot,
        HyperliquidUserFeesSnapshot,
    },
    economics::{
        EconomicClass, EconomicComponentId, FormulaId, LiquidityRoleAssumption, RoutingAttachment,
        RoutingAttachmentId, SourceId,
    },
};

use super::economics_support::{canonical_fixture_request, decimal, native_unit};

fn config() -> HyperliquidEconomicsAdapterConfig {
    HyperliquidEconomicsAdapterConfig {
        settlement_unit: native_unit("USDC"),
        protocol_component_id: EconomicComponentId::new("protocol-fee").unwrap(),
        protocol_formula_id: FormulaId::new("developer-formula").unwrap(),
        protocol_rate_factor_id: FormulaId::new("effective-rate").unwrap(),
        builder_component_id: EconomicComponentId::new("builder-fee").unwrap(),
        builder_formula_id: FormulaId::new("builder-formula").unwrap(),
        builder_rate_factor_id: FormulaId::new("builder-rate").unwrap(),
        source_id: SourceId::new("user-fees-and-product").unwrap(),
        formula: HyperliquidFormulaPolicy {
            stable_pair_scale: decimal("1"),
            growth_mode_scale: decimal("1"),
            hip3_scale_threshold: decimal("1"),
            hip3_below_threshold_base: decimal("1"),
            hip3_at_or_above_threshold_multiplier: decimal("1"),
            hip3_at_or_above_deployer_share: decimal("0"),
        },
    }
}

fn user_fees(maker_rate: &str) -> HyperliquidUserFeesSnapshot {
    HyperliquidUserFeesSnapshot::from_json(&format!(
        r#"{{
            "snapshotId":"user-fees-snapshot","accountId":"account",
            "sourceAtNs":90,"fetchedAtNs":95,"validUntilNs":110,
            "feeTier":"tier-1","dailyUserVolume":100000,
            "activeReferralDiscount":0,"activeStakingDiscount":0,"trialCredits":0,
            "perpTakerBaseRate":0.000315,"perpMakerBaseRate":{maker_rate},
            "spotTakerBaseRate":0.0004,"spotMakerBaseRate":0.0001
        }}"#
    ))
    .unwrap()
}

fn product(
    kind: &str,
    aligned: bool,
    dust_complete: bool,
    builder_rate_bps: &str,
    builder_max_bps: &str,
) -> HyperliquidProductEconomicsSnapshot {
    HyperliquidProductEconomicsSnapshot::from_json(&format!(
        r#"{{
            "snapshotId":"product-snapshot","sourceAtNs":91,"fetchedAtNs":96,
            "validUntilNs":110,"productKind":"{kind}","stablePair":false,
            "alignedQuoteOrCollateral":{aligned},"hip3":false,"deployerScale":0,
            "growthMode":false,"builderProfileId":"builder-profile",
            "builderRateBps":{builder_rate_bps},"builderApprovedMaxBps":{builder_max_bps},
            "spotDustAuthorityComplete":{dust_complete}
        }}"#
    ))
    .unwrap()
}

#[test]
fn complete_perp_surface_applies_account_rate_and_builder_approval() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("-0.00001"),
        product("perp", false, false, "1", "2"),
    )
    .unwrap();
    let mut request = canonical_fixture_request();
    request.planned_fill_legs[0].price = decimal("100");
    request.planned_fill_legs[0].quantity = decimal("100");
    request.routing.attached_charge = Some(RoutingAttachment {
        attachment_id: RoutingAttachmentId::new("builder-profile").unwrap(),
    });

    let components = adapter.quote_components(&request).unwrap();
    assert_eq!(components.len(), 2);
    assert_eq!(components[0].point_effect.amount(), decimal("-3.15"));
    assert_eq!(components[1].point_effect.amount(), decimal("-1.00"));
}

#[test]
fn negative_maker_rate_is_guaranteed_credit_not_forecast_reward() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("-0.00001"),
        product("perp", false, false, "0", "0"),
    )
    .unwrap();
    let mut request = canonical_fixture_request();
    request.liquidity_role = LiquidityRoleAssumption::GuaranteedMaker;
    request.planned_fill_legs[0].price = decimal("100");
    request.planned_fill_legs[0].quantity = decimal("500");

    let components = adapter.quote_components(&request).unwrap();
    assert_eq!(components[0].point_effect.amount(), decimal("0.50"));
    assert_eq!(components[0].class, EconomicClass::Credit);
    assert!(components[0].authorizes_admission());
}

#[test]
fn aligned_and_unproved_spot_surfaces_are_explicitly_blocked() {
    let aligned = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("0"),
        product("perp", true, false, "0", "0"),
    );
    assert_eq!(
        aligned.err(),
        Some(HyperliquidEconomicsError::BlockedUnsupported(
            BlockedUnsupported::MissingGovernedAlignedStatusCapture
        ))
    );

    let spot = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("0"),
        product("spot", false, false, "0", "0"),
    );
    assert_eq!(
        spot.err(),
        Some(HyperliquidEconomicsError::BlockedUnsupported(
            BlockedUnsupported::SpotDustAuthorityIncomplete
        ))
    );
}

#[test]
fn builder_rate_above_account_approval_fails_closed() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("0"),
        product("perp", false, false, "3", "2"),
    );
    assert_eq!(
        adapter.err(),
        Some(HyperliquidEconomicsError::BuilderApprovalExceeded)
    );
}

#[test]
fn negative_hip3_deployer_scale_fails_closed() {
    let product = HyperliquidProductEconomicsSnapshot::from_json(
        r#"{
            "snapshotId":"product-snapshot","sourceAtNs":91,"fetchedAtNs":96,
            "validUntilNs":110,"productKind":"perp","stablePair":false,
            "alignedQuoteOrCollateral":false,"hip3":true,"deployerScale":-0.1,
            "growthMode":false,"builderProfileId":"builder-profile",
            "builderRateBps":0,"builderApprovedMaxBps":0,
            "spotDustAuthorityComplete":false
        }"#,
    )
    .unwrap();

    assert_eq!(
        HyperliquidEconomicsAdapter::try_new(config(), user_fees("0"), product).err(),
        Some(HyperliquidEconomicsError::InvalidFeeSurface)
    );
}

#[test]
fn user_fees_parser_requires_complete_account_surface() {
    let incomplete = r#"{
        "snapshotId":"user-fees-snapshot","accountId":"account",
        "sourceAtNs":90,"fetchedAtNs":95,"validUntilNs":110,
        "feeTier":"tier-1","dailyUserVolume":100000,
        "perpTakerBaseRate":0.000315
    }"#;
    assert_eq!(
        HyperliquidUserFeesSnapshot::from_json(incomplete),
        Err(HyperliquidEconomicsError::InvalidUserFees)
    );
}
