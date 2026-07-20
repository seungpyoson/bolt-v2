#![cfg(test)]

use super::*;

use async_trait::async_trait;
use nautilus_common::cache::Cache;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::QuoteTick,
    enums::AssetClass,
    identifiers::{InstrumentId, Symbol},
    instruments::{BinaryOption, CurrencyPair, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use rust_decimal::Decimal;
use std::{
    any::Any,
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
    str::FromStr,
    sync::{Arc, Mutex},
};
use ustr::Ustr;

use crate::{
    bolt_v3_capital_admission::{
        CapitalAdmissionPolicy, PredictionMarketAdmissionSnapshot, ProductAdmissionSnapshot,
        ProductKind,
    },
    bolt_v3_capital_admission_state::{
        OrderLifecycleCapitalAdmissionSnapshot, PortfolioCapitalAdmissionSnapshot,
        VenueSpendabilitySnapshot,
    },
    bolt_v3_capital_reservation::CapitalPoolSnapshot,
    bolt_v3_decision_evidence::{BoltV3OrderIntentEvidence, BoltV3OrderIntentKind},
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeValuationObservation,
        ConfiguredEconomicsAdmissionSource, ConfiguredEconomicsSourcePolicy,
        EconomicsAdmissionSource, EconomicsReceiptClock,
    },
    bolt_v3_operator_artifacts::BoltV3OperatorArtifactError,
    bolt_v3_order_execution::{BoltV3OrderEconomicsIntent, BoltV3PlannedFillLeg},
    bolt_v3_providers::polymarket::collateral_accounting_source::{
        OnChainBlockHeader, OnChainCollateralRpc,
    },
    bolt_v3_providers::polymarket::economics::{
        PolymarketEconomicsAuthority, PolymarketEconomicsSource, PolymarketEconomicsSourceOverride,
        function_calldata,
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
        RoutingContext, currency_from_code,
    },
};

const INSTRUMENT_ID: &str = "condition-token.POLYMARKET";
const MARKET_INFO: &str = include_str!(
    "../../../tests/fixtures/bolt_v3/boundary_evidence/polymarket-market-info-fee-bearing.json"
);
const COLLATERAL_FINALIZED_BLOCK: &str = include_str!(
    "../../../tests/fixtures/bolt_v3/boundary_evidence/polymarket-collateral-finalized-block.json"
);
const COLLATERAL_PROXY_IMPLEMENTATION: &str = include_str!(
    "../../../tests/fixtures/bolt_v3/boundary_evidence/polymarket-collateral-proxy-implementation.json"
);
const COLLATERAL_CODE_HASHES: &str = include_str!(
    "../../../tests/fixtures/bolt_v3/boundary_evidence/polymarket-collateral-contract-code-hashes.json"
);
const COLLATERAL_REDEMPTION_SEMANTICS: &str = include_str!(
    "../../../tests/fixtures/bolt_v3/boundary_evidence/polymarket-collateral-redemption-semantics.json"
);

struct GovernedCollateralRpcFixture {
    chain_id: u64,
    latest_block: u64,
    block: OnChainBlockHeader,
    calls: Mutex<VecDeque<ExpectedContractCall>>,
    storage: Mutex<Option<ExpectedStorageRead>>,
    codes: Mutex<VecDeque<ExpectedCodeRead>>,
}

struct ExpectedContractCall {
    contract_address: String,
    calldata: String,
    block_tag: String,
    result: [u8; 32],
}

struct ExpectedStorageRead {
    contract_address: String,
    slot: String,
    block_tag: String,
    result: [u8; 32],
}

struct ExpectedCodeRead {
    contract_address: String,
    block_tag: String,
    sha256: String,
}

impl GovernedCollateralRpcFixture {
    fn load() -> anyhow::Result<Self> {
        let block: serde_json::Value = serde_json::from_str(COLLATERAL_FINALIZED_BLOCK)?;
        let proxy: serde_json::Value = serde_json::from_str(COLLATERAL_PROXY_IMPLEMENTATION)?;
        let codes: serde_json::Value = serde_json::from_str(COLLATERAL_CODE_HASHES)?;
        let semantics: serde_json::Value = serde_json::from_str(COLLATERAL_REDEMPTION_SEMANTICS)?;
        let contracts = codes["contracts"]
            .as_object()
            .context("governed collateral code-hash contracts are missing")?;
        let provider_checks = &semantics["provider_checks"];
        let collateral_token = contracts["collateral_proxy"]["address"]
            .as_str()
            .context("governed collateral token address is missing")?;
        let redemption_asset = semantics["redemption_asset_address"]
            .as_str()
            .context("governed redemption asset address is missing")?;
        let paused = provider_checks["paused"]
            .as_str()
            .context("governed paused result is missing")?
            .parse::<u64>()?;
        let decimals = provider_checks["decimals"]
            .as_str()
            .context("governed decimals result is missing")?
            .parse::<u64>()?;
        let amount_ratio = Decimal::from_str(
            semantics["amount_ratio"]
                .as_str()
                .context("governed redemption amount ratio is missing")?,
        )?;
        anyhow::ensure!(
            amount_ratio == Decimal::ONE,
            "governed redemption evidence is not an exact-amount conversion"
        );
        let block_number = hex_quantity(
            block["finalized_block"]
                .as_str()
                .context("governed finalized block is missing")?,
        )?;
        let block_tag = format!("0x{block_number:x}");
        let collateral_token_rpc = fixture_rpc_address(collateral_token)?;
        let redemption_asset_rpc = fixture_rpc_address(redemption_asset)?;
        let offramp_rpc = fixture_rpc_address(
            contracts["collateral_offramp"]["address"]
                .as_str()
                .context("governed collateral offramp address is missing")?,
        )?;
        let redemption_selector = semantics["redemption_asset_selector"]
            .as_str()
            .context("governed redemption selector is missing")?;
        Ok(Self {
            chain_id: block["chain_id"]
                .as_u64()
                .context("governed chain id is missing")?,
            latest_block: hex_quantity(
                block["latest_block"]
                    .as_str()
                    .context("governed latest block is missing")?,
            )?,
            block: OnChainBlockHeader {
                number: block_number,
                hash: block["finalized_block_hash"]
                    .as_str()
                    .context("governed finalized block hash is missing")?
                    .to_string(),
                timestamp_secs: hex_quantity(
                    block["finalized_block_timestamp"]
                        .as_str()
                        .context("governed finalized block timestamp is missing")?,
                )?,
            },
            calls: Mutex::new(VecDeque::from([
                ExpectedContractCall {
                    contract_address: offramp_rpc.clone(),
                    calldata: function_calldata("COLLATERAL_TOKEN()", None),
                    block_tag: block_tag.clone(),
                    result: address_word_fixture(collateral_token)?,
                },
                ExpectedContractCall {
                    contract_address: collateral_token_rpc.clone(),
                    calldata: function_calldata(redemption_selector, None),
                    block_tag: block_tag.clone(),
                    result: address_word_fixture(redemption_asset)?,
                },
                ExpectedContractCall {
                    contract_address: offramp_rpc.clone(),
                    calldata: function_calldata("paused(address)", Some(&redemption_asset_rpc)),
                    block_tag: block_tag.clone(),
                    result: quantity_word(paused),
                },
                ExpectedContractCall {
                    contract_address: collateral_token_rpc.clone(),
                    calldata: function_calldata("decimals()", None),
                    block_tag: block_tag.clone(),
                    result: quantity_word(decimals),
                },
                ExpectedContractCall {
                    contract_address: redemption_asset_rpc,
                    calldata: function_calldata("decimals()", None),
                    block_tag: block_tag.clone(),
                    result: quantity_word(decimals),
                },
            ])),
            storage: Mutex::new(Some(ExpectedStorageRead {
                contract_address: collateral_token_rpc.clone(),
                slot: proxy["slot"]
                    .as_str()
                    .context("governed proxy implementation slot is missing")?
                    .to_string(),
                block_tag: block_tag.clone(),
                result: word_fixture(
                    proxy["result"]
                        .as_str()
                        .context("governed proxy implementation result is missing")?,
                )?,
            })),
            codes: Mutex::new(VecDeque::from([
                expected_code_read(contracts, "collateral_proxy", &block_tag)?,
                expected_code_read(contracts, "collateral_implementation", &block_tag)?,
                expected_code_read(contracts, "collateral_offramp", &block_tag)?,
            ])),
        })
    }

    fn assert_all_contract_calls_consumed(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.calls
                .lock()
                .map_err(|_| anyhow::anyhow!("governed collateral fixture lock is poisoned"))?
                .is_empty(),
            "production collateral observation skipped governed contract calls"
        );
        anyhow::ensure!(
            self.storage
                .lock()
                .map_err(|_| anyhow::anyhow!("governed collateral fixture lock is poisoned"))?
                .is_none(),
            "production collateral observation skipped governed proxy evidence"
        );
        anyhow::ensure!(
            self.codes
                .lock()
                .map_err(|_| anyhow::anyhow!("governed collateral fixture lock is poisoned"))?
                .is_empty(),
            "production collateral observation skipped governed code hashes"
        );
        Ok(())
    }
}

fn fixture_rpc_error() -> BoltV3OperatorArtifactError {
    BoltV3OperatorArtifactError::PreRunClobV2SourceInvalid {
        field: "on_chain_collateral.governed_fixture",
    }
}

fn hex_quantity(value: &str) -> anyhow::Result<u64> {
    Ok(u64::from_str_radix(
        value
            .strip_prefix("0x")
            .context("governed hex quantity has no prefix")?,
        16,
    )?)
}

fn governed_now_ns() -> u64 {
    let block: serde_json::Value = serde_json::from_str(COLLATERAL_FINALIZED_BLOCK)
        .expect("governed block fixture must parse");
    let timestamp_secs = hex_quantity(
        block["finalized_block_timestamp"]
            .as_str()
            .expect("governed block timestamp must exist"),
    )
    .expect("governed block timestamp must be hexadecimal");
    timestamp_secs
        .checked_mul(crate::bolt_v3_numeric::NANOS_PER_SECOND_U64)
        .expect("governed block timestamp must fit nanoseconds")
}

fn fixture_rpc_address(value: &str) -> anyhow::Result<String> {
    let encoded = value
        .strip_prefix("0x")
        .context("governed EVM address has no prefix")?;
    anyhow::ensure!(
        encoded.len() == 40,
        "governed EVM address must contain 20 bytes"
    );
    anyhow::ensure!(
        encoded.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "governed EVM address is not hexadecimal"
    );
    Ok(encoded.to_ascii_lowercase())
}

fn expected_code_read(
    contracts: &serde_json::Map<String, serde_json::Value>,
    name: &str,
    block_tag: &str,
) -> anyhow::Result<ExpectedCodeRead> {
    let contract = &contracts[name];
    Ok(ExpectedCodeRead {
        contract_address: fixture_rpc_address(
            contract["address"]
                .as_str()
                .context("governed collateral contract address is missing")?,
        )?,
        block_tag: block_tag.to_string(),
        sha256: contract["sha256"]
            .as_str()
            .context("governed collateral contract hash is missing")?
            .to_string(),
    })
}

fn word_fixture(value: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = hex::decode(
        value
            .strip_prefix("0x")
            .context("governed EVM word has no prefix")?,
    )?;
    anyhow::ensure!(bytes.len() == 32, "governed EVM word must contain 32 bytes");
    let mut word = [0_u8; 32];
    word.copy_from_slice(&bytes);
    Ok(word)
}

fn address_word_fixture(address: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = hex::decode(
        address
            .strip_prefix("0x")
            .context("governed EVM address has no prefix")?,
    )?;
    anyhow::ensure!(
        bytes.len() == 20,
        "governed EVM address must contain 20 bytes"
    );
    let mut word = [0_u8; 32];
    word[12..].copy_from_slice(&bytes);
    Ok(word)
}

fn quantity_word(value: u64) -> [u8; 32] {
    let mut word = [0_u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

#[async_trait(?Send)]
impl OnChainCollateralRpc for GovernedCollateralRpcFixture {
    async fn chain_id(&self) -> Result<u64, BoltV3OperatorArtifactError> {
        Ok(self.chain_id)
    }

    async fn block_number(&self) -> Result<u64, BoltV3OperatorArtifactError> {
        Ok(self.latest_block)
    }

    async fn eth_call_u256_word_at(
        &self,
        contract_address: &str,
        calldata: &str,
        block_tag: &str,
    ) -> Result<[u8; 32], BoltV3OperatorArtifactError> {
        let expected = self
            .calls
            .lock()
            .map_err(|_| fixture_rpc_error())?
            .pop_front()
            .ok_or_else(fixture_rpc_error)?;
        if expected.contract_address != contract_address
            || expected.calldata != calldata
            || expected.block_tag != block_tag
        {
            return Err(fixture_rpc_error());
        }
        Ok(expected.result)
    }

    async fn code_sha256_at(
        &self,
        contract_address: &str,
        block_tag: &str,
    ) -> Result<String, BoltV3OperatorArtifactError> {
        let expected = self
            .codes
            .lock()
            .map_err(|_| fixture_rpc_error())?
            .pop_front()
            .ok_or_else(fixture_rpc_error)?;
        if expected.contract_address != contract_address || expected.block_tag != block_tag {
            return Err(fixture_rpc_error());
        }
        Ok(expected.sha256)
    }

    async fn storage_word_at(
        &self,
        contract_address: &str,
        slot: &str,
        block_tag: &str,
    ) -> Result<[u8; 32], BoltV3OperatorArtifactError> {
        let expected = self
            .storage
            .lock()
            .map_err(|_| fixture_rpc_error())?
            .take()
            .ok_or_else(fixture_rpc_error)?;
        if expected.contract_address != contract_address
            || expected.slot != slot
            || expected.block_tag != block_tag
        {
            return Err(fixture_rpc_error());
        }
        Ok(expected.result)
    }

    async fn block_header(
        &self,
        block_number: u64,
    ) -> Result<OnChainBlockHeader, BoltV3OperatorArtifactError> {
        if block_number != self.block.number {
            return Err(fixture_rpc_error());
        }
        Ok(self.block.clone())
    }
}

#[tokio::test]
async fn governed_collateral_fixture_rejects_a_non_governed_rpc_request() {
    let rpc = GovernedCollateralRpcFixture::load().expect("governed RPC fixture must load");

    assert!(
        rpc.eth_call_u256_word_at("wrong-contract", "wrong-calldata", "wrong-block")
            .await
            .is_err()
    );
}

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
        authority: &PolymarketEconomicsAuthority,
        receipt_clock: &dyn EconomicsReceiptClock,
        max_age_ns: u64,
    ) -> anyhow::Result<AuthoritativeValuationObservation> {
        let semantics: serde_json::Value = serde_json::from_str(COLLATERAL_REDEMPTION_SEMANTICS)?;
        anyhow::ensure!(
            semantics["source_commit"].as_str()
                == Some(authority.redemption_semantics_source_commit()),
            "configured redemption semantics commit differs from governed evidence"
        );
        let rpc = GovernedCollateralRpcFixture::load()?;
        let observation = authority
            .observe_collateral_redemption_with_rpc(&rpc, receipt_clock, max_age_ns)
            .await?;
        rpc.assert_all_contract_calls_consumed()?;
        Ok(observation)
    }
}

fn binary_instrument() -> InstrumentAny {
    InstrumentAny::BinaryOption(BinaryOption::new(
        InstrumentId::from(INSTRUMENT_ID),
        Symbol::from("condition-token"),
        AssetClass::Alternative,
        Currency::pUSD(),
        UnixNanos::from(governed_now_ns()),
        UnixNanos::from(governed_now_ns() + 1),
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
        UnixNanos::from(governed_now_ns()),
        UnixNanos::from(governed_now_ns()),
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
        UnixNanos::from(governed_now_ns()),
        UnixNanos::from(governed_now_ns()),
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
            UnixNanos::from(governed_now_ns()),
            UnixNanos::from(governed_now_ns()),
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
        requested_at_ns: governed_now_ns(),
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
        &|| Ok(governed_now_ns()),
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
            requested_at_ns: governed_now_ns(),
            decision_correlation_id: "composition-tracer",
            gross_expected_value: Decimal::from(10),
        })
        .expect("config-derived production routing must quote and admit");
    assert!(admission.guaranteed_debit().amount() > Decimal::ZERO);
    let pool = CapitalPoolSnapshot {
        source: "shadow-capital-fixture".to_string(),
        observed_at_ns: governed_now_ns(),
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
            collateral_currency: "pUSD".to_string(),
            capital_pool: pool,
            policy: CapitalAdmissionPolicy {
                min_remaining_pool_balance: None,
            },
            dedupe_retention_ns: 1,
        },
    );
    submit_state.update_capital_admission_nt_components(BoltV3SubmitCapitalAdmissionNtComponents {
        source: "composition-tracer".to_string(),
        observed_at_ns: governed_now_ns(),
        portfolio: PortfolioCapitalAdmissionSnapshot {
            source: "composition-portfolio".to_string(),
            observed_at_ns: governed_now_ns(),
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            collateral_currency: "pUSD".to_string(),
            free_collateral: Decimal::from(100),
            total_equity: Decimal::from(100),
        },
        venue_spendability: VenueSpendabilitySnapshot {
            source: "composition-spendability".to_string(),
            observed_at_ns: governed_now_ns(),
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            collateral_currency: "pUSD".to_string(),
            spendable_collateral: Decimal::from(100),
            collateral_allowance: Decimal::from(100),
        },
        order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
            source: "composition-orders".to_string(),
            observed_at_ns: governed_now_ns(),
            open_order_count: 0,
            all_open_orders_attributed: true,
        },
        product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
            PredictionMarketAdmissionSnapshot {
                source: "composition-product".to_string(),
                observed_at_ns: governed_now_ns(),
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
        .admit_at(&submit_request, governed_now_ns())
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
        &|| Ok(governed_now_ns()),
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
