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
use std::{any::Any, cell::RefCell, collections::BTreeMap, rc::Rc, str::FromStr, sync::Arc};
use ustr::Ustr;

use crate::{
    bolt_v3_capital_admission::{
        CapitalAdmissionPolicy, OrderLifecycleCapitalAdmissionSnapshot,
        PortfolioCapitalAdmissionSnapshot, PredictionMarketAdmissionSnapshot,
        ProductAdmissionSnapshot, ProductKind, VenueSpendabilitySnapshot,
    },
    bolt_v3_capital_reservation::CapitalPoolSnapshot,
    bolt_v3_decision_evidence::{BoltV3OrderIntentEvidence, BoltV3OrderIntentKind},
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeValuationObservation,
        ConfiguredEconomicsAdmissionSource, ConfiguredEconomicsSourcePolicy,
        EconomicsAdmissionPurpose, EconomicsAdmissionQuoteIntent, EconomicsAdmissionSource,
        EconomicsOrderBinding, EconomicsReceiptClock, ProviderEconomicsAuthority,
    },
    bolt_v3_order_execution::{BoltV3OrderEconomicsIntent, BoltV3PlannedFillLeg},
    bolt_v3_providers::polymarket::economics::{
        PolymarketEconomicsAuthority, PolymarketEconomicsSource, PolymarketEconomicsSourceOverride,
    },
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequestInput, BoltV3SubmitAdmissionState,
        BoltV3SubmitCapitalAdmissionConfig, BoltV3SubmitCapitalAdmissionNtComponents,
        BoltV3SubmitLifecyclePolicy, OrderValuationContext,
        build_submit_admission_request_from_order,
    },
    economics::{
        AccountId, DecisionCorrelationId, EconomicQuoteRequest, EdgeBasisPolicyId,
        ExecutionClientId, InstrumentId as EconomicsInstrumentId, LifecyclePath,
        LiquidityRoleAssumption, OrderSide, PlannedFillLeg, ProductSurfaceId, ReportingPolicyId,
        RoutingContext, SnapshotId, currency_from_code,
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
            from_unit: currency_from_code("pUSD")?,
            to_unit: currency_from_code("USDC.e")?,
            rate: Decimal::ONE,
            snapshot_id: SnapshotId::new("governed-pusd-usdc-e")?,
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
        reporting_unit: currency_from_code("USD").unwrap(),
        edge_basis_policy_id: EdgeBasisPolicyId::new("primary").unwrap(),
        requested_at_ns: NOW_NS,
        decision_correlation_id: DecisionCorrelationId::new("composition-tracer").unwrap(),
    }
}

#[tokio::test]
async fn shipped_shaped_capture_publishes_quotes_reserves_and_rolls_back() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("shipped-shaped fixture config must load");
    loaded
        .root
        .clients
        .retain(|key, client| key == "polymarket_main" || client.execution.is_none());
    let resolved = crate::bolt_v3_secrets::ResolvedBoltV3Secrets {
        clients: BTreeMap::new(),
    };
    let override_value: Arc<dyn Any + Send + Sync> = Arc::new(PolymarketEconomicsSourceOverride {
        source: Arc::new(FixturePolymarketSource {
            wire_body: MARKET_INFO,
        }),
    });
    let overrides = BTreeMap::from([("polymarket_main".to_string(), override_value)]);
    let authorities = build_economics_authorities(&loaded, &resolved, &overrides)
        .expect("the production provider registry must build the fixture-backed authority");
    let [authority] = authorities.as_slice() else {
        panic!("the fixture must build exactly one economics authority");
    };
    let inputs = AuthoritativeEconomicsInputStore::default();
    let cache = cache_with_valuation();
    let instrument = binary_instrument();

    let published = refresh_compile_publish_economics_once(
        authority,
        &inputs,
        &cache,
        vec![instrument.clone()],
        &|| Ok(NOW_NS),
    )
    .await
    .expect("the production one-shot refresh must succeed");
    assert_eq!(published, 1);

    let strategy = loaded
        .strategies
        .first()
        .expect("fixture must load its configured strategy");
    let client = &loaded.root.clients["polymarket_main"];
    let routing = crate::bolt_v3_strategy_registration::build_order_routing_handle(
        &loaded, strategy, client, &inputs,
    )
    .expect("config-derived production order routing must build");
    let order = generic_market_order(
        "composition-order",
        INSTRUMENT_ID,
        nautilus_model::enums::OrderSide::Buy,
        Quantity::from("10"),
    );
    let order_intent = BoltV3OrderIntentEvidence::from_compiled_order(
        strategy.config.strategy_instance_id.clone(),
        BoltV3OrderIntentKind::Entry,
        "0.5".to_string(),
        &order,
    );
    let submit_input = BoltV3SubmitAdmissionRequestInput {
        execution_client_id: "polymarket_main",
        intent: &order_intent,
        order: &order,
        valuation: OrderValuationContext {
            last_quote: None,
            instrument: Some(&instrument),
        },
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_position: None,
    };
    let admission = routing
        .quote_admission(BoltV3OrderEconomicsIntent {
            request: &submit_input,
            planned_fill_legs: vec![BoltV3PlannedFillLeg {
                price: Decimal::from_str("0.5").unwrap(),
                quantity: Decimal::from(10),
            }],
            liquidity_role: LiquidityRoleAssumption::Taker,
            position: None,
            lifecycle_path: LifecyclePath::PlannedExit,
            requested_at_ns: NOW_NS,
            decision_correlation_id: "composition-tracer",
            gross_expected_value: Decimal::from(10),
        })
        .expect("config-derived production routing must quote and admit");
    assert!(admission.guaranteed_debit().amount() > Decimal::ZERO);
    let pool = CapitalPoolSnapshot {
        source: "shadow-capital-fixture".to_string(),
        observed_at_ns: NOW_NS,
        pool_id: "shadow-evaluation".to_string(),
        max_pool_liability: Decimal::from(100),
        committed_liability: Decimal::ZERO,
        max_snapshot_age_ns: 1,
    };
    let sealed_liability = admission.full_reservation_liability().amount();
    let submit_request = build_submit_admission_request_from_order(submit_input, admission)
        .expect("the routed admission must bind to the final order");
    let submit_state = BoltV3SubmitAdmissionState::new_with_capital_admission(
        Arc::new(NoStrategyDecisionEvidenceWriter),
        BoltV3SubmitCapitalAdmissionConfig {
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            collateral_currency: "PUSD".to_string(),
            capital_pool: pool,
            policy: CapitalAdmissionPolicy {
                min_remaining_pool_balance: None,
            },
            dedupe_retention_ns: 1,
        },
    );
    submit_state.update_capital_admission_nt_components(BoltV3SubmitCapitalAdmissionNtComponents {
        source: "composition-tracer".to_string(),
        observed_at_ns: NOW_NS,
        portfolio: PortfolioCapitalAdmissionSnapshot {
            source: "composition-portfolio".to_string(),
            observed_at_ns: NOW_NS,
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            collateral_currency: "PUSD".to_string(),
            free_collateral: Decimal::from(100),
            total_equity: Decimal::from(100),
        },
        venue_spendability: VenueSpendabilitySnapshot {
            source: "composition-spendability".to_string(),
            observed_at_ns: NOW_NS,
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            collateral_currency: "PUSD".to_string(),
            spendable_collateral: Decimal::from(100),
            collateral_allowance: Decimal::from(100),
        },
        order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
            source: "composition-orders".to_string(),
            observed_at_ns: NOW_NS,
            open_order_count: 0,
            all_open_orders_attributed: true,
        },
        product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
            PredictionMarketAdmissionSnapshot {
                source: "composition-product".to_string(),
                observed_at_ns: NOW_NS,
                yes_instrument_id: INSTRUMENT_ID.to_string(),
                no_instrument_id: "other-condition-token.POLYMARKET".to_string(),
                yes_position: Decimal::ZERO,
                no_position: Decimal::ZERO,
                collateral_allowance: Decimal::from(100),
                conditional_token_allowance: Decimal::from(100),
                collateral_coupled_group_id: "polymarket-collateral".to_string(),
            },
        ),
        loss_snapshot: None,
    });
    let permit = submit_state
        .admit_at(&submit_request, NOW_NS)
        .expect("the production submit and capital gates must reserve the sealed liability");
    assert_eq!(
        submit_state.capital_admission_live_reserved_liability(),
        Some(sealed_liability)
    );
    drop(permit);
    assert_eq!(
        submit_state.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[tokio::test]
async fn malformed_capture_never_publishes_quote_authority() {
    let mut loaded = fixture_loaded_config();
    loaded
        .root
        .clients
        .retain(|key, client| key == "polymarket_main" || client.execution.is_none());
    let resolved = crate::bolt_v3_secrets::ResolvedBoltV3Secrets {
        clients: BTreeMap::new(),
    };
    let override_value: Arc<dyn Any + Send + Sync> = Arc::new(PolymarketEconomicsSourceOverride {
        source: Arc::new(FixturePolymarketSource {
            wire_body: r#"{"unsupported":true}"#,
        }),
    });
    let overrides = BTreeMap::from([("polymarket_main".to_string(), override_value)]);
    let authorities = build_economics_authorities(&loaded, &resolved, &overrides).unwrap();
    let [authority] = authorities.as_slice() else {
        panic!("the fixture must build exactly one economics authority");
    };
    let inputs = AuthoritativeEconomicsInputStore::default();
    let published = refresh_compile_publish_economics_once(
        authority,
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
