//! Strategy-archetype binding for the `binary_oracle_maker` (Slice 2, #488).
//!
//! This module lives under `src/strategies/` (the NON-scanned strategy layer),
//! so it may freely name both the maker strategy layer
//! (`crate::strategies::binary_oracle_maker::*`, `production_strategy_registry`)
//! and the shared bolt-v3 registration/live-node types. It mirrors the taker
//! archetype binding (`crate::bolt_v3_archetypes::binary_oracle_edge_taker`)
//! *structurally* — it owns:
//!
//! 1. `validate_strategy` — the maker's bolt-v3 startup-validation policy and
//!    **go-live gate**. It deserializes the operator
//!    `[strategies.<id>.parameters]` block into [`ParametersBlock`]
//!    (`deny_unknown_fields`, mirroring the taker's
//!    `try_into::<ParametersBlock>()`) and then bounds-checks the μ-estimator /
//!    health-gate runtime knobs ([`validate_parameter_bounds`]) so a degenerate
//!    or never-warming μ fails closed at load instead of at the first dead
//!    trading session.
//! 2. `register_runtime_strategy` — resolves the configured fee provider and
//!    execution venue, builds the `StrategyBuildContext` + flat raw config table
//!    the maker consumes (the NT envelope plus the μ runtime knobs threaded from
//!    `[parameters.runtime]`), and registers the strategy through the shared
//!    `production_strategy_registry()`.
//! 3. `RUNTIME_BINDING` — the `StrategyRuntimeBinding` the production aggregator
//!    (`crate::strategy_bindings`) lists alongside the taker binding.

use rust_decimal::Decimal;
use serde::Deserialize;
use toml::{Value, map::Map};

use nautilus_model::{
    data::{OrderBookDelta, TradeTick},
    identifiers::StrategyId,
};

use crate::bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy};
use crate::bolt_v3_maker_go_live_gate::{
    MakerBacktestEvidence, MakerBacktestVerdict, maker_backtest_gate_blockers,
};
use crate::bolt_v3_maker_market_selection::{
    MakerMarketPortfolioBlocker, MakerMarketPortfolioPolicy, maker_market_portfolio_policy_blockers,
};
use crate::bolt_v3_operator_artifacts::{
    build_head_sha_matches_current, is_lowercase_sha256, json_artifact_sha256,
};
use crate::bolt_v3_providers::resolve_fee_provider;
use crate::bolt_v3_strategy_registration::{
    BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
};
use crate::bolt_v3_trade_flow::SignedTradeFlowConfig;
use crate::strategies::binary_oracle_maker::{BinaryOracleMakerBuilder, KEY};
use crate::strategies::production_strategy_registry;
use crate::strategies::registry::StrategyBuilder;

const NT_BACKTEST_NODE_EXECUTION_MODEL: &str = "nt_backtest_node";

/// The maker runtime binding the production aggregator lists. `key` and
/// `strategy_kind` both resolve to the single archetype constant
/// `binary_oracle_maker`; `register` is this module's `register_runtime_strategy`.
pub const RUNTIME_BINDING: StrategyRuntimeBinding = StrategyRuntimeBinding {
    key: KEY,
    strategy_kind: BinaryOracleMakerBuilder::kind,
    register: register_runtime_strategy,
};

/// Operator `[strategies.<id>.parameters]` block for the maker. Mirrors the
/// taker's `ParametersBlock` shape: runtime-tuning knobs live in a nested
/// `[parameters.runtime]` sub-table so the same knob name sits at the same path
/// across strategies. `deny_unknown_fields` fails loud on any stray key.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ParametersBlock {
    runtime: RuntimeParametersBlock,
    market_portfolio: MarketPortfolioParametersBlock,
    backtest: BacktestParametersBlock,
}

/// Runtime-tuning knobs for the maker's μ (informed-fraction) estimator and its
/// fail-closed health gate. Every value is operator-supplied from TOML; nothing
/// defaults. Unlike the taker's hand-written `Deserialize` (which rejects
/// migrated fields), the maker has no migration history, so the derived
/// `deny_unknown_fields` deserialization is the single source of truth.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RuntimeParametersBlock {
    trade_flow_window_secs: u64,
    trade_flow_max_samples: u64,
    mu_min_classified_samples: u64,
    mu_stale_window_ms: u64,
    mu_min_floor: f64,
    requote_min_interval_ms: u64,
}

/// Operator policy for generic Slice 9 market portfolio selection. Discovery and
/// eligibility live upstream; this policy bounds how many eligible markets may
/// quote concurrently and how bankroll is split across isolated per-market slots.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct MarketPortfolioParametersBlock {
    max_active_markets: usize,
    total_bankroll_notional: f64,
    min_slot_notional: f64,
}

/// Operator-supplied go-live evidence for the maker backtest. These are not
/// strategy runtime knobs; they are explicit startup evidence that the built
/// maker cleared Slice 10 before it can be registered for live quoting.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct BacktestParametersBlock {
    verdict: BacktestVerdictParameter,
    build_head_sha: String,
    strategy_config_hash: String,
    run_artifact: String,
    run_artifact_sha256: String,
    threshold_artifact: String,
    threshold_artifact_sha256: String,
    execution_model_artifact: String,
    execution_model_artifact_sha256: String,
    maker_order_count: u64,
    passive_fill_count: u64,
    min_passive_fill_count: u64,
    resolved_market_count: u64,
    min_resolved_market_count: u64,
    built_maker_replayed: bool,
    captured_spread_score_micros: i64,
    fees_score_micros: i64,
    adverse_selection_score_micros: i64,
    settlement_loss_score_micros: i64,
    net_score_micros: i64,
    thresholds_registered_before_run: bool,
    balanced_gate_evaluated: bool,
    strict_gate_evaluated: bool,
    balanced_gate_passed: bool,
    strict_gate_passed: bool,
    historical_full_depth_l2: bool,
    full_population_corpus: bool,
    entry_gated_corpus_used: bool,
    result_contract_replay: BacktestResultContractReplayBlock,
    custom_fill_model_used: bool,
    custom_fill_model_source_proven: bool,
    underlying_spot_causal_join: bool,
    statistical_significance: bool,
    shared_fair_value_pricing: bool,
    shared_settlement_primitive: bool,
}

/// Replay-specific fields copied from BTE's objective BacktestResultContract.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct BacktestResultContractReplayBlock {
    strategy_config_hash: String,
    execution_model: String,
    venue_queue_position: bool,
    catalog_data_types: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BacktestVerdictParameter {
    Pass,
    Fail,
}

impl BacktestParametersBlock {
    fn evidence(&self) -> MakerBacktestEvidence {
        MakerBacktestEvidence {
            verdict: match self.verdict {
                BacktestVerdictParameter::Pass => MakerBacktestVerdict::Pass,
                BacktestVerdictParameter::Fail => MakerBacktestVerdict::Fail,
            },
            build_head_sha_valid: build_head_sha_matches_current(&self.build_head_sha),
            strategy_config_hash_valid: is_lowercase_sha256(&self.strategy_config_hash),
            result_contract_strategy_config_hash_valid: self
                .result_contract_replay
                .strategy_config_hash_matches(&self.strategy_config_hash),
            run_artifact_present: artifact_present(&self.run_artifact),
            run_artifact_sha256_valid: is_lowercase_sha256(&self.run_artifact_sha256),
            threshold_artifact_present: artifact_present(&self.threshold_artifact),
            threshold_artifact_sha256_valid: is_lowercase_sha256(&self.threshold_artifact_sha256),
            execution_model_artifact_present: artifact_present(&self.execution_model_artifact),
            execution_model_artifact_sha256_valid: is_lowercase_sha256(
                &self.execution_model_artifact_sha256,
            ),
            maker_orders_observed: self.maker_order_count > 0,
            passive_fills_observed: self.passive_fill_count > 0,
            built_maker_replayed: self.built_maker_replayed,
            full_net_scoring: self.net_score_reconciles(),
            thresholds_registered_before_run: self.thresholds_registered_before_run,
            balanced_gate_evaluated: self.balanced_gate_evaluated,
            strict_gate_evaluated: self.strict_gate_evaluated,
            balanced_gate_passed: self.balanced_gate_passed,
            strict_gate_passed: self.strict_gate_passed,
            historical_full_depth_l2: self.historical_full_depth_l2,
            full_population_corpus: self.full_population_corpus,
            entry_gated_corpus_used: self.entry_gated_corpus_used,
            trade_ticks_present: self.result_contract_replay.trade_ticks_present(),
            order_book_deltas_present: self.result_contract_replay.order_book_deltas_present(),
            queue_position_enabled: self.result_contract_replay.venue_queue_position,
            nt_execution_model_used: self.result_contract_replay.nt_backtest_node_used(),
            custom_fill_model_used: self.custom_fill_model_used,
            custom_fill_model_source_proven: self.custom_fill_model_source_proven,
            underlying_spot_causal_join: self.underlying_spot_causal_join,
            net_edge_positive: self.net_score_micros > 0,
            statistical_significance: self.statistical_significance,
            passive_fill_power_floor: self.passive_fill_floor_met(),
            resolved_market_corpus_floor: self.resolved_market_floor_met(),
            shared_fair_value_pricing: self.shared_fair_value_pricing,
            shared_settlement_primitive: self.shared_settlement_primitive,
        }
    }

    fn net_score_reconciles(&self) -> bool {
        i128::from(self.captured_spread_score_micros)
            - i128::from(self.fees_score_micros)
            - i128::from(self.adverse_selection_score_micros)
            - i128::from(self.settlement_loss_score_micros)
            == i128::from(self.net_score_micros)
    }

    fn passive_fill_floor_met(&self) -> bool {
        self.min_passive_fill_count > 0 && self.passive_fill_count >= self.min_passive_fill_count
    }

    fn resolved_market_floor_met(&self) -> bool {
        self.min_resolved_market_count > 0
            && self.resolved_market_count >= self.min_resolved_market_count
    }
}

impl BacktestResultContractReplayBlock {
    fn strategy_config_hash_matches(&self, expected: &str) -> bool {
        is_lowercase_sha256(&self.strategy_config_hash) && self.strategy_config_hash == expected
    }

    fn trade_ticks_present(&self) -> bool {
        self.catalog_data_types_contains(nt_contract_data_type_name::<TradeTick>())
    }

    fn order_book_deltas_present(&self) -> bool {
        self.catalog_data_types_contains(nt_contract_data_type_name::<OrderBookDelta>())
    }

    fn nt_backtest_node_used(&self) -> bool {
        self.execution_model.trim() == NT_BACKTEST_NODE_EXECUTION_MODEL
    }

    fn catalog_data_types_contains(&self, expected: &str) -> bool {
        self.catalog_data_types
            .iter()
            .any(|data_type| data_type.trim() == expected)
    }
}

impl MarketPortfolioParametersBlock {
    fn policy(&self) -> MakerMarketPortfolioPolicy {
        MakerMarketPortfolioPolicy {
            max_active_markets: self.max_active_markets,
            total_bankroll_notional: self.total_bankroll_notional,
            min_slot_notional: self.min_slot_notional,
        }
    }
}

fn artifact_present(value: &str) -> bool {
    !value.trim().is_empty()
}

fn nt_contract_data_type_name<T>() -> &'static str {
    let type_name = std::any::type_name::<T>();
    type_name.rsplit(':').next().unwrap_or(type_name)
}

/// Bolt-v3 startup validation and **go-live gate** for the maker.
///
/// Confirms the archetype key, then deserializes the operator `[parameters]`
/// block into [`ParametersBlock`] (`deny_unknown_fields`, mirroring the taker's
/// `strategy.parameters.try_into::<ParametersBlock>()`) and bounds-checks the μ
/// runtime knobs ([`validate_parameter_bounds`]). A malformed block or an
/// out-of-bounds knob fails closed at load. `context` and `_default_max_notional`
/// mirror the taker validator's signature so the function is assignable to
/// `ArchetypeValidationBinding::validate_strategy`; the risk-cap parameter is
/// unused until later slices add notional parameters.
pub fn validate_strategy(
    context: &str,
    _root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
    _default_max_notional: Option<&Decimal>,
) -> Vec<String> {
    if strategy.strategy_archetype.as_str() != KEY {
        return vec![format!(
            "{context}: strategy_archetype `{}` is not `{KEY}`",
            strategy.strategy_archetype.as_str()
        )];
    }
    let parameters = match strategy.parameters.clone().try_into::<ParametersBlock>() {
        Ok(value) => value,
        Err(error) => {
            return vec![format!(
                "{context}: parameters block is not a valid `{KEY}` [parameters] block: {error}"
            )];
        }
    };
    let mut errors = validate_parameter_bounds(context, &parameters);
    validate_strategy_config_hash_binding(context, strategy, &parameters, &mut errors);
    errors
}

/// Fail-closed bounds for the maker's μ runtime knobs (the go-live gate). Each
/// rejected shape would otherwise yield a silently degenerate or never-producible
/// μ at runtime — a fail-soft dead strategy — so it must fail closed at load:
///
/// - a zero retention window, sample cap, or classified-sample minimum means the
///   estimator can never warm or the buffer never retains a trade, so μ is never
///   produced;
/// - a retention window so large its millisecond conversion (`window_secs × 1000`)
///   overflows `u64` would silently saturate to a near-infinite window instead of
///   the value the operator wrote;
/// - a classified-sample minimum above the sample cap is unsatisfiable (the
///   buffer can never hold that many classified samples), so μ is never produced;
/// - a zero staleness window marks every reading stale, blocking μ permanently;
/// - a μ floor outside the open interval `(0, 1)` is degenerate: a floor of `0`
///   admits the constant-0 μ the health gate exists to reject, and a floor `>= 1`
///   blocks every non-degenerate μ.
fn validate_parameter_bounds(context: &str, parameters: &ParametersBlock) -> Vec<String> {
    let runtime = &parameters.runtime;
    let market_portfolio = &parameters.market_portfolio;
    let mut errors = Vec::new();
    if runtime.trade_flow_window_secs == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.trade_flow_window_secs must be > 0 (a zero retention window holds no trades, so a μ can never be produced)"
        ));
    }
    let trade_flow = SignedTradeFlowConfig {
        window_secs: runtime.trade_flow_window_secs,
        max_samples: runtime.trade_flow_max_samples,
    };
    if trade_flow.window_ms().is_none() {
        errors.push(format!(
            "{context}: parameters.runtime.trade_flow_window_secs ({}) must be small enough that its second-to-millisecond conversion does not overflow u64 (a larger window silently saturates the retention window instead of meaning the configured value)",
            runtime.trade_flow_window_secs
        ));
    }
    if runtime.trade_flow_max_samples == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.trade_flow_max_samples must be > 0 (a zero sample cap retains no trades, so a μ can never be produced)"
        ));
    }
    if runtime.mu_min_classified_samples == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.mu_min_classified_samples must be > 0 (a zero warmup threshold would admit a μ from an empty window)"
        ));
    }
    if runtime.mu_min_classified_samples > runtime.trade_flow_max_samples {
        errors.push(format!(
            "{context}: parameters.runtime.mu_min_classified_samples ({}) must be <= parameters.runtime.trade_flow_max_samples ({}) (a warmup threshold above the buffer cap is unsatisfiable, so a μ can never be produced)",
            runtime.mu_min_classified_samples, runtime.trade_flow_max_samples
        ));
    }
    if runtime.mu_stale_window_ms == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.mu_stale_window_ms must be > 0 (a zero staleness window marks every reading stale, blocking μ permanently)"
        ));
    }
    if !crate::bolt_v3_numeric::is_positive_finite(runtime.mu_min_floor)
        || runtime.mu_min_floor >= crate::bolt_v3_numeric::UNIT_F64
    {
        errors.push(format!(
            "{context}: parameters.runtime.mu_min_floor ({}) must be finite and in the open interval (0, 1) (a floor of 0 admits the degenerate constant-0 μ the health gate rejects; a floor >= 1 blocks every non-degenerate μ)",
            runtime.mu_min_floor
        ));
    }
    if runtime.requote_min_interval_ms == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.requote_min_interval_ms must be > 0 (a zero requote interval disables the same-tick throttle the requote budget relies on, so the budget rejects construction)"
        ));
    }
    validate_market_portfolio_policy(context, market_portfolio, &mut errors);
    for blocker in maker_backtest_gate_blockers(&parameters.backtest.evidence()) {
        errors.push(format!(
            "{context}: parameters.backtest.{} {}",
            blocker.parameter_path(),
            blocker.required_state()
        ));
    }
    errors
}

fn validate_market_portfolio_policy(
    context: &str,
    market_portfolio: &MarketPortfolioParametersBlock,
    errors: &mut Vec<String>,
) {
    let policy = market_portfolio.policy();
    for blocker in maker_market_portfolio_policy_blockers(policy) {
        errors.push(format!(
            "{context}: parameters.market_portfolio.{} {}",
            market_portfolio_blocker_parameter_path(blocker),
            market_portfolio_blocker_required_state(blocker)
        ));
    }
    if crate::bolt_v3_numeric::is_positive_finite(policy.total_bankroll_notional)
        && crate::bolt_v3_numeric::is_positive_finite(policy.min_slot_notional)
        && policy.total_bankroll_notional < policy.min_slot_notional
    {
        errors.push(format!(
            "{context}: parameters.market_portfolio.total_bankroll_notional must be >= parameters.market_portfolio.min_slot_notional (otherwise no market slot can receive the configured minimum allocation)"
        ));
    }
}

fn validate_strategy_config_hash_binding(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    parameters: &ParametersBlock,
    errors: &mut Vec<String>,
) {
    if !is_lowercase_sha256(&parameters.backtest.strategy_config_hash) {
        return;
    }
    match maker_strategy_config_hash(strategy, parameters) {
        Ok(expected) if parameters.backtest.strategy_config_hash == expected => {}
        Ok(_) => errors.push(format!(
            "{context}: parameters.backtest.strategy_config_hash must match the canonical maker strategy config hash for this loaded strategy"
        )),
        Err(message) => errors.push(format!(
            "{context}: parameters.backtest.strategy_config_hash could not be computed from this loaded strategy: {message}"
        )),
    }
}

fn maker_strategy_config_hash(
    strategy: &BoltV3StrategyConfig,
    parameters: &ParametersBlock,
) -> Result<String, String> {
    let raw = raw_maker_config_from_parts(strategy, parameters)?;
    json_artifact_sha256(&raw).map_err(|error| error.to_string())
}

fn market_portfolio_blocker_parameter_path(
    blocker: MakerMarketPortfolioBlocker<'_>,
) -> &'static str {
    match blocker {
        MakerMarketPortfolioBlocker::InvalidMaxActiveMarkets => "max_active_markets",
        MakerMarketPortfolioBlocker::InvalidTotalBankroll => "total_bankroll_notional",
        MakerMarketPortfolioBlocker::InvalidMinSlotNotional => "min_slot_notional",
        MakerMarketPortfolioBlocker::EmptyCandidateMarketKey
        | MakerMarketPortfolioBlocker::DuplicateCandidateMarket { .. }
        | MakerMarketPortfolioBlocker::EmptyActiveMarketKey
        | MakerMarketPortfolioBlocker::DuplicateActiveMarket { .. }
        | MakerMarketPortfolioBlocker::NoEligibleCandidates
        | MakerMarketPortfolioBlocker::InsufficientSlotAllocation => "policy",
    }
}

fn market_portfolio_blocker_required_state(
    blocker: MakerMarketPortfolioBlocker<'_>,
) -> &'static str {
    match blocker {
        MakerMarketPortfolioBlocker::InvalidMaxActiveMarkets => {
            "must be > 0 so the maker can select at least one market"
        }
        MakerMarketPortfolioBlocker::InvalidTotalBankroll => {
            "must be a positive finite bankroll notional"
        }
        MakerMarketPortfolioBlocker::InvalidMinSlotNotional => {
            "must be a positive finite per-market slot notional"
        }
        MakerMarketPortfolioBlocker::EmptyCandidateMarketKey
        | MakerMarketPortfolioBlocker::DuplicateCandidateMarket { .. }
        | MakerMarketPortfolioBlocker::EmptyActiveMarketKey
        | MakerMarketPortfolioBlocker::DuplicateActiveMarket { .. }
        | MakerMarketPortfolioBlocker::NoEligibleCandidates
        | MakerMarketPortfolioBlocker::InsufficientSlotAllocation => {
            "must be valid when candidate markets are discovered"
        }
    }
}

/// Register the maker on the live node.
///
/// Mirrors the taker's `register_runtime_strategy` structurally: resolve the fee
/// provider and execution venue from the loaded config, build a
/// `StrategyBuildContext`, then hand the flat raw config table to the shared
/// `production_strategy_registry()`. The raw table carries the NautilusTrader
/// envelope fields (`strategy_id`, `order_id_tag`, `oms_type`, `client_id`) plus
/// the μ runtime knobs `raw_maker_config` threads from the operator
/// `[parameters.runtime]` block.
pub fn register_runtime_strategy(
    node: &mut nautilus_live::node::LiveNode,
    context: StrategyRegistrationContext<'_>,
) -> Result<StrategyId, BoltV3StrategyRegistrationError> {
    let raw =
        raw_maker_config(context.strategy).map_err(|message| binding_message(&context, message))?;
    let fee_provider = resolve_fee_provider(
        context.loaded,
        context.strategy.config.execution_client_id.as_str(),
        context.resolved,
    )
    .map_err(|error| binding_message(&context, error.to_string()))?;
    let execution_client_id = context.strategy.config.execution_client_id.as_str();
    let execution_venue = context
        .loaded
        .root
        .clients
        .get(execution_client_id)
        .map(|client| client.venue)
        .ok_or_else(|| {
            binding_message(
                &context,
                format!(
                    "execution_client_id `{execution_client_id}` is not present in loaded clients for execution-venue resolution"
                ),
            )
        })?;
    let build_context = crate::strategies::registry::StrategyBuildContext::new(
        fee_provider,
        context.decision_evidence.clone(),
        context.submit_admission.clone(),
        context.order_execution_policy,
        execution_venue,
    )
    .with_realized_volatility_runtime(context.realized_volatility_runtime.clone());
    let registry = production_strategy_registry()
        .map_err(|error| binding_message(&context, error.to_string()))?;
    registry
        .register_strategy(
            BinaryOracleMakerBuilder::kind(),
            &raw,
            &build_context,
            node.kernel().trader(),
        )
        .map_err(|error| binding_message(&context, error.to_string()))
}

/// Build the flat raw config table the maker consumes. The NautilusTrader
/// strategy id is `<strategy_archetype>-<order_id_tag>` (validated as an NT
/// `StrategyId`), mirroring the taker's `nt_strategy_id`; `oms_type` is the
/// lowercased NT enum display, matching how the maker config deserializes it.
/// `client_id` is the configured execution client id the runtime submit/cancel
/// bridge passes into NT routing context. The μ runtime knobs and market
/// portfolio policy are read from the operator `[parameters]` block and threaded
/// in flat under the same names `BinaryOracleMakerConfig` consumes.
fn raw_maker_config(strategy: &LoadedStrategy) -> Result<Value, String> {
    raw_maker_config_from_config(&strategy.config)
}

fn raw_maker_config_from_config(strategy: &BoltV3StrategyConfig) -> Result<Value, String> {
    if strategy.strategy_archetype.as_str() != KEY {
        return Err(format!(
            "strategy_archetype `{}` is not `{KEY}`",
            strategy.strategy_archetype.as_str()
        ));
    }
    let parameters: ParametersBlock = strategy
        .parameters
        .clone()
        .try_into()
        .map_err(|error| format!("invalid [parameters] block: {error}"))?;
    raw_maker_config_from_parts(strategy, &parameters)
}

fn raw_maker_config_from_parts(
    strategy: &BoltV3StrategyConfig,
    parameters: &ParametersBlock,
) -> Result<Value, String> {
    let runtime = &parameters.runtime;
    let market_portfolio = &parameters.market_portfolio;

    let mut strategy_id = strategy.strategy_archetype.as_str().to_string();
    strategy_id.push('-');
    strategy_id.push_str(&strategy.order_id_tag);
    StrategyId::new_checked(&strategy_id)
        .map_err(|error| format!("maps to invalid NT StrategyId `{strategy_id}`: {error}"))?;

    let mut table = Map::new();
    table.insert("strategy_id".to_string(), Value::String(strategy_id));
    table.insert(
        "order_id_tag".to_string(),
        Value::String(strategy.order_id_tag.clone()),
    );
    table.insert(
        "oms_type".to_string(),
        Value::String(strategy.oms_type.to_string().to_ascii_lowercase()),
    );
    table.insert(
        "client_id".to_string(),
        Value::String(strategy.execution_client_id.to_string()),
    );
    insert_runtime_knobs(&mut table, runtime)?;
    insert_market_portfolio_knobs(&mut table, market_portfolio)?;
    Ok(Value::Table(table))
}

/// Thread the μ runtime knobs from the operator `[parameters.runtime]` block into
/// the flat config table under the exact field names `BinaryOracleMakerConfig`
/// consumes. Factored out so the operator → flat-table → consumer-config bridge
/// is unit-testable in isolation (a key-name drift here fails the flat table's
/// `deny_unknown_fields` deserialization at `parse_config`).
fn insert_runtime_knobs(
    table: &mut Map<String, Value>,
    runtime: &RuntimeParametersBlock,
) -> Result<(), String> {
    insert_u64_field(
        table,
        "trade_flow_window_secs",
        runtime.trade_flow_window_secs,
    )?;
    insert_u64_field(
        table,
        "trade_flow_max_samples",
        runtime.trade_flow_max_samples,
    )?;
    insert_u64_field(
        table,
        "mu_min_classified_samples",
        runtime.mu_min_classified_samples,
    )?;
    insert_u64_field(table, "mu_stale_window_ms", runtime.mu_stale_window_ms)?;
    table.insert(
        "mu_min_floor".to_string(),
        Value::Float(runtime.mu_min_floor),
    );
    insert_u64_field(
        table,
        "requote_min_interval_ms",
        runtime.requote_min_interval_ms,
    )?;
    Ok(())
}

/// Thread the operator `[parameters.market_portfolio]` policy into the flat
/// config table under the exact field names `BinaryOracleMakerConfig` consumes.
fn insert_market_portfolio_knobs(
    table: &mut Map<String, Value>,
    market_portfolio: &MarketPortfolioParametersBlock,
) -> Result<(), String> {
    insert_usize_field(
        table,
        "market_portfolio_max_active_markets",
        market_portfolio.max_active_markets,
    )?;
    table.insert(
        "market_portfolio_total_bankroll_notional".to_string(),
        Value::Float(market_portfolio.total_bankroll_notional),
    );
    table.insert(
        "market_portfolio_min_slot_notional".to_string(),
        Value::Float(market_portfolio.min_slot_notional),
    );
    Ok(())
}

/// Insert a `u64` runtime knob into the flat config table as a TOML integer.
/// TOML integers are signed 64-bit, so a value above `i64::MAX` cannot round-trip
/// and fails closed here rather than silently wrapping.
fn insert_u64_field(table: &mut Map<String, Value>, key: &str, value: u64) -> Result<(), String> {
    let integer = i64::try_from(value).map_err(|_| {
        format!("runtime knob `{key}` ({value}) exceeds the supported TOML integer range")
    })?;
    table.insert(key.to_string(), Value::Integer(integer));
    Ok(())
}

fn insert_usize_field(
    table: &mut Map<String, Value>,
    key: &str,
    value: usize,
) -> Result<(), String> {
    let integer = i64::try_from(value).map_err(|_| {
        format!("runtime knob `{key}` ({value}) exceeds the supported TOML integer range")
    })?;
    table.insert(key.to_string(), Value::Integer(integer));
    Ok(())
}

/// Wrap a registration failure message in the shared
/// `BoltV3StrategyRegistrationError::Binding` variant, mirroring the taker's
/// `binding_message`.
fn binding_message(
    context: &StrategyRegistrationContext<'_>,
    message: String,
) -> BoltV3StrategyRegistrationError {
    BoltV3StrategyRegistrationError::Binding {
        strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
        strategy_archetype: context
            .strategy
            .config
            .strategy_archetype
            .as_str()
            .to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXT: &str = "strategy `maker-001`";
    const TEST_ARTIFACT_SHA256: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn valid_runtime() -> RuntimeParametersBlock {
        RuntimeParametersBlock {
            trade_flow_window_secs: 600,
            trade_flow_max_samples: 1000,
            mu_min_classified_samples: 4,
            mu_stale_window_ms: 60_000,
            mu_min_floor: 0.05,
            requote_min_interval_ms: 500,
        }
    }

    fn valid_market_portfolio() -> MarketPortfolioParametersBlock {
        MarketPortfolioParametersBlock {
            max_active_markets: 3,
            total_bankroll_notional: 1500.0,
            min_slot_notional: 100.0,
        }
    }

    fn valid_result_contract_replay() -> BacktestResultContractReplayBlock {
        BacktestResultContractReplayBlock {
            strategy_config_hash: TEST_ARTIFACT_SHA256.to_string(),
            execution_model: NT_BACKTEST_NODE_EXECUTION_MODEL.to_string(),
            venue_queue_position: true,
            catalog_data_types: vec![
                nt_contract_data_type_name::<OrderBookDelta>().to_string(),
                nt_contract_data_type_name::<TradeTick>().to_string(),
            ],
        }
    }

    fn current_test_build_head_sha() -> String {
        crate::bolt_v3_operator_artifacts::current_build_head_sha()
            .expect("test binary must carry BOLT_V3_BUILD_HEAD_SHA")
            .to_string()
    }

    fn mismatched_test_build_head_sha() -> String {
        let current = current_test_build_head_sha();
        let mismatch = "0123456789abcdef0123456789abcdef01234567";
        if current == mismatch {
            "89abcdef0123456789abcdef0123456789abcdef".to_string()
        } else {
            mismatch.to_string()
        }
    }

    fn valid_backtest() -> BacktestParametersBlock {
        BacktestParametersBlock {
            verdict: BacktestVerdictParameter::Pass,
            build_head_sha: current_test_build_head_sha(),
            strategy_config_hash: TEST_ARTIFACT_SHA256.to_string(),
            run_artifact: "artifact://maker/backtest/run".to_string(),
            run_artifact_sha256: TEST_ARTIFACT_SHA256.to_string(),
            threshold_artifact: "artifact://maker/backtest/thresholds".to_string(),
            threshold_artifact_sha256: TEST_ARTIFACT_SHA256.to_string(),
            execution_model_artifact: "artifact://maker/backtest/execution-model".to_string(),
            execution_model_artifact_sha256: TEST_ARTIFACT_SHA256.to_string(),
            maker_order_count: 3,
            passive_fill_count: 2,
            min_passive_fill_count: 2,
            resolved_market_count: 5,
            min_resolved_market_count: 5,
            built_maker_replayed: true,
            captured_spread_score_micros: 1_000,
            fees_score_micros: 100,
            adverse_selection_score_micros: 200,
            settlement_loss_score_micros: 300,
            net_score_micros: 400,
            thresholds_registered_before_run: true,
            balanced_gate_evaluated: true,
            strict_gate_evaluated: true,
            balanced_gate_passed: true,
            strict_gate_passed: false,
            historical_full_depth_l2: true,
            full_population_corpus: true,
            entry_gated_corpus_used: false,
            result_contract_replay: valid_result_contract_replay(),
            custom_fill_model_used: false,
            custom_fill_model_source_proven: false,
            underlying_spot_causal_join: true,
            statistical_significance: true,
            shared_fair_value_pricing: true,
            shared_settlement_primitive: true,
        }
    }

    fn valid_backtest_toml() -> String {
        valid_backtest_toml_with_strategy_config_hash(TEST_ARTIFACT_SHA256)
    }

    fn valid_backtest_toml_with_strategy_config_hash(strategy_config_hash: &str) -> String {
        format!(
            r#"
            [backtest]
            verdict = "pass"
            build_head_sha = "{}"
            strategy_config_hash = "{}"
            run_artifact = "artifact://maker/backtest/run"
            run_artifact_sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            threshold_artifact = "artifact://maker/backtest/thresholds"
            threshold_artifact_sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            execution_model_artifact = "artifact://maker/backtest/execution-model"
            execution_model_artifact_sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            maker_order_count = 3
            passive_fill_count = 2
            min_passive_fill_count = 2
            resolved_market_count = 5
            min_resolved_market_count = 5
            built_maker_replayed = true
            captured_spread_score_micros = 1000
            fees_score_micros = 100
            adverse_selection_score_micros = 200
            settlement_loss_score_micros = 300
            net_score_micros = 400
            thresholds_registered_before_run = true
            balanced_gate_evaluated = true
            strict_gate_evaluated = true
            balanced_gate_passed = true
            strict_gate_passed = false
            historical_full_depth_l2 = true
            full_population_corpus = true
            entry_gated_corpus_used = false
            custom_fill_model_used = false
            custom_fill_model_source_proven = false
            underlying_spot_causal_join = true
            statistical_significance = true
            shared_fair_value_pricing = true
            shared_settlement_primitive = true

            [backtest.result_contract_replay]
            strategy_config_hash = "{}"
            execution_model = "nt_backtest_node"
            venue_queue_position = true
            catalog_data_types = ["OrderBookDelta", "TradeTick"]
            "#,
            current_test_build_head_sha(),
            strategy_config_hash,
            strategy_config_hash
        )
    }

    fn prefixed_valid_backtest_toml(strategy_config_hash: &str) -> String {
        valid_backtest_toml_with_strategy_config_hash(strategy_config_hash)
            .replace("[backtest", "[parameters.backtest")
    }

    const VALID_MARKET_PORTFOLIO_TOML: &str = r#"
            [market_portfolio]
            max_active_markets = 3
            total_bankroll_notional = 1500.0
            min_slot_notional = 100.0
            "#;

    fn valid_strategy_toml(strategy_config_hash: &str) -> String {
        format!(
            r#"
            schema_version = 2
            strategy_instance_id = "maker-001"
            strategy_archetype = "binary_oracle_maker"
            order_id_tag = "001"
            oms_type = "netting"
            use_uuid_client_order_ids = true
            use_hyphens_in_client_order_ids = false
            external_order_claims = []
            manage_contingent_orders = false
            manage_gtd_expiry = false
            manage_stop = false
            market_exit_interval_ms = 100
            market_exit_max_attempts = 100
            log_events = true
            log_commands = true
            log_rejected_due_post_only_as_warning = true
            execution_client_id = "maker_execution"

            [target]
            kind = "test"

            [signal_data.primary]
            data_client_id = "maker_data"
            instrument_id = "GENERIC.TEST"

            [parameters.runtime]
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500

            [parameters.market_portfolio]
            max_active_markets = 3
            total_bankroll_notional = 1500.0
            min_slot_notional = 100.0

            {}
            "#,
            prefixed_valid_backtest_toml(strategy_config_hash)
        )
    }

    fn valid_strategy_config_with_hash(strategy_config_hash: &str) -> BoltV3StrategyConfig {
        toml::from_str(&valid_strategy_toml(strategy_config_hash))
            .expect("valid maker strategy config fixture parses")
    }

    fn valid_strategy_config() -> BoltV3StrategyConfig {
        let placeholder = valid_strategy_config_with_hash(TEST_ARTIFACT_SHA256);
        let parameters: ParametersBlock = placeholder
            .parameters
            .clone()
            .try_into()
            .expect("placeholder parameters parse");
        let expected_hash = maker_strategy_config_hash(&placeholder, &parameters)
            .expect("maker strategy config hash computes");
        valid_strategy_config_with_hash(&expected_hash)
    }

    fn mismatched_strategy_config_hash(expected_hash: &str) -> String {
        if expected_hash == TEST_ARTIFACT_SHA256 {
            "89abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567".to_string()
        } else {
            TEST_ARTIFACT_SHA256.to_string()
        }
    }

    fn bounds_errors(runtime: RuntimeParametersBlock) -> Vec<String> {
        validate_parameter_bounds(
            CONTEXT,
            &ParametersBlock {
                runtime,
                market_portfolio: valid_market_portfolio(),
                backtest: valid_backtest(),
            },
        )
    }

    fn market_portfolio_errors(market_portfolio: MarketPortfolioParametersBlock) -> Vec<String> {
        validate_parameter_bounds(
            CONTEXT,
            &ParametersBlock {
                runtime: valid_runtime(),
                market_portfolio,
                backtest: valid_backtest(),
            },
        )
    }

    fn backtest_errors(backtest: BacktestParametersBlock) -> Vec<String> {
        validate_parameter_bounds(
            CONTEXT,
            &ParametersBlock {
                runtime: valid_runtime(),
                market_portfolio: valid_market_portfolio(),
                backtest,
            },
        )
    }

    #[test]
    fn runtime_binding_key_is_archetype_key() {
        assert_eq!(RUNTIME_BINDING.key, "binary_oracle_maker");
        assert_eq!(RUNTIME_BINDING.key, KEY);
    }

    #[test]
    fn runtime_binding_strategy_kind_matches_key() {
        assert_eq!((RUNTIME_BINDING.strategy_kind)(), KEY);
    }

    #[test]
    fn validate_parameter_bounds_accepts_valid_runtime() {
        assert!(
            bounds_errors(valid_runtime()).is_empty(),
            "valid runtime knobs must pass the go-live gate"
        );
    }

    #[test]
    fn validate_parameter_bounds_accepts_valid_market_portfolio_policy() {
        assert!(
            market_portfolio_errors(valid_market_portfolio()).is_empty(),
            "valid market portfolio policy must pass the go-live gate"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_window() {
        let errors = bounds_errors(RuntimeParametersBlock {
            trade_flow_window_secs: 0,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("trade_flow_window_secs")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_window_secs_that_overflows_millis() {
        // A window_secs whose × 1000 millisecond conversion overflows u64 would
        // silently saturate the retention window instead of meaning the configured
        // value; the go-live gate must reject it loud.
        let errors = bounds_errors(RuntimeParametersBlock {
            trade_flow_window_secs: u64::MAX,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("silently saturates the retention window")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_max_samples() {
        let errors = bounds_errors(RuntimeParametersBlock {
            trade_flow_max_samples: 0,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("trade_flow_max_samples")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_min_classified() {
        let errors = bounds_errors(RuntimeParametersBlock {
            mu_min_classified_samples: 0,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("mu_min_classified_samples")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_min_classified_above_cap() {
        // A warmup threshold above the buffer cap is unsatisfiable: the buffer can
        // never hold that many classified samples, so μ is never produced.
        let errors = bounds_errors(RuntimeParametersBlock {
            mu_min_classified_samples: 1001,
            trade_flow_max_samples: 1000,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("must be <= parameters.runtime.trade_flow_max_samples")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_allows_min_classified_equal_to_cap() {
        // The boundary (threshold == cap) is satisfiable, so it must not be rejected.
        let errors = bounds_errors(RuntimeParametersBlock {
            mu_min_classified_samples: 1000,
            trade_flow_max_samples: 1000,
            ..valid_runtime()
        });
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_stale_window() {
        let errors = bounds_errors(RuntimeParametersBlock {
            mu_stale_window_ms: 0,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("mu_stale_window_ms")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_floor_at_or_below_zero() {
        for floor in [0.0, -0.1] {
            let errors = bounds_errors(RuntimeParametersBlock {
                mu_min_floor: floor,
                ..valid_runtime()
            });
            assert!(
                errors.iter().any(|error| error.contains("mu_min_floor")),
                "floor {floor} must be rejected: {errors:?}"
            );
        }
    }

    #[test]
    fn validate_parameter_bounds_rejects_floor_at_or_above_one() {
        for floor in [1.0, 1.5] {
            let errors = bounds_errors(RuntimeParametersBlock {
                mu_min_floor: floor,
                ..valid_runtime()
            });
            assert!(
                errors.iter().any(|error| error.contains("mu_min_floor")),
                "floor {floor} must be rejected: {errors:?}"
            );
        }
    }

    #[test]
    fn validate_parameter_bounds_rejects_non_finite_floor() {
        for floor in [f64::NAN, f64::INFINITY] {
            let errors = bounds_errors(RuntimeParametersBlock {
                mu_min_floor: floor,
                ..valid_runtime()
            });
            assert!(
                errors.iter().any(|error| error.contains("mu_min_floor")),
                "non-finite floor must be rejected: {errors:?}"
            );
        }
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_requote_interval() {
        // A zero requote interval disables the same-tick throttle the requote
        // budget relies on; `build_requote_budget_pair` rejects it, so the go-live
        // gate must reject it loud at load rather than at first quote.
        let errors = bounds_errors(RuntimeParametersBlock {
            requote_min_interval_ms: 0,
            ..valid_runtime()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("requote_min_interval_ms")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_zero_market_concurrency_cap() {
        let errors = market_portfolio_errors(MarketPortfolioParametersBlock {
            max_active_markets: 0,
            ..valid_market_portfolio()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.market_portfolio.max_active_markets")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_invalid_market_bankroll() {
        for total_bankroll_notional in [0.0, f64::NAN] {
            let errors = market_portfolio_errors(MarketPortfolioParametersBlock {
                total_bankroll_notional,
                ..valid_market_portfolio()
            });
            assert!(
                errors
                    .iter()
                    .any(|error| error
                        .contains("parameters.market_portfolio.total_bankroll_notional")),
                "bankroll {total_bankroll_notional} must be rejected: {errors:?}"
            );
        }
    }

    #[test]
    fn validate_parameter_bounds_rejects_invalid_market_slot_floor() {
        for min_slot_notional in [0.0, f64::INFINITY] {
            let errors = market_portfolio_errors(MarketPortfolioParametersBlock {
                min_slot_notional,
                ..valid_market_portfolio()
            });
            assert!(
                errors
                    .iter()
                    .any(|error| error.contains("parameters.market_portfolio.min_slot_notional")),
                "slot floor {min_slot_notional} must be rejected: {errors:?}"
            );
        }
    }

    #[test]
    fn validate_parameter_bounds_rejects_bankroll_below_minimum_slot() {
        let errors = market_portfolio_errors(MarketPortfolioParametersBlock {
            total_bankroll_notional: 99.0,
            min_slot_notional: 100.0,
            ..valid_market_portfolio()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("total_bankroll_notional must be >=")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_failed_backtest_verdict() {
        let errors = backtest_errors(BacktestParametersBlock {
            verdict: BacktestVerdictParameter::Fail,
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.verdict")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_missing_backtest_artifacts() {
        let errors = backtest_errors(BacktestParametersBlock {
            run_artifact: "   ".to_string(),
            threshold_artifact: String::new(),
            execution_model_artifact: String::new(),
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.run_artifact")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.threshold_artifact")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.execution_model_artifact")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_missing_backtest_build_identity() {
        let errors = backtest_errors(BacktestParametersBlock {
            build_head_sha: "not-a-git-sha".to_string(),
            strategy_config_hash: "ABC".to_string(),
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.build_head_sha")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.strategy_config_hash")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_mismatched_backtest_build_head_sha() {
        let errors = backtest_errors(BacktestParametersBlock {
            build_head_sha: mismatched_test_build_head_sha(),
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.build_head_sha")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_strategy_config_hash_binding_accepts_current_raw_maker_config() {
        let strategy = valid_strategy_config();
        let parameters: ParametersBlock = strategy
            .parameters
            .clone()
            .try_into()
            .expect("valid parameters parse");
        let mut errors = Vec::new();
        validate_strategy_config_hash_binding(CONTEXT, &strategy, &parameters, &mut errors);
        assert!(
            errors.is_empty(),
            "matching strategy config hash must pass: {errors:?}"
        );
    }

    #[test]
    fn validate_strategy_config_hash_binding_rejects_mismatched_hash() {
        let expected = {
            let strategy = valid_strategy_config_with_hash(TEST_ARTIFACT_SHA256);
            let parameters: ParametersBlock = strategy
                .parameters
                .clone()
                .try_into()
                .expect("placeholder parameters parse");
            maker_strategy_config_hash(&strategy, &parameters)
                .expect("maker strategy config hash computes")
        };
        let strategy = valid_strategy_config_with_hash(&mismatched_strategy_config_hash(&expected));
        let parameters: ParametersBlock = strategy
            .parameters
            .clone()
            .try_into()
            .expect("valid parameters parse");
        let mut errors = Vec::new();
        validate_strategy_config_hash_binding(CONTEXT, &strategy, &parameters, &mut errors);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.strategy_config_hash")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_result_contract_strategy_hash_mismatch() {
        let errors = backtest_errors(BacktestParametersBlock {
            result_contract_replay: BacktestResultContractReplayBlock {
                strategy_config_hash: mismatched_strategy_config_hash(TEST_ARTIFACT_SHA256),
                ..valid_result_contract_replay()
            },
            ..valid_backtest()
        });
        assert!(
            errors.iter().any(|error| error
                .contains("parameters.backtest.result_contract_replay.strategy_config_hash")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_noop_backtest_counts() {
        let errors = backtest_errors(BacktestParametersBlock {
            maker_order_count: 0,
            passive_fill_count: 0,
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.maker_order_count")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.passive_fill_count")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_unsatisfied_backtest_count_floors() {
        let errors = backtest_errors(BacktestParametersBlock {
            passive_fill_count: 1,
            min_passive_fill_count: 2,
            resolved_market_count: 4,
            min_resolved_market_count: 5,
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.passive_fill_count")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.resolved_market_count")),
            "{errors:?}"
        );

        let zero_floor_errors = backtest_errors(BacktestParametersBlock {
            min_passive_fill_count: 0,
            min_resolved_market_count: 0,
            ..valid_backtest()
        });
        assert!(
            zero_floor_errors
                .iter()
                .any(|error| error.contains("parameters.backtest.passive_fill_count")),
            "{zero_floor_errors:?}"
        );
        assert!(
            zero_floor_errors
                .iter()
                .any(|error| error.contains("parameters.backtest.resolved_market_count")),
            "{zero_floor_errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_unreconciled_net_score() {
        let errors = backtest_errors(BacktestParametersBlock {
            net_score_micros: 401,
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.net_score_micros")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_non_positive_net_score() {
        let errors = backtest_errors(BacktestParametersBlock {
            captured_spread_score_micros: 300,
            fees_score_micros: 100,
            adverse_selection_score_micros: 100,
            settlement_loss_score_micros: 100,
            net_score_micros: 0,
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.net_score_micros")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_missing_backtest_artifact_digests() {
        let errors = backtest_errors(BacktestParametersBlock {
            run_artifact_sha256: "ABC".to_string(),
            threshold_artifact_sha256: String::new(),
            execution_model_artifact_sha256: "not-a-sha256".to_string(),
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.run_artifact_sha256")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.threshold_artifact_sha256")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.execution_model_artifact_sha256")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_missing_trade_and_book_corpus() {
        let errors = backtest_errors(BacktestParametersBlock {
            result_contract_replay: BacktestResultContractReplayBlock {
                catalog_data_types: Vec::new(),
                ..valid_result_contract_replay()
            },
            ..valid_backtest()
        });
        assert!(
            errors.iter().any(|error| error
                .contains("parameters.backtest.result_contract_replay.catalog_data_types")
                && error.contains("TradeTick")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error
                .contains("parameters.backtest.result_contract_replay.catalog_data_types")
                && error.contains("OrderBookDelta")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_disabled_queue_position() {
        let errors = backtest_errors(BacktestParametersBlock {
            result_contract_replay: BacktestResultContractReplayBlock {
                venue_queue_position: false,
                ..valid_result_contract_replay()
            },
            ..valid_backtest()
        });
        assert!(
            errors.iter().any(|error| error
                .contains("parameters.backtest.result_contract_replay.venue_queue_position")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_non_nt_result_contract_execution_model() {
        let errors = backtest_errors(BacktestParametersBlock {
            result_contract_replay: BacktestResultContractReplayBlock {
                execution_model: "custom_fill_sim".to_string(),
                ..valid_result_contract_replay()
            },
            ..valid_backtest()
        });
        assert!(
            errors.iter().any(|error| error
                .contains("parameters.backtest.result_contract_replay.execution_model")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_entry_gated_corpus() {
        let errors = backtest_errors(BacktestParametersBlock {
            entry_gated_corpus_used: true,
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.entry_gated_corpus_used")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_custom_fill_without_source_proof() {
        let errors = backtest_errors(BacktestParametersBlock {
            custom_fill_model_used: true,
            custom_fill_model_source_proven: false,
            ..valid_backtest()
        });
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.backtest.custom_fill_model_source_proven")),
            "{errors:?}"
        );
    }

    fn parameters_from_str(toml: &str) -> Result<ParametersBlock, toml::de::Error> {
        toml::from_str(toml)
    }

    #[test]
    fn parameters_block_deserializes_nested_runtime() {
        let toml = format!(
            "{}{}{}",
            r#"
            [runtime]
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            "#,
            VALID_MARKET_PORTFOLIO_TOML,
            valid_backtest_toml()
        );
        let parsed = parameters_from_str(&toml).expect("valid block deserializes");
        assert_eq!(parsed.runtime, valid_runtime());
        assert_eq!(parsed.market_portfolio, valid_market_portfolio());
        assert_eq!(parsed.backtest, valid_backtest());
    }

    #[test]
    fn parameters_block_requires_strict_gate_verdict() {
        let toml = format!(
            "{}{}{}",
            r#"
            [runtime]
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            "#,
            VALID_MARKET_PORTFOLIO_TOML,
            valid_backtest_toml().replace("            strict_gate_passed = false\n", "")
        );
        assert!(
            parameters_from_str(&toml).is_err(),
            "missing strict_gate_passed must fail loud"
        );
    }

    #[test]
    fn parameters_block_rejects_unknown_runtime_key() {
        let toml = format!(
            "{}{}{}",
            r#"
            [runtime]
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            surprise = 1
            "#,
            VALID_MARKET_PORTFOLIO_TOML,
            valid_backtest_toml()
        );
        assert!(
            parameters_from_str(&toml).is_err(),
            "an unknown [parameters.runtime] key must fail loud"
        );
    }

    #[test]
    fn parameters_block_rejects_unknown_backtest_key() {
        let toml = format!(
            "{}{}{}",
            r#"
            [runtime]
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            "#,
            VALID_MARKET_PORTFOLIO_TOML,
            r#"
            [backtest]
            verdict = "pass"
            build_head_sha = "0123456789abcdef0123456789abcdef01234567"
            strategy_config_hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            run_artifact = "artifact://maker/backtest/run"
            run_artifact_sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            threshold_artifact = "artifact://maker/backtest/thresholds"
            threshold_artifact_sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            execution_model_artifact = "artifact://maker/backtest/execution-model"
            execution_model_artifact_sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            maker_order_count = 3
            passive_fill_count = 2
            min_passive_fill_count = 2
            resolved_market_count = 5
            min_resolved_market_count = 5
            built_maker_replayed = true
            captured_spread_score_micros = 1000
            fees_score_micros = 100
            adverse_selection_score_micros = 200
            settlement_loss_score_micros = 300
            net_score_micros = 400
            thresholds_registered_before_run = true
            balanced_gate_evaluated = true
            strict_gate_evaluated = true
            balanced_gate_passed = true
            strict_gate_passed = false
            historical_full_depth_l2 = true
            full_population_corpus = true
            entry_gated_corpus_used = false
            custom_fill_model_used = false
            custom_fill_model_source_proven = false
            underlying_spot_causal_join = true
            statistical_significance = true
            shared_fair_value_pricing = true
            shared_settlement_primitive = true
            surprise = true

            [backtest.result_contract_replay]
            strategy_config_hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            execution_model = "nt_backtest_node"
            venue_queue_position = true
            catalog_data_types = ["OrderBookDelta", "TradeTick"]
            "#
        );
        assert!(
            parameters_from_str(&toml).is_err(),
            "an unknown [parameters.backtest] key must fail loud"
        );
    }

    #[test]
    fn parameters_block_rejects_missing_runtime_table() {
        assert!(
            parameters_from_str("decoy = 1").is_err(),
            "an absent [parameters.runtime] table must fail loud"
        );
    }

    #[test]
    fn parameters_block_rejects_missing_backtest_table() {
        assert!(
            parameters_from_str(&format!(
                "{}{}",
                r#"
                [runtime]
                trade_flow_window_secs = 600
                trade_flow_max_samples = 1000
                mu_min_classified_samples = 4
                mu_stale_window_ms = 60000
                mu_min_floor = 0.05
                requote_min_interval_ms = 500
                "#,
                VALID_MARKET_PORTFOLIO_TOML
            ))
            .is_err(),
            "an absent [parameters.backtest] table must fail loud"
        );
    }

    #[test]
    fn parameters_block_rejects_unknown_market_portfolio_key() {
        let toml = format!(
            "{}{}{}",
            r#"
            [runtime]
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            "#,
            r#"
            [market_portfolio]
            max_active_markets = 3
            total_bankroll_notional = 1500.0
            min_slot_notional = 100.0
            surprise = true
            "#,
            valid_backtest_toml()
        );
        assert!(
            parameters_from_str(&toml).is_err(),
            "an unknown [parameters.market_portfolio] key must fail loud"
        );
    }

    #[test]
    fn parameters_block_rejects_missing_market_portfolio_table() {
        let toml = format!(
            "{}{}",
            r#"
            [runtime]
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            "#,
            valid_backtest_toml()
        );
        assert!(
            parameters_from_str(&toml).is_err(),
            "an absent [parameters.market_portfolio] table must fail loud"
        );
    }

    #[test]
    fn parameters_block_rejects_missing_runtime_knob() {
        let toml = format!(
            "{}{}{}",
            r#"
            [runtime]
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            "#,
            VALID_MARKET_PORTFOLIO_TOML,
            valid_backtest_toml()
        );
        assert!(
            parameters_from_str(&toml).is_err(),
            "a missing μ knob must fail loud"
        );
    }

    #[test]
    fn parameters_block_rejects_missing_market_portfolio_knob() {
        let toml = format!(
            "{}{}{}",
            r#"
            [runtime]
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            "#,
            r#"
            [market_portfolio]
            max_active_markets = 3
            total_bankroll_notional = 1500.0
            "#,
            valid_backtest_toml()
        );
        assert!(
            parameters_from_str(&toml).is_err(),
            "a missing market-portfolio knob must fail loud"
        );
    }

    #[test]
    fn operator_knobs_thread_into_consumer_config() {
        // The load-bearing bridge test: the insert_* helpers must write exactly
        // the field names `BinaryOracleMakerConfig` deserializes. A key-name drift
        // fails the consumer config's `deny_unknown_fields` parse below; a value
        // drift fails an assertion.
        use crate::strategies::binary_oracle_maker::parse_config;
        let mut table = Map::new();
        table.insert(
            "strategy_id".to_string(),
            Value::String("binary_oracle_maker-001".to_string()),
        );
        table.insert("order_id_tag".to_string(), Value::String("001".to_string()));
        table.insert("oms_type".to_string(), Value::String("netting".to_string()));
        table.insert(
            "client_id".to_string(),
            Value::String("maker_execution_client".to_string()),
        );
        insert_runtime_knobs(&mut table, &valid_runtime()).expect("knobs thread");
        insert_market_portfolio_knobs(&mut table, &valid_market_portfolio())
            .expect("market portfolio policy threads");
        let config =
            parse_config(&Value::Table(table)).expect("flat table parses into the consumer config");
        assert_eq!(config.client_id, "maker_execution_client");
        assert_eq!(config.trade_flow_window_secs, 600);
        assert_eq!(config.trade_flow_max_samples, 1000);
        assert_eq!(config.mu_min_classified_samples, 4);
        assert_eq!(config.mu_stale_window_ms, 60_000);
        assert_eq!(config.mu_min_floor, 0.05);
        assert_eq!(config.requote_min_interval_ms, 500);
        assert_eq!(config.market_portfolio_max_active_markets, 3);
        assert_eq!(config.market_portfolio_total_bankroll_notional, 1500.0);
        assert_eq!(config.market_portfolio_min_slot_notional, 100.0);
    }

    #[test]
    fn insert_u64_field_rejects_value_above_i64_max() {
        let mut table = Map::new();
        assert!(
            insert_u64_field(&mut table, "trade_flow_window_secs", u64::MAX).is_err(),
            "a u64 above i64::MAX cannot round-trip through TOML and must fail closed"
        );
    }

    #[test]
    fn insert_usize_field_rejects_value_above_i64_max() {
        let mut table = Map::new();
        let too_large = usize::try_from(i64::MAX)
            .ok()
            .and_then(|value| value.checked_add(1));
        if let Some(value) = too_large {
            assert!(
                insert_usize_field(&mut table, "market_portfolio_max_active_markets", value)
                    .is_err(),
                "a usize above i64::MAX cannot round-trip through TOML and must fail closed"
            );
        }
    }
}
