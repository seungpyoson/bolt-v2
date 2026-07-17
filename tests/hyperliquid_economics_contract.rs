use bolt_v2::{
    bolt_v3_providers::hyperliquid::economics::{
        BlockedUnsupported, HyperliquidCarryPolicy, HyperliquidEconomicsAdapter,
        HyperliquidEconomicsAdapterConfig, HyperliquidEconomicsError, HyperliquidFormulaPolicy,
        HyperliquidProductEconomicsSnapshot, HyperliquidSnapshotMetadata,
        HyperliquidUserFeesSnapshot,
    },
    economics::{
        EconomicClass, EconomicComponentId, FormulaId, LiquidityRoleAssumption, PositionContext,
        PositionSide, RoutingAttachment, RoutingAttachmentId, SnapshotId, SourceId,
        VenueEconomicsAdapter,
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
        carry: Some(HyperliquidCarryPolicy {
            component_id: EconomicComponentId::new("funding-carry").unwrap(),
            formula_id: FormulaId::new("funding-rate-bound").unwrap(),
            point_rate_factor_id: FormulaId::new("funding-point-rate").unwrap(),
            bound_rate_factor_id: FormulaId::new("funding-bound-rate").unwrap(),
            risk_policy_id: FormulaId::new("funding-risk-policy").unwrap(),
            stress_fixture_id: FormulaId::new("funding-stress-fixture").unwrap(),
        }),
    }
}

fn user_fees(maker_rate: &str) -> HyperliquidUserFeesSnapshot {
    user_fees_with_discounts(maker_rate, "0", "0")
}

fn official_user_fees(maker_rate: &str) -> HyperliquidUserFeesSnapshot {
    user_fees_with_discounts(maker_rate, "0.04", "0.3")
}

fn user_fees_with_discounts(
    maker_rate: &str,
    referral_discount: &str,
    staking_discount: &str,
) -> HyperliquidUserFeesSnapshot {
    HyperliquidUserFeesSnapshot::from_wire_json(
        HyperliquidSnapshotMetadata {
            snapshot_id: "user-fees-snapshot".to_string(),
            source_at_ns: 90,
            fetched_at_ns: 95,
            valid_until_ns: 110,
        },
        "account",
        &format!(
            r#"{{
                "dailyUserVlm":[{{
                    "date":"2026-07-18",
                    "userCross":"100000",
                    "userAdd":"50000",
                    "exchange":"1000000"
                }}],
                "feeSchedule":{{
                    "cross":"0.00045",
                    "add":"0.00015",
                    "spotCross":"0.0007",
                    "spotAdd":"0.0004",
                    "tiers":{{
                        "vip":[],
                        "mm":[{{"makerFractionCutoff":"0.005","add":"-0.00001"}}]
                    }},
                    "referralDiscount":"0.04",
                    "stakingDiscountTiers":[
                        {{"bpsOfMaxSupply":"0","discount":"0"}},
                        {{"bpsOfMaxSupply":"4.7577998927","discount":"{staking_discount}"}}
                    ]
                }},
                "userCrossRate":"0.000315",
                "userAddRate":"{maker_rate}",
                "userSpotCrossRate":"0.00049",
                "userSpotAddRate":"0.00028",
                "activeReferralDiscount":"{referral_discount}",
                "trial":null,
                "feeTrialReward":"0",
                "nextTrialAvailableTimestamp":null,
                "stakingLink":{{
                    "type":"tradingUser",
                    "stakingUser":"0x54c049d9c7d3c92c2462bf3d28e083f3d6805061"
                }},
                "activeStakingDiscount":{{
                    "bpsOfMaxSupply":"4.7577998927",
                    "discount":"{staking_discount}"
                }}
            }}"#
        ),
    )
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
            "baseUnit":"BTC","quoteUnit":"USDC",
            "alignedQuoteOrCollateral":{aligned},"hip3":false,"deployerScale":0,
            "growthMode":false,"builderProfileId":"builder-profile",
            "builderRateBps":{builder_rate_bps},"builderApprovedMaxBps":{builder_max_bps},
            "spotDustAuthorityComplete":{dust_complete},
            "carryPointRatePerNs":0.000000001,
            "carryDebitRateBoundPerNs":0.000000002
        }}"#
    ))
    .unwrap()
}

fn perp_request() -> bolt_v2::economics::EconomicQuoteRequest {
    let mut request = canonical_fixture_request();
    request.position = Some(PositionContext {
        side: PositionSide::Long,
        quantity: decimal("100"),
        holding_horizon_ns: 1000,
    });
    request
}

#[test]
fn spot_buy_fee_uses_base_unit_and_omits_builder_charge() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("0"),
        product("spot", false, true, "1", "2"),
    )
    .unwrap();
    let mut request = canonical_fixture_request();
    request.order_side = bolt_v2::economics::OrderSide::Buy;
    request.planned_fill_legs[0].price = decimal("100");
    request.planned_fill_legs[0].quantity = decimal("2");
    request.routing.attached_charge = Some(RoutingAttachment {
        attachment_id: RoutingAttachmentId::new("builder-profile").unwrap(),
    });

    let components = adapter.quote_components(&request).unwrap();

    assert_eq!(components.len(), 1);
    assert_eq!(components[0].point_effect.amount(), decimal("-0.0008"));
    assert_eq!(components[0].point_effect.unit().as_str(), "BTC");
}

#[test]
fn spot_sell_fee_and_builder_charge_use_quote_unit() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("0"),
        product("spot", false, true, "1", "2"),
    )
    .unwrap();
    let mut request = canonical_fixture_request();
    request.order_side = bolt_v2::economics::OrderSide::Sell;
    request.planned_fill_legs[0].price = decimal("100");
    request.planned_fill_legs[0].quantity = decimal("2");
    request.routing.attached_charge = Some(RoutingAttachment {
        attachment_id: RoutingAttachmentId::new("builder-profile").unwrap(),
    });

    let components = adapter.quote_components(&request).unwrap();

    assert_eq!(components.len(), 2);
    assert!(
        components
            .iter()
            .all(|component| component.point_effect.unit().as_str() == "USDC")
    );
}

#[test]
fn complete_perp_surface_applies_account_rate_and_builder_approval() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("-0.00001"),
        product("perp", false, false, "1", "2"),
    )
    .unwrap();
    let mut request = perp_request();
    request.planned_fill_legs[0].price = decimal("100");
    request.planned_fill_legs[0].quantity = decimal("100");
    request.routing.attached_charge = Some(RoutingAttachment {
        attachment_id: RoutingAttachmentId::new("builder-profile").unwrap(),
    });

    let components = adapter.quote_components(&request).unwrap();
    assert_eq!(components.len(), 3);
    assert_eq!(components[0].point_effect.amount(), decimal("-3.15"));
    assert_eq!(components[1].point_effect.amount(), decimal("-1.00"));
}

#[test]
fn official_user_fees_wire_shape_drives_effective_taker_rate_without_double_staking() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        official_user_fees("0.000105"),
        product("perp", false, false, "0", "0"),
    )
    .unwrap();
    let mut request = perp_request();
    request.planned_fill_legs[0].price = decimal("100");
    request.planned_fill_legs[0].quantity = decimal("100");

    let components = adapter.quote_components(&request).unwrap();

    assert_eq!(components[0].point_effect.amount(), decimal("-3.024"));
}

#[test]
fn negative_maker_rate_bypasses_referral_and_hip3_scaling() {
    let product = HyperliquidProductEconomicsSnapshot::from_json(
        r#"{
            "snapshotId":"product-snapshot","sourceAtNs":91,"fetchedAtNs":96,
            "validUntilNs":110,"productKind":"perp","stablePair":false,
            "alignedQuoteOrCollateral":false,"hip3":true,"deployerScale":0.5,
            "growthMode":false,"builderProfileId":"builder-profile",
            "builderRateBps":0,"builderApprovedMaxBps":0,
            "spotDustAuthorityComplete":false,
            "carryPointRatePerNs":0.000000001,
            "carryDebitRateBoundPerNs":0.000000002
        }"#,
    )
    .unwrap();
    let adapter =
        HyperliquidEconomicsAdapter::try_new(config(), official_user_fees("-0.00001"), product)
            .unwrap();
    let mut request = perp_request();
    request.liquidity_role = LiquidityRoleAssumption::GuaranteedMaker;
    request.planned_fill_legs[0].price = decimal("100");
    request.planned_fill_legs[0].quantity = decimal("500");

    let components = adapter.quote_components(&request).unwrap();

    assert_eq!(components[0].point_effect.amount(), decimal("0.50"));
}

#[test]
fn sealed_quote_evidence_names_account_and_product_snapshots() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("0"),
        product("perp", false, false, "0", "0"),
    )
    .unwrap();

    let estimate = adapter.quote(&perp_request()).unwrap();

    assert_eq!(
        estimate.authority.snapshot_id,
        SnapshotId::new("user-fees-snapshot").unwrap()
    );
    assert_eq!(
        estimate
            .dependency_sources
            .iter()
            .map(|source| source.snapshot_id.as_str())
            .collect::<Vec<_>>(),
        vec!["product-snapshot"]
    );
}

#[test]
fn negative_maker_rate_is_guaranteed_credit_not_forecast_reward() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("-0.00001"),
        product("perp", false, false, "0", "0"),
    )
    .unwrap();
    let mut request = perp_request();
    request.liquidity_role = LiquidityRoleAssumption::GuaranteedMaker;
    request.planned_fill_legs[0].price = decimal("100");
    request.planned_fill_legs[0].quantity = decimal("500");

    let components = adapter.quote_components(&request).unwrap();
    assert_eq!(components[0].point_effect.amount(), decimal("0.50"));
    assert_eq!(components[0].class, EconomicClass::Credit);
    assert!(components[0].authorizes_admission());
}

#[test]
fn perp_without_horizon_or_debit_bound_fails_closed() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("0"),
        product("perp", false, false, "0", "0"),
    )
    .unwrap();
    assert_eq!(
        adapter.quote_components(&canonical_fixture_request()),
        Err(HyperliquidEconomicsError::MissingCarryContext)
    );

    let missing_bound = HyperliquidProductEconomicsSnapshot::from_json(
        &serde_json::to_string(&serde_json::json!({
            "snapshotId": "product-snapshot",
            "sourceAtNs": 91,
            "fetchedAtNs": 96,
            "validUntilNs": 110,
            "productKind": "perp",
            "baseUnit": "BTC",
            "quoteUnit": "USDC",
            "stablePair": false,
            "alignedQuoteOrCollateral": false,
            "hip3": false,
            "deployerScale": 0,
            "growthMode": false,
            "builderProfileId": "builder-profile",
            "builderRateBps": 0,
            "builderApprovedMaxBps": 0,
            "spotDustAuthorityComplete": false,
            "carryPointRatePerNs": 0.000000001
        }))
        .unwrap(),
    )
    .unwrap();
    let adapter =
        HyperliquidEconomicsAdapter::try_new(config(), user_fees("0"), missing_bound).unwrap();
    assert_eq!(
        adapter.quote_components(&perp_request()),
        Err(HyperliquidEconomicsError::MissingCarryPolicy)
    );
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
        "dailyUserVlm":[],
        "userCrossRate":"0.000315"
    }"#;
    assert_eq!(
        HyperliquidUserFeesSnapshot::from_wire_json(
            HyperliquidSnapshotMetadata {
                snapshot_id: "user-fees-snapshot".to_string(),
                source_at_ns: 90,
                fetched_at_ns: 95,
                valid_until_ns: 110,
            },
            "account",
            incomplete,
        ),
        Err(HyperliquidEconomicsError::InvalidUserFees)
    );
}
