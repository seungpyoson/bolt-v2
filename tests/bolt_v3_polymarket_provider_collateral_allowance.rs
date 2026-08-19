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
const UINT256_MAX_PLUS_ONE: &str =
    "115792089237316195423570985008687907853269984665640564039457584007913129639936";
const SPENDER_A: &str = "0xE111180000d2663C0091e4f400237545B87B996B";
const SPENDER_A_LOWER: &str = "0xe111180000d2663c0091e4f400237545b87b996b";
const SPENDER_B: &str = "0xe2222d279d744050d28e00520010520000310F59";
const SPENDER_C: &str = "0xadA2005600Dec949baf300f4C6120000bDB6eAab";
const SPENDER_D: &str = "0xdddddddddddddddddddddddddddddddddddddddd";

#[test]
fn complete_plural_allowances_use_the_conservative_spendable_minimum() {
    let snapshot =
        build_polymarket_provider_collateral_allowance_snapshot(input(balance_allowance(
            Decimal::new(50_000_000, 0),
            [
                (SPENDER_A, UINT256_MAX),
                (SPENDER_B, "40000000"),
                (SPENDER_C, "45000000"),
            ],
        )))
        .expect("complete provider allowance evidence should convert");

    assert_eq!(snapshot.observed_at_ns, 1_000);
    assert_eq!(snapshot.account_id, "POLYMARKET-001");
    assert_eq!(snapshot.collateral_allowance, Decimal::new(4000, 2));
}

#[test]
fn unlimited_plural_allowances_are_capped_by_available_balance() {
    let snapshot =
        build_polymarket_provider_collateral_allowance_snapshot(input(balance_allowance(
            Decimal::new(50_000_000, 0),
            [
                (SPENDER_A, UINT256_MAX),
                (SPENDER_B, UINT256_MAX),
                (SPENDER_C, UINT256_MAX),
            ],
        )))
        .expect("uint256-max allowances should be capped before money conversion");

    assert_eq!(snapshot.collateral_allowance, Decimal::new(5000, 2));
}

#[test]
fn rejects_allowance_above_uint256_range() {
    assert_invalid_allowance(balance_allowance(
        Decimal::new(50_000_000, 0),
        [
            (SPENDER_A, UINT256_MAX_PLUS_ONE),
            (SPENDER_B, UINT256_MAX),
            (SPENDER_C, UINT256_MAX),
        ],
    ));
}

#[test]
fn rejects_missing_plural_allowance_evidence() {
    let result = build_polymarket_provider_collateral_allowance_snapshot(input(BalanceAllowance {
        balance: Decimal::ZERO,
        allowance: None,
        allowances: HashMap::new(),
    }));

    assert_eq!(
        result,
        Err(PolymarketProviderCollateralAllowanceBuildError::MissingCollateralAllowance)
    );
}

#[test]
fn rejects_legacy_singular_allowance_without_spender_identity() {
    let result = build_polymarket_provider_collateral_allowance_snapshot(input(BalanceAllowance {
        balance: Decimal::new(50_000_000, 0),
        allowance: Some(Decimal::new(40_000_000, 0)),
        allowances: HashMap::new(),
    }));

    assert_eq!(
        result,
        Err(PolymarketProviderCollateralAllowanceBuildError::InvalidCollateralAllowance)
    );
}

#[test]
fn rejects_missing_extra_or_wrong_spender_identity() {
    assert_invalid_allowance(BalanceAllowance {
        balance: Decimal::new(50_000_000, 0),
        allowance: None,
        allowances: allowance_map([(SPENDER_A, UINT256_MAX), (SPENDER_B, UINT256_MAX)]),
    });
    assert_invalid_allowance(BalanceAllowance {
        balance: Decimal::new(50_000_000, 0),
        allowance: None,
        allowances: allowance_map([
            (SPENDER_A, UINT256_MAX),
            (SPENDER_B, UINT256_MAX),
            (SPENDER_C, UINT256_MAX),
            (SPENDER_D, UINT256_MAX),
        ]),
    });
    assert_invalid_allowance(balance_allowance(
        Decimal::new(50_000_000, 0),
        [
            (SPENDER_A, UINT256_MAX),
            (SPENDER_B, UINT256_MAX),
            (SPENDER_D, UINT256_MAX),
        ],
    ));
}

#[test]
fn rejects_malformed_or_duplicate_spender_identity() {
    assert_invalid_allowance(balance_allowance(
        Decimal::new(50_000_000, 0),
        [
            ("exchange", UINT256_MAX),
            (SPENDER_B, UINT256_MAX),
            (SPENDER_C, UINT256_MAX),
        ],
    ));
    assert_invalid_allowance(balance_allowance(
        Decimal::new(50_000_000, 0),
        [
            (" 0xE111180000d2663C0091e4f400237545B87B996B", UINT256_MAX),
            (SPENDER_B, UINT256_MAX),
            (SPENDER_C, UINT256_MAX),
        ],
    ));
    assert_invalid_allowance(BalanceAllowance {
        balance: Decimal::new(50_000_000, 0),
        allowance: None,
        allowances: allowance_map([
            (SPENDER_A, UINT256_MAX),
            (SPENDER_A_LOWER, UINT256_MAX),
            (SPENDER_B, UINT256_MAX),
            (SPENDER_C, UINT256_MAX),
        ]),
    });
}

#[test]
fn rejects_malformed_fractional_or_negative_allowance_values() {
    for invalid in ["not-a-number", "1.5", "-1"] {
        assert_invalid_allowance(balance_allowance(
            Decimal::new(50_000_000, 0),
            [
                (SPENDER_A, UINT256_MAX),
                (SPENDER_B, invalid),
                (SPENDER_C, UINT256_MAX),
            ],
        ));
    }
}

#[test]
fn rejects_negative_collateral_balance() {
    let result = build_polymarket_provider_collateral_allowance_snapshot(input(balance_allowance(
        Decimal::NEGATIVE_ONE,
        [
            (SPENDER_A, UINT256_MAX),
            (SPENDER_B, UINT256_MAX),
            (SPENDER_C, UINT256_MAX),
        ],
    )));

    assert_eq!(
        result,
        Err(PolymarketProviderCollateralAllowanceBuildError::InvalidCollateralMoney)
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

fn assert_invalid_allowance(collateral: BalanceAllowance) {
    assert_eq!(
        build_polymarket_provider_collateral_allowance_snapshot(input(collateral)),
        Err(PolymarketProviderCollateralAllowanceBuildError::InvalidCollateralAllowance)
    );
}

fn balance_allowance<const N: usize>(
    balance: Decimal,
    allowances: [(&str, &str); N],
) -> BalanceAllowance {
    BalanceAllowance {
        balance,
        allowance: None,
        allowances: allowance_map(allowances),
    }
}

fn allowance_map<const N: usize>(allowances: [(&str, &str); N]) -> HashMap<String, String> {
    allowances
        .into_iter()
        .map(|(spender, allowance)| (spender.to_string(), allowance.to_string()))
        .collect()
}

fn input(collateral: BalanceAllowance) -> PolymarketProviderCollateralAllowanceInput {
    PolymarketProviderCollateralAllowanceInput {
        captured_at: UnixNanos::from(1_000),
        account_id: AccountId::from("POLYMARKET-001"),
        collateral_currency: Currency::pUSD(),
        collateral,
        required_spenders: required_spenders(),
    }
}

fn required_spenders() -> Vec<String> {
    vec![
        SPENDER_A.to_string(),
        SPENDER_B.to_string(),
        SPENDER_C.to_string(),
    ]
}
