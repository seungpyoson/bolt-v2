#![cfg(test)]

use super::*;

use async_trait::async_trait;
use nautilus_common::cache::Cache;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::QuoteTick,
    enums::AssetClass,
    identifiers::{InstrumentId, Symbol, Venue},
    instruments::{BinaryOption, CurrencyPair, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use rust_decimal::Decimal;
use std::{cell::RefCell, rc::Rc, str::FromStr, sync::Arc};
use ustr::Ustr;

use crate::{
    bolt_v3_capital_reservation::{CapitalPoolSnapshot, ReservationLedger, ReservationRequest},
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeValuationObservation,
        ConfiguredEconomicsAdmissionSource, ConfiguredEconomicsSourcePolicy,
        EconomicsAdmissionPurpose, EconomicsAdmissionQuoteIntent, EconomicsAdmissionSource,
        EconomicsOrderBinding, EconomicsReceiptClock, ProviderEconomicsAuthority,
    },
    bolt_v3_providers::polymarket::economics::{
        PolymarketEconomicsAuthority, PolymarketEconomicsSource,
    },
    economics::{
        AccountId, DecisionCorrelationId, EconomicQuoteRequest, EdgeBasisPolicyId,
        ExecutionClientId, InstrumentId as EconomicsInstrumentId, LifecyclePath,
        LiquidityRoleAssumption, NativeUnitId, OrderSide, PlannedFillLeg, ProductSurfaceId,
        ReportingPolicyId, RoutingContext, SnapshotId,
    },
};

const NOW_NS: u64 = 1_800_000_000_000_000_000;
const INSTRUMENT_ID: &str = "condition-token.POLYMARKET";
const MARKET_INFO: &str = include_str!(
    "../../../tests/fixtures/bolt_v3/boundary_evidence/polymarket-market-info-fee-bearing.json"
);

struct FixturePolymarketSource {
    wire_body: &'static str,
}

#[async_trait(?Send)]
impl PolymarketEconomicsSource for FixturePolymarketSource {
    async fn fetch_market_info_body(
        &self,
        _authority: &PolymarketEconomicsAuthority,
        _instrument_id: InstrumentId,
    ) -> anyhow::Result<Vec<u8>> {
        Ok(self.wire_body.as_bytes().to_vec())
    }

    async fn observe_collateral_redemption(
        &self,
        _authority: &PolymarketEconomicsAuthority,
        receipt_clock: &dyn EconomicsReceiptClock,
        max_age_ns: u64,
    ) -> anyhow::Result<AuthoritativeValuationObservation> {
        let fetched_at_ns = receipt_clock.now_ns()?;
        let valid_until_ns = fetched_at_ns.checked_add(max_age_ns).unwrap();
        Ok(AuthoritativeValuationObservation::ProviderConversion {
            source_id: "collateral".to_string(),
            from_unit: NativeUnitId::new("pUSD")?,
            to_unit: NativeUnitId::new("USDC")?,
            rate: Decimal::ONE,
            snapshot_id: SnapshotId::new("governed-pusd-usdc")?,
            observed_at_ns: fetched_at_ns,
            fetched_at_ns,
            valid_until_ns,
        })
    }
}

fn binary_instrument() -> InstrumentAny {
    InstrumentAny::BinaryOption(BinaryOption::new(
        InstrumentId::from(INSTRUMENT_ID),
        Symbol::from("condition-token"),
        AssetClass::Alternative,
        Currency::pUSD(),
        UnixNanos::from(NOW_NS),
        UnixNanos::from(NOW_NS + 1),
        3,
        3,
        Price::from("0.001"),
        Quantity::from("0.001"),
        Some(Ustr::from("YES")),
        None,
        None,
        Some(Quantity::from("0.001")),
        None,
        None,
        Some(Price::from("0.999")),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UnixNanos::from(NOW_NS),
        UnixNanos::from(NOW_NS),
    ))
}

fn valuation_instrument() -> InstrumentAny {
    InstrumentAny::CurrencyPair(CurrencyPair::new(
        InstrumentId::from("USDC-USD.COINBASE"),
        Symbol::from("USDC-USD"),
        Currency::from("USDC"),
        Currency::USD(),
        4,
        2,
        Price::from("0.0001"),
        Quantity::from("0.01"),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UnixNanos::from(NOW_NS),
        UnixNanos::from(NOW_NS),
    ))
}

fn cache_with_valuation() -> Rc<RefCell<Cache>> {
    let cache = Rc::new(RefCell::new(Cache::new(None, None)));
    let mut cache_mut = cache.borrow_mut();
    cache_mut
        .add_instrument(valuation_instrument())
        .expect("valuation instrument should enter the production cache");
    cache_mut
        .add_quote(QuoteTick::new(
            InstrumentId::from("USDC-USD.COINBASE"),
            Price::from("0.9999"),
            Price::from("1.0001"),
            Quantity::from("100"),
            Quantity::from("100"),
            UnixNanos::from(NOW_NS),
            UnixNanos::from(NOW_NS),
        ))
        .expect("valuation quote should enter the production cache");
    drop(cache_mut);
    cache
}

fn request() -> EconomicQuoteRequest {
    EconomicQuoteRequest {
        execution_client_id: ExecutionClientId::new("polymarket_main").unwrap(),
        account_id: AccountId::new("POLYMARKET-001").unwrap(),
        instrument_id: EconomicsInstrumentId::new(INSTRUMENT_ID).unwrap(),
        product_surface_id: ProductSurfaceId::new("binary_outcome").unwrap(),
        order_side: OrderSide::Buy,
        liquidity_role: LiquidityRoleAssumption::Taker,
        planned_fill_legs: vec![
            PlannedFillLeg {
                price: Decimal::from_str("0.49").unwrap(),
                quantity: Decimal::from(5),
            },
            PlannedFillLeg {
                price: Decimal::from_str("0.51").unwrap(),
                quantity: Decimal::from(5),
            },
        ],
        routing: RoutingContext {
            attached_charge: None,
        },
        position: None,
        lifecycle_path: LifecyclePath::PlannedExit,
        reporting_policy_id: ReportingPolicyId::new("primary-pnl").unwrap(),
        reporting_unit: NativeUnitId::new("USD").unwrap(),
        edge_basis_policy_id: EdgeBasisPolicyId::new("primary").unwrap(),
        requested_at_ns: NOW_NS,
        decision_correlation_id: DecisionCorrelationId::new("composition-tracer").unwrap(),
    }
}

#[tokio::test]
async fn shipped_shaped_capture_publishes_quotes_reserves_and_rolls_back() {
    let loaded = fixture_loaded_config();
    let execution: crate::bolt_v3_providers::polymarket::PolymarketExecutionConfig =
        loaded.root.clients["polymarket_main"]
            .execution
            .as_ref()
            .expect("shipped-shaped fixture must configure Polymarket execution")
            .clone()
            .try_into()
            .expect("shipped-shaped Polymarket execution must parse");
    let authority: Arc<dyn ProviderEconomicsAuthority> = Arc::new(
        PolymarketEconomicsAuthority::try_new_with_source(
            "polymarket_main",
            Venue::from("POLYMARKET"),
            execution,
            Arc::new(FixturePolymarketSource {
                wire_body: MARKET_INFO,
            }),
        )
        .expect("shipped-shaped production authority must compile"),
    );
    let inputs = AuthoritativeEconomicsInputStore::default();
    let cache = cache_with_valuation();

    let published = refresh_compile_publish_economics_once(
        &authority,
        &inputs,
        &cache,
        vec![binary_instrument()],
        &|| Ok(NOW_NS),
    )
    .await
    .expect("the production one-shot refresh must succeed");
    assert_eq!(published, 1);

    let source = ConfiguredEconomicsAdmissionSource::new(
        "POLYMARKET",
        inputs,
        ConfiguredEconomicsSourcePolicy {
            quote_refresh_ns: 30_000_000_000,
            quote_max_age_ns: 60_000_000_000,
            quote_validity_ns: 30_000_000_000,
            resting_order_refresh_margin_ns: 5_000_000_000,
        },
    )
    .unwrap();
    let admission = source
        .quote_admission(EconomicsAdmissionQuoteIntent {
            request: request(),
            order_binding: EconomicsOrderBinding::from_sha256(
                <sha2::Sha256 as sha2::Digest>::digest(b"composition-order"),
            ),
            purpose: EconomicsAdmissionPurpose::TradingEdge,
            gross_expected_value: Decimal::from(10),
            reservation_basis: crate::economics::ReservationBasis::new(
                Decimal::from_str("9.99").unwrap(),
            )
            .unwrap(),
        })
        .expect("published governed facts must quote and admit");
    assert!(admission.guaranteed_debit().amount() > Decimal::ZERO);

    let mut ledger = ReservationLedger::reconciled();
    let pool = CapitalPoolSnapshot {
        source: "shadow-capital-fixture".to_string(),
        observed_at_ns: NOW_NS,
        pool_id: "shadow-evaluation".to_string(),
        max_pool_liability: Decimal::from(100),
        committed_liability: Decimal::ZERO,
        max_snapshot_age_ns: 1,
    };
    let reservation = ReservationRequest {
        request_id: "composition-order".to_string(),
        pool_id: pool.pool_id.clone(),
        collateral_group_id: "polymarket-collateral".to_string(),
        liability: admission.full_reservation_liability().amount(),
        observed_at_ns: NOW_NS,
        evidence_label: "sealed-economics-admission".to_string(),
    };
    assert!(ledger.reserve(&pool, &reservation, NOW_NS, None).accepted);
    assert_eq!(
        ledger.live_reserved_liability(&pool.pool_id),
        admission.full_reservation_liability().amount()
    );
    assert_eq!(
        ledger.rollback_uncommitted(&pool.pool_id, &reservation.request_id),
        Some(admission.full_reservation_liability().amount())
    );
    assert_eq!(ledger.live_reserved_liability(&pool.pool_id), Decimal::ZERO);
}

#[tokio::test]
async fn malformed_capture_never_publishes_quote_authority() {
    let loaded = fixture_loaded_config();
    let execution: crate::bolt_v3_providers::polymarket::PolymarketExecutionConfig =
        loaded.root.clients["polymarket_main"]
            .execution
            .as_ref()
            .unwrap()
            .clone()
            .try_into()
            .expect("shipped-shaped Polymarket execution must parse");
    let authority: Arc<dyn ProviderEconomicsAuthority> = Arc::new(
        PolymarketEconomicsAuthority::try_new_with_source(
            "polymarket_main",
            Venue::from("POLYMARKET"),
            execution,
            Arc::new(FixturePolymarketSource {
                wire_body: r#"{"unsupported":true}"#,
            }),
        )
        .unwrap(),
    );
    let inputs = AuthoritativeEconomicsInputStore::default();
    let published = refresh_compile_publish_economics_once(
        &authority,
        &inputs,
        &cache_with_valuation(),
        vec![binary_instrument()],
        &|| Ok(NOW_NS),
    )
    .await
    .expect("per-instrument malformed input is isolated by the production publisher");
    assert_eq!(published, 0);
    let source = ConfiguredEconomicsAdmissionSource::new(
        "POLYMARKET",
        inputs,
        ConfiguredEconomicsSourcePolicy {
            quote_refresh_ns: 30_000_000_000,
            quote_max_age_ns: 60_000_000_000,
            quote_validity_ns: 30_000_000_000,
            resting_order_refresh_margin_ns: 5_000_000_000,
        },
    )
    .unwrap();
    assert!(
        source
            .resolve_product_surface(
                &ExecutionClientId::new("polymarket_main").unwrap(),
                &EconomicsInstrumentId::new(INSTRUMENT_ID).unwrap(),
                &[ProductSurfaceId::new("binary_outcome").unwrap()],
            )
            .is_err()
    );
}
