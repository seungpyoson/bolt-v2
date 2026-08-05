//! Polymarket provider collateral allowance boundary tests.

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
            },
        },
    )
    .expect("valid provider payload should convert");

    assert_eq!(snapshot.observed_at_ns, 1_000);
    assert_eq!(snapshot.account_id, "POLYMARKET-001");
    assert_eq!(snapshot.collateral_allowance, Decimal::new(4000, 2));
}

#[test]
fn rejects_provider_payloads_that_cannot_form_a_snapshot() {
    let missing_allowance =
        build_polymarket_provider_collateral_allowance_snapshot(input(BalanceAllowance {
            balance: Decimal::ZERO,
            allowance: None,
        }));
    assert_eq!(
        missing_allowance,
        Err(PolymarketProviderCollateralAllowanceBuildError::MissingCollateralAllowance)
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
