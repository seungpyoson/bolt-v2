//! Polymarket provider collateral allowance boundary tests.

use std::collections::HashMap;

use bolt_v2::bolt_v3_provider_collateral_allowance::{
    ProviderCollateralAllowanceCaptureEndpoint, ProviderCollateralAllowanceCaptureEndpointError,
    ProviderCollateralAllowanceCaptureErrorClass,
    provider_collateral_allowance_capture_failure_parts,
};
use bolt_v2::bolt_v3_providers::polymarket::{
    PolymarketProviderCollateralAllowanceBuildError, PolymarketProviderCollateralAllowanceInput,
    build_polymarket_provider_collateral_allowance_snapshot,
};
use nautilus_core::UnixNanos;
use nautilus_model::{identifiers::AccountId, types::Currency};
use nautilus_polymarket::http::query::BalanceAllowance;
use rust_decimal::Decimal;

const UINT256_MAX: &str =
    "115792089237316195423570985008687907853269984665640564039457584007913129639935";

#[test]
fn builds_snapshot_from_provider_payloads_without_reconciling_nt_state() {
    let snapshot = build_polymarket_provider_collateral_allowance_snapshot(
        PolymarketProviderCollateralAllowanceInput {
            captured_at: UnixNanos::from(1_000),
            account_id: AccountId::from("POLYMARKET-001"),
            collateral_currency: Currency::pUSD(),
            collateral: BalanceAllowance {
                balance: Decimal::new(50_000_000, 0),
                allowance: Some(Decimal::new(40_000_000, 0)),
                allowances: HashMap::new(),
            },
        },
    )
    .expect("valid provider payload should convert");

    assert_eq!(snapshot.observed_at_ns, 1_000);
    assert_eq!(snapshot.account_id, "POLYMARKET-001");
    assert_eq!(snapshot.collateral_allowance, Decimal::new(4000, 2));
}

#[test]
fn current_plural_allowances_use_the_conservative_spendable_minimum() {
    let snapshot =
        build_polymarket_provider_collateral_allowance_snapshot(input(BalanceAllowance {
            balance: Decimal::new(50_000_000, 0),
            allowance: None,
            allowances: HashMap::from([
                ("exchange".to_string(), UINT256_MAX.to_string()),
                ("neg-risk".to_string(), "40000000".to_string()),
                ("neg-risk-adapter".to_string(), "45000000".to_string()),
            ]),
        }))
        .expect("current plural allowance payload should convert");

    assert_eq!(snapshot.collateral_allowance, Decimal::new(4000, 2));
}

#[test]
fn unlimited_plural_allowances_are_capped_by_available_balance() {
    let snapshot =
        build_polymarket_provider_collateral_allowance_snapshot(input(BalanceAllowance {
            balance: Decimal::new(50_000_000, 0),
            allowance: None,
            allowances: HashMap::from([
                ("exchange".to_string(), UINT256_MAX.to_string()),
                ("neg-risk".to_string(), UINT256_MAX.to_string()),
            ]),
        }))
        .expect("uint256-max allowances should not overflow Bolt money");

    assert_eq!(snapshot.collateral_allowance, Decimal::new(5000, 2));
}

#[test]
fn rejects_provider_payloads_that_cannot_form_a_snapshot() {
    let missing_allowance =
        build_polymarket_provider_collateral_allowance_snapshot(input(BalanceAllowance {
            balance: Decimal::ZERO,
            allowance: None,
            allowances: HashMap::new(),
        }));
    assert_eq!(
        missing_allowance,
        Err(PolymarketProviderCollateralAllowanceBuildError::MissingCollateralAllowance)
    );
}

#[test]
fn rejects_malformed_plural_allowance_values() {
    let malformed =
        build_polymarket_provider_collateral_allowance_snapshot(input(BalanceAllowance {
            balance: Decimal::new(50_000_000, 0),
            allowance: None,
            allowances: HashMap::from([("exchange".to_string(), "not-a-number".to_string())]),
        }));

    assert_eq!(
        malformed,
        Err(PolymarketProviderCollateralAllowanceBuildError::InvalidCollateralAllowance)
    );
}

#[test]
fn capture_failure_domain_survives_anyhow_context() {
    let error = anyhow::anyhow!(ProviderCollateralAllowanceCaptureEndpointError::new(
        ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance,
        ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode,
        anyhow::anyhow!("synthetic endpoint failure"),
    ))
    .context("poll Polymarket provider-allowance endpoints");

    assert_eq!(
        provider_collateral_allowance_capture_failure_parts(&error),
        (
            ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance,
            ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode
        )
    );
}

fn input(collateral: BalanceAllowance) -> PolymarketProviderCollateralAllowanceInput {
    PolymarketProviderCollateralAllowanceInput {
        captured_at: UnixNanos::from(1_000),
        account_id: AccountId::from("POLYMARKET-001"),
        collateral_currency: Currency::pUSD(),
        collateral,
    }
}
