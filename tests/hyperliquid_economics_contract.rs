use bolt_v2::{
    bolt_v3_providers::hyperliquid::economics::{
        BlockedUnsupported, HyperliquidCarryPolicy, HyperliquidEconomicsAdapter,
        HyperliquidEconomicsAdapterConfig, HyperliquidEconomicsError,
        HyperliquidFeeEligibilityPolicy, HyperliquidFormulaPolicy,
        HyperliquidProductEconomicsSnapshot, HyperliquidSnapshotMetadata,
        HyperliquidUserFeesSnapshot,
    },
    economics::{
        EconomicClass, EconomicComponentId, FormulaId, LiquidityRoleAssumption, PositionContext,
        PositionSide, RoutingAttachment, RoutingAttachmentId, SnapshotId, SourceId,
        VenueEconomicsAdapter,
    },
};
use std::num::NonZeroUsize;

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
        fee_eligibility: fee_eligibility_policy(1, 1),
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
            oracle_price_factor_id: FormulaId::new("funding-oracle-price").unwrap(),
            next_funding_at_factor_id: FormulaId::new("funding-next-event-at").unwrap(),
            funding_interval_ns: 3_600_000_000_000,
            funding_schedule_phase_ns: 900_000_000_000,
            venue_rate_cap_fraction: decimal("0.04"),
            standard_price_stress_multiplier: decimal("1.5"),
        }),
    }
}

fn fee_eligibility_policy(
    history_days: usize,
    rolling_window_days: usize,
) -> HyperliquidFeeEligibilityPolicy {
    HyperliquidFeeEligibilityPolicy {
        history_days: NonZeroUsize::new(history_days).unwrap(),
        rolling_window_days: NonZeroUsize::new(rolling_window_days).unwrap(),
        latest_day_offset_days: 0,
    }
}

fn governed_fee_eligibility_policy() -> HyperliquidFeeEligibilityPolicy {
    HyperliquidFeeEligibilityPolicy {
        history_days: NonZeroUsize::new(15).unwrap(),
        rolling_window_days: NonZeroUsize::new(14).unwrap(),
        latest_day_offset_days: 1,
    }
}

fn governed_user_fees_metadata() -> HyperliquidSnapshotMetadata {
    HyperliquidSnapshotMetadata {
        snapshot_id: "governed-live-user-fees".to_string(),
        source_at_ns: 1_784_332_800_000_000_000,
        fetched_at_ns: 1_784_332_800_000_000_000,
        valid_until_ns: 1_784_332_860_000_000_000,
    }
}

fn user_fees(maker_rate: &str) -> HyperliquidUserFeesSnapshot {
    user_fees_with_discounts(maker_rate, "0", "0", "0.00045", "0.0007", "0.0004")
}

fn official_user_fees(maker_rate: &str) -> HyperliquidUserFeesSnapshot {
    user_fees_with_discounts(maker_rate, "0.04", "0.3", "0.000315", "0.00049", "0.00028")
}

fn user_fees_with_discounts(
    maker_rate: &str,
    referral_discount: &str,
    staking_discount: &str,
    perp_taker_rate: &str,
    spot_taker_rate: &str,
    spot_maker_rate: &str,
) -> HyperliquidUserFeesSnapshot {
    let (base_maker_rate, maker_volume, maker_tiers) = if maker_rate.starts_with('-') {
        (
            "0.00015",
            "10000",
            r#"[{"makerFractionCutoff":"0.005","add":"-0.00001"}]"#,
        )
    } else if maker_rate == "0" {
        ("0", "0", "[]")
    } else {
        ("0.00015", "0", "[]")
    };
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
                    "date":"1970-01-01",
                    "userCross":"100000",
                    "userAdd":"{maker_volume}",
                    "exchange":"1000000"
                }}],
                "feeSchedule":{{
                    "cross":"0.00045",
                    "add":"{base_maker_rate}",
                    "spotCross":"0.0007",
                    "spotAdd":"0.0004",
                    "tiers":{{
                        "vip":[],
                        "mm":{maker_tiers}
                    }},
                    "referralDiscount":"0.04",
                    "stakingDiscountTiers":[
                        {{"bpsOfMaxSupply":"0","discount":"0"}},
                        {{"bpsOfMaxSupply":"4.7577998927","discount":"{staking_discount}"}}
                    ]
                }},
                "userCrossRate":"{perp_taker_rate}",
                "userAddRate":"{maker_rate}",
                "userSpotCrossRate":"{spot_taker_rate}",
                "userSpotAddRate":"{spot_maker_rate}",
                "activeReferralDiscount":"{referral_discount}",
                "trial":null,
                "feeTrialEscrow":"0",
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
        &fee_eligibility_policy(1, 1),
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
            "carryOraclePrice":100,
            "carryPointRatePerInterval":0.001,
            "carryDebitRateBoundPerInterval":0.002,
            "carryNextFundingAtNs":500
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
    assert_eq!(
        components[0].point_estimate.effect().unwrap().amount(),
        decimal("-0.0008")
    );
    assert_eq!(
        components[0]
            .point_estimate
            .effect()
            .unwrap()
            .unit()
            .as_str(),
        "BTC"
    );
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
        components.iter().all(|component| component
            .point_estimate
            .effect()
            .unwrap()
            .unit()
            .as_str()
            == "USDC")
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
    assert_eq!(
        components[0].point_estimate.effect().unwrap().amount(),
        decimal("-3.15")
    );
    assert_eq!(
        components[1].point_estimate.effect().unwrap().amount(),
        decimal("-1.00")
    );
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

    assert_eq!(
        components[0].point_estimate.effect().unwrap().amount(),
        decimal("-3.024")
    );
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
            "carryOraclePrice":100,
            "carryPointRatePerInterval":0.001,
            "carryDebitRateBoundPerInterval":0.002,
            "carryNextFundingAtNs":500
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

    assert_eq!(
        components[0].point_estimate.effect().unwrap().amount(),
        decimal("0.50")
    );
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
    assert_eq!(
        components[0].point_estimate.effect().unwrap().amount(),
        decimal("0.50")
    );
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
            "carryOraclePrice": 100,
            "carryPointRatePerInterval": 0.001,
            "carryNextFundingAtNs": 500
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
fn funding_bound_counts_intersected_events_at_the_exact_boundary() {
    let adapter = HyperliquidEconomicsAdapter::try_new(
        config(),
        user_fees("0"),
        product("perp", false, false, "0", "0"),
    )
    .unwrap();
    let mut before = perp_request();
    before.position.as_mut().unwrap().holding_horizon_ns = 399;
    assert_eq!(adapter.quote_components(&before).unwrap().len(), 1);

    let mut at = perp_request();
    at.position.as_mut().unwrap().holding_horizon_ns = 400;
    let components = adapter.quote_components(&at).unwrap();
    let carry = components
        .iter()
        .find(|component| component.component_id.as_str() == "funding-carry")
        .unwrap();
    assert_eq!(
        carry.debit_risk_bound.as_ref().unwrap().amount(),
        decimal("-30")
    );
    assert!(carry.point_estimate.effect().is_some());
}

#[test]
fn zero_point_funding_still_seals_the_venue_debit_bound() {
    let product = HyperliquidProductEconomicsSnapshot::from_json(
        r#"{
            "snapshotId":"product-snapshot","sourceAtNs":91,"fetchedAtNs":96,
            "validUntilNs":110,"productKind":"perp","stablePair":false,
            "baseUnit":"BTC","quoteUnit":"USDC","alignedQuoteOrCollateral":false,
            "hip3":false,"deployerScale":0,"growthMode":false,
            "builderProfileId":"builder-profile","builderRateBps":0,
            "builderApprovedMaxBps":0,"spotDustAuthorityComplete":false,
            "carryOraclePrice":100,"carryPointRatePerInterval":0,
            "carryDebitRateBoundPerInterval":0.002,"carryNextFundingAtNs":500
        }"#,
    )
    .unwrap();
    let adapter = HyperliquidEconomicsAdapter::try_new(config(), user_fees("0"), product).unwrap();
    let mut request = perp_request();
    request.position.as_mut().unwrap().holding_horizon_ns = 400;

    let components = adapter.quote_components(&request).unwrap();
    let carry = components
        .iter()
        .find(|component| component.component_id.as_str() == "funding-carry")
        .unwrap();
    assert_eq!(
        carry.debit_risk_bound.as_ref().unwrap().amount(),
        decimal("-30")
    );
    assert!(carry.point_estimate.effect().is_none());
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
            &fee_eligibility_policy(1, 1),
        ),
        Err(HyperliquidEconomicsError::InvalidUserFees)
    );
}

#[test]
fn governed_live_hyperliquid_quote_authority_captures_parse() {
    let product_metadata = HyperliquidSnapshotMetadata {
        snapshot_id: "governed-live-user-fees".to_string(),
        source_at_ns: 1_000_000_000_000,
        fetched_at_ns: 1_000_000_000_005,
        valid_until_ns: 1_000_000_000_110,
    };
    HyperliquidUserFeesSnapshot::from_wire_json(
        governed_user_fees_metadata(),
        "governed-public-fixture-account",
        include_str!("fixtures/bolt_v3/boundary_evidence/hyperliquid-user-fees.json"),
        &governed_fee_eligibility_policy(),
    )
    .unwrap();
    let product = HyperliquidProductEconomicsSnapshot::from_perp_meta_wire(
        product_metadata,
        include_bytes!("fixtures/bolt_v3/boundary_evidence/hyperliquid-meta-and-asset-ctxs.json"),
        "BTC",
        config().carry.as_ref().unwrap(),
    )
    .unwrap();
    assert_eq!(product.carry_next_funding_at_ns(), Some(4_500_000_000_000));
}

#[test]
fn user_fees_parser_rejects_unconfigured_volume_history_rows() {
    let mut wire: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/bolt_v3/boundary_evidence/hyperliquid-user-fees.json"
    ))
    .unwrap();
    let rows = wire["dailyUserVlm"].as_array_mut().unwrap();
    let mut extra = rows[0].clone();
    extra["date"] = serde_json::Value::String("2026-07-02".to_string());
    rows.insert(0, extra);

    assert_eq!(
        HyperliquidUserFeesSnapshot::from_wire_json(
            governed_user_fees_metadata(),
            "account",
            &serde_json::to_string(&wire).unwrap(),
            &governed_fee_eligibility_policy(),
        ),
        Err(HyperliquidEconomicsError::InvalidUserFees)
    );
}

#[test]
fn user_fees_parser_excludes_history_outside_the_configured_rolling_window() {
    let mut wire: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/bolt_v3/boundary_evidence/hyperliquid-user-fees.json"
    ))
    .unwrap();
    wire["dailyUserVlm"][0]["userCross"] = serde_json::Value::String("5000001".to_string());

    HyperliquidUserFeesSnapshot::from_wire_json(
        governed_user_fees_metadata(),
        "account",
        &serde_json::to_string(&wire).unwrap(),
        &governed_fee_eligibility_policy(),
    )
    .unwrap();
}

#[test]
fn user_fees_parser_rejects_duplicate_or_gapped_volume_dates() {
    for invalid_date in ["2026-07-03", "2026-07-05"] {
        let mut wire: serde_json::Value = serde_json::from_str(include_str!(
            "fixtures/bolt_v3/boundary_evidence/hyperliquid-user-fees.json"
        ))
        .unwrap();
        wire["dailyUserVlm"][1]["date"] = serde_json::Value::String(invalid_date.to_string());

        assert_eq!(
            HyperliquidUserFeesSnapshot::from_wire_json(
                governed_user_fees_metadata(),
                "account",
                &serde_json::to_string(&wire).unwrap(),
                &governed_fee_eligibility_policy(),
            ),
            Err(HyperliquidEconomicsError::InvalidUserFees)
        );
    }
}

#[test]
fn user_fees_parser_rejects_old_but_consecutive_volume_history() {
    let metadata = HyperliquidSnapshotMetadata {
        snapshot_id: "user-fees-snapshot".to_string(),
        source_at_ns: 1_786_924_800_000_000_000,
        fetched_at_ns: 1_786_924_800_000_000_000,
        valid_until_ns: 1_786_924_860_000_000_000,
    };

    assert_eq!(
        HyperliquidUserFeesSnapshot::from_wire_json(
            metadata,
            "account",
            include_str!("fixtures/bolt_v3/boundary_evidence/hyperliquid-user-fees.json"),
            &governed_fee_eligibility_policy(),
        ),
        Err(HyperliquidEconomicsError::InvalidUserFees)
    );
}

#[test]
fn user_fees_parser_requires_the_highest_eligible_maker_tier() {
    let mut wire: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/bolt_v3/boundary_evidence/hyperliquid-user-fees.json"
    ))
    .unwrap();
    for row in wire["dailyUserVlm"].as_array_mut().unwrap() {
        row["userAdd"] = serde_json::Value::String("20000".to_string());
        row["exchange"] = serde_json::Value::String("1000000".to_string());
    }
    wire["userAddRate"] = serde_json::Value::String("-0.00001".to_string());

    assert_eq!(
        HyperliquidUserFeesSnapshot::from_wire_json(
            governed_user_fees_metadata(),
            "account",
            &serde_json::to_string(&wire).unwrap(),
            &governed_fee_eligibility_policy(),
        ),
        Err(HyperliquidEconomicsError::InvalidUserFees)
    );
}

#[test]
fn user_fees_parser_rejects_duplicate_or_non_monotonic_maker_tiers() {
    for (field, invalid_value) in [("makerFractionCutoff", "0.005"), ("add", "-0.000005")] {
        let mut wire: serde_json::Value = serde_json::from_str(include_str!(
            "fixtures/bolt_v3/boundary_evidence/hyperliquid-user-fees.json"
        ))
        .unwrap();
        wire["feeSchedule"]["tiers"]["mm"][1][field] =
            serde_json::Value::String(invalid_value.to_string());

        assert_eq!(
            HyperliquidUserFeesSnapshot::from_wire_json(
                governed_user_fees_metadata(),
                "account",
                &serde_json::to_string(&wire).unwrap(),
                &governed_fee_eligibility_policy(),
            ),
            Err(HyperliquidEconomicsError::InvalidUserFees)
        );
    }
}

#[test]
fn user_fees_parser_rejects_effective_rate_that_disagrees_with_schedule() {
    let json = serde_json::to_string(&serde_json::json!({
        "dailyUserVlm": [{
            "date": "1970-01-01",
            "userCross": "100000",
            "userAdd": "50000",
            "exchange": "1000000"
        }],
        "feeSchedule": {
            "cross": "0.00045",
            "add": "0.00015",
            "spotCross": "0.0007",
            "spotAdd": "0.0004",
            "tiers": {"vip": [], "mm": []},
            "referralDiscount": "0.04",
            "stakingDiscountTiers": [
                {"bpsOfMaxSupply": "0", "discount": "0"},
                {"bpsOfMaxSupply": "4.7577998927", "discount": "0.3"}
            ]
        },
        "userCrossRate": "0.000314",
        "userAddRate": "0.000105",
        "userSpotCrossRate": "0.00049",
        "userSpotAddRate": "0.00028",
        "activeReferralDiscount": "0.04",
        "trial": null,
        "feeTrialEscrow": "0",
        "nextTrialAvailableTimestamp": null,
        "stakingLink": null,
        "activeStakingDiscount": {
            "bpsOfMaxSupply": "4.7577998927",
            "discount": "0.3"
        }
    }))
    .unwrap();

    assert_eq!(
        HyperliquidUserFeesSnapshot::from_wire_json(
            HyperliquidSnapshotMetadata {
                snapshot_id: "user-fees-snapshot".to_string(),
                source_at_ns: 90,
                fetched_at_ns: 95,
                valid_until_ns: 110,
            },
            "account",
            &json,
            &fee_eligibility_policy(1, 1),
        ),
        Err(HyperliquidEconomicsError::InvalidUserFees)
    );
}

#[test]
fn user_fees_parser_rejects_vip_rate_at_exact_volume_cutoff() {
    let json = serde_json::to_string(&serde_json::json!({
        "dailyUserVlm": [{
            "date": "1970-01-01",
            "userCross": "5000000",
            "userAdd": "0",
            "exchange": "1000000"
        }],
        "feeSchedule": {
            "cross": "0.00045",
            "add": "0.00015",
            "spotCross": "0.0007",
            "spotAdd": "0.0004",
            "tiers": {
                "vip": [{
                    "ntlCutoff": "5000000",
                    "cross": "0.0004",
                    "add": "0.00012",
                    "spotCross": "0.0006",
                    "spotAdd": "0.0003"
                }],
                "mm": []
            },
            "referralDiscount": "0.04",
            "stakingDiscountTiers": [{"bpsOfMaxSupply": "0", "discount": "0"}]
        },
        "userCrossRate": "0.0004",
        "userAddRate": "0.00012",
        "userSpotCrossRate": "0.0006",
        "userSpotAddRate": "0.0003",
        "activeReferralDiscount": "0",
        "trial": null,
        "feeTrialEscrow": "0",
        "nextTrialAvailableTimestamp": null,
        "stakingLink": null,
        "activeStakingDiscount": {"bpsOfMaxSupply": "0", "discount": "0"}
    }))
    .unwrap();

    assert_eq!(
        HyperliquidUserFeesSnapshot::from_wire_json(
            HyperliquidSnapshotMetadata {
                snapshot_id: "user-fees-snapshot".to_string(),
                source_at_ns: 90,
                fetched_at_ns: 95,
                valid_until_ns: 110,
            },
            "account",
            &json,
            &fee_eligibility_policy(1, 1),
        ),
        Err(HyperliquidEconomicsError::InvalidUserFees)
    );
}

#[test]
fn user_fees_parser_rejects_maker_rebate_at_exact_fraction_cutoff() {
    let json = serde_json::to_string(&serde_json::json!({
        "dailyUserVlm": [{
            "date": "1970-01-01",
            "userCross": "0",
            "userAdd": "5000",
            "exchange": "1000000"
        }],
        "feeSchedule": {
            "cross": "0.00045",
            "add": "0.00015",
            "spotCross": "0.0007",
            "spotAdd": "0.0004",
            "tiers": {
                "vip": [],
                "mm": [{"makerFractionCutoff": "0.005", "add": "-0.00001"}]
            },
            "referralDiscount": "0.04",
            "stakingDiscountTiers": [{"bpsOfMaxSupply": "0", "discount": "0"}]
        },
        "userCrossRate": "0.00045",
        "userAddRate": "-0.00001",
        "userSpotCrossRate": "0.0007",
        "userSpotAddRate": "0.0004",
        "activeReferralDiscount": "0",
        "trial": null,
        "feeTrialEscrow": "0",
        "nextTrialAvailableTimestamp": null,
        "stakingLink": null,
        "activeStakingDiscount": {"bpsOfMaxSupply": "0", "discount": "0"}
    }))
    .unwrap();

    assert_eq!(
        HyperliquidUserFeesSnapshot::from_wire_json(
            HyperliquidSnapshotMetadata {
                snapshot_id: "user-fees-snapshot".to_string(),
                source_at_ns: 90,
                fetched_at_ns: 95,
                valid_until_ns: 110,
            },
            "account",
            &json,
            &fee_eligibility_policy(1, 1),
        ),
        Err(HyperliquidEconomicsError::InvalidUserFees)
    );
}

#[test]
fn contradictory_product_kind_flags_fail_closed() {
    let spot_hip3 = HyperliquidProductEconomicsSnapshot::from_json(
        r#"{
            "snapshotId":"product-snapshot","sourceAtNs":91,"fetchedAtNs":96,
            "validUntilNs":110,"productKind":"spot","stablePair":false,
            "baseUnit":"BTC","quoteUnit":"USDC",
            "alignedQuoteOrCollateral":false,"hip3":true,"deployerScale":0.5,
            "growthMode":false,"builderProfileId":"builder-profile",
            "builderRateBps":0,"builderApprovedMaxBps":0,
            "spotDustAuthorityComplete":true
        }"#,
    )
    .unwrap();
    assert_eq!(
        HyperliquidEconomicsAdapter::try_new(config(), user_fees("0"), spot_hip3).err(),
        Some(HyperliquidEconomicsError::InvalidFeeSurface)
    );

    let perp_stable = HyperliquidProductEconomicsSnapshot::from_json(
        r#"{
            "snapshotId":"product-snapshot","sourceAtNs":91,"fetchedAtNs":96,
            "validUntilNs":110,"productKind":"perp","stablePair":true,
            "baseUnit":"BTC","quoteUnit":"USDC",
            "alignedQuoteOrCollateral":false,"hip3":false,"deployerScale":0,
            "growthMode":false,"builderProfileId":"builder-profile",
            "builderRateBps":0,"builderApprovedMaxBps":0,
            "spotDustAuthorityComplete":false
        }"#,
    )
    .unwrap();
    assert_eq!(
        HyperliquidEconomicsAdapter::try_new(config(), user_fees("0"), perp_stable).err(),
        Some(HyperliquidEconomicsError::InvalidFeeSurface)
    );
}
