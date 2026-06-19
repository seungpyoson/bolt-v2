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
use serde::{Deserialize, Serialize};
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
use crate::strategies::binary_oracle_maker::binding::MakerMarketDeclaration;
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
    /// Operator-declared markets the maker should quote. Each entry is an
    /// array-of-tables row (`[[parameters.markets]]`) mirroring the taker's
    /// `MarketSelectionTarget` config fields plus a stable `market_key`. The set
    /// is bounds-checked in [`validate_market_declarations`] (non-empty, within
    /// the concurrency cap, registered family, unique keys) so a misdeclared
    /// portfolio fails closed at load instead of silently idling.
    markets: Vec<MarketBindingParametersBlock>,
    backtest: BacktestParametersBlock,
}

/// One operator-declared market in `[[parameters.markets]]`. Mirrors the taker's
/// `MarketSelectionTarget`-building config fields exactly (`family_key`,
/// `underlying_asset`, `cadence_seconds`, `cadence_slug_token`, and the optional
/// static-market overrides), plus an operator-supplied `market_key` the portfolio
/// planner keys slots and rotation by. `deny_unknown_fields` fails loud on stray
/// keys. The per-market reference/resolution feed wiring is PR-B's runtime
/// concern; PR-A declares only the discovery-target identity.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct MarketBindingParametersBlock {
    market_key: String,
    family_key: String,
    underlying_asset: String,
    cadence_seconds: u64,
    cadence_slug_token: String,
    static_condition_id: Option<String>,
    static_yes_outcome: Option<String>,
    static_no_outcome: Option<String>,
}

impl MarketBindingParametersBlock {
    fn declaration(&self) -> MakerMarketDeclaration {
        MakerMarketDeclaration {
            market_key: self.market_key.clone(),
            family_key: self.family_key.clone(),
            underlying_asset: self.underlying_asset.clone(),
            cadence_seconds: self.cadence_seconds,
            cadence_slug_token: self.cadence_slug_token.clone(),
            static_condition_id: self.static_condition_id.clone(),
            static_yes_outcome: self.static_yes_outcome.clone(),
            static_no_outcome: self.static_no_outcome.clone(),
        }
    }
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

/// Deserialize the operator `[parameters]` block, first deriving any omitted
/// updown `cadence_slug_token` per `[[parameters.markets]]` row through the
/// shared, registry-dispatched family helper. The maker never recomputes the
/// slug — the derivation lives in exactly one place
/// ([`crate::bolt_v3_market_families::inject_derived_cadence_slug_token`], the
/// same seam the updown `[target]` block uses); this surface only plumbs its own
/// field names (`family_key` + `cadence_seconds`). One seam for all three
/// `[parameters]` deserialize sites so derivation is identical everywhere a maker
/// `[parameters]` block is read.
fn deserialize_parameters_block(parameters: &Value) -> Result<ParametersBlock, String> {
    parameters_with_derived_market_cadence_slug_tokens(parameters)?
        .try_into()
        .map_err(|error: toml::de::Error| error.to_string())
}

/// Return a copy of the `[parameters]` value with each `[[parameters.markets]]`
/// row's omitted, derivable `cadence_slug_token` filled in. A row whose family
/// does not derive a slug from cadence (e.g. a free-form static event market) is
/// left untouched and still required to supply its own slug. Returns `Err` only
/// when a contract-defining row omits the slug and its cadence has no contract
/// token (the shared helper names the offending cadence).
fn parameters_with_derived_market_cadence_slug_tokens(parameters: &Value) -> Result<Value, String> {
    let mut value = parameters.clone();
    let Some(table) = value.as_table_mut() else {
        return Ok(value);
    };
    let Some(markets) = table
        .get_mut(stringify!(markets))
        .and_then(Value::as_array_mut)
    else {
        return Ok(value);
    };
    for market in markets.iter_mut() {
        let Some(row) = market.as_table_mut() else {
            continue;
        };
        crate::bolt_v3_market_families::inject_derived_cadence_slug_token(
            row,
            stringify!(family_key),
            stringify!(cadence_seconds),
        )?;
    }
    Ok(value)
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
    let parameters = match deserialize_parameters_block(&strategy.parameters) {
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
    validate_market_declarations(
        context,
        &parameters.markets,
        market_portfolio.max_active_markets,
        &mut errors,
    );
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

/// Fail-closed bounds for the operator-declared `[[parameters.markets]]` set (the
/// PR-A binding go-live gate). Each rejected shape would otherwise produce a maker
/// that silently does nothing or selects an unbuildable market:
///
/// - an empty `markets` array means the maker has nothing to quote, which must
///   fail loud at load rather than start a live strategy that idles silently;
/// - declaring more markets than `market_portfolio.max_active_markets` is a
///   configuration contradiction (the planner could never admit them all), so it
///   is rejected rather than silently truncated;
/// - an `family_key` not registered in the shared market-family registry can
///   never resolve a market, so it is rejected at load — the same registered-family
///   policy the runtime selection engine fails loud on;
/// - an empty or duplicated `market_key` breaks the portfolio planner's per-market
///   slot/rotation keying (it requires non-empty unique keys), so it is rejected
///   at load.
fn validate_market_declarations(
    context: &str,
    markets: &[MarketBindingParametersBlock],
    max_active_markets: usize,
    errors: &mut Vec<String>,
) {
    if markets.is_empty() {
        errors.push(format!(
            "{context}: parameters.markets must declare at least one market (an empty market set would silently idle the maker instead of failing closed at load)"
        ));
        return;
    }
    if markets.len() > max_active_markets {
        errors.push(format!(
            "{context}: parameters.markets declares {} markets but parameters.market_portfolio.max_active_markets is {max_active_markets} (the portfolio can never admit more declared markets than the concurrency cap)",
            markets.len()
        ));
    }
    let mut seen_keys = std::collections::BTreeSet::new();
    for market in markets {
        if market.market_key.trim().is_empty() {
            errors.push(format!(
                "{context}: parameters.markets entry market_key must be a non-empty string (the portfolio planner requires a non-empty market_key to key slots and rotation)"
            ));
        } else if !seen_keys.insert(market.market_key.as_str()) {
            errors.push(format!(
                "{context}: parameters.markets market_key `{}` is declared more than once (each declared market must have a unique key)",
                market.market_key
            ));
        }
        validate_market_target(context, market, errors);
    }
}

/// Validate one declared market's discovery target through its OWNING family's
/// registry binding, at LOAD. This closes two fail-closed gaps a bare
/// registered-family membership check left open: (1) families that are registered
/// but cannot resolve a binary up/down market (`outcome_group`,
/// `hyperliquid_instrument`) are rejected via their
/// `unsupported_maker_market_target` error rather than admitted to fail only at
/// runtime; and (2) target-shape errors (zero/invalid cadence, malformed slug,
/// empty/malformed underlying, missing or identical static outcomes, static
/// fields on a rotating family) are caught at load instead of becoming silent
/// runtime resolution-misses. Reuses the shared family registry — no per-family
/// validation is reimplemented here. The operator-facing `cadence_seconds` is
/// `u64`; a value that cannot be represented as the engine's signed `i64` cadence
/// fails closed here rather than wrapping to a negative cadence.
fn validate_market_target(
    context: &str,
    market: &MarketBindingParametersBlock,
    errors: &mut Vec<String>,
) {
    let Ok(cadence_seconds) = i64::try_from(market.cadence_seconds) else {
        errors.push(format!(
            "{context}: parameters.markets entry `{}` cadence_seconds ({}) exceeds the supported signed-integer cadence range",
            market.market_key, market.cadence_seconds
        ));
        return;
    };
    let target = crate::bolt_v3_market_families::MarketSelectionTarget {
        family_key: market.family_key.as_str(),
        underlying_asset: market.underlying_asset.as_str(),
        cadence_seconds,
        cadence_slug_token: market.cadence_slug_token.as_str(),
        static_condition_id: market.static_condition_id.as_deref(),
        static_yes_outcome: market.static_yes_outcome.as_deref(),
        static_no_outcome: market.static_no_outcome.as_deref(),
    };
    let market_context = format!(
        "{context}: parameters.markets entry `{}`",
        market.market_key
    );
    errors.extend(
        crate::bolt_v3_market_families::validate_maker_market_target_from_target(
            &market_context,
            target,
        ),
    );
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

/// Extract the operator-declared markets from a loaded maker strategy as the
/// resolver's input type ([`MakerMarketDeclaration`]). This is the PR-A → PR-B
/// seam: PR-A validates and surfaces the declared market set from
/// `[[parameters.markets]]`; PR-B's runtime loop feeds these declarations into the
/// shared discovery engine via [`crate::strategies::binary_oracle_maker::binding`].
/// Fails with the same fail-closed message shape as `raw_maker_config` when the
/// `[parameters]` block is malformed (the bounds gate in [`validate_strategy`]
/// already rejects a malformed block at load, so this is the defensive runtime
/// re-parse).
pub fn declared_markets(strategy: &LoadedStrategy) -> Result<Vec<MakerMarketDeclaration>, String> {
    if strategy.config.strategy_archetype.as_str() != KEY {
        return Err(format!(
            "strategy_archetype `{}` is not `{KEY}`",
            strategy.config.strategy_archetype.as_str()
        ));
    }
    let parameters = deserialize_parameters_block(&strategy.config.parameters)
        .map_err(|error| format!("invalid [parameters] block: {error}"))?;
    Ok(parameters
        .markets
        .iter()
        .map(MarketBindingParametersBlock::declaration)
        .collect())
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
    let parameters = deserialize_parameters_block(&strategy.parameters)
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
    table.insert(
        super::config::MARKETS_CONFIG_DIGEST_FIELD.to_string(),
        Value::String(markets_config_digest(&parameters.markets)?),
    );
    Ok(Value::Table(table))
}

/// Compute a deterministic digest of the operator-declared market set so the
/// strategy-config hash (computed over the flat table this digest is inserted
/// into) covers `parameters.markets`. Closes the evidence-integrity gap where
/// changing a declared market's family/underlying/cadence/slug/static field or
/// the market count did NOT change `strategy_config_hash`, so the go-live gate
/// would accept a backtest run captured for a DIFFERENT market set. The
/// representation is CANONICAL: entries are sorted by `market_key` and every
/// field is serialized in a fixed order, so the digest depends only on the
/// declared set's content, not on operator TOML ordering.
fn markets_config_digest(markets: &[MarketBindingParametersBlock]) -> Result<String, String> {
    // Sort by `market_key` so the digest depends only on the declared set's
    // content, not on operator TOML ordering. Each entry is serialized through
    // `MarketBindingParametersBlock`'s own derived `Serialize`, so EVERY declared
    // field (family/underlying/cadence/slug/static_*) is covered with no
    // hand-maintained field list that could silently drift from the struct.
    let mut canonical: Vec<&MarketBindingParametersBlock> = markets.iter().collect();
    canonical.sort_by(|a, b| a.market_key.cmp(&b.market_key));
    json_artifact_sha256(&canonical).map_err(|error| error.to_string())
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
    use crate::bolt_v3_market_families::validation_bindings;

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

    fn market_declaration(market_key: &str) -> MarketBindingParametersBlock {
        MarketBindingParametersBlock {
            market_key: market_key.to_string(),
            family_key: "updown".to_string(),
            underlying_asset: "ETH".to_string(),
            cadence_seconds: 3_600,
            cadence_slug_token: "1h".to_string(),
            static_condition_id: None,
            static_yes_outcome: None,
            static_no_outcome: None,
        }
    }

    fn valid_markets() -> Vec<MarketBindingParametersBlock> {
        vec![market_declaration("eth-hourly")]
    }

    fn static_market_declaration(market_key: &str) -> MarketBindingParametersBlock {
        MarketBindingParametersBlock {
            market_key: market_key.to_string(),
            family_key: "static_binary_event".to_string(),
            underlying_asset: "ETH".to_string(),
            cadence_seconds: 3_600,
            cadence_slug_token: "eth-event-market".to_string(),
            static_condition_id: Some("condition-1".to_string()),
            static_yes_outcome: Some("Yes".to_string()),
            static_no_outcome: Some("No".to_string()),
        }
    }

    fn market_declaration_errors(markets: Vec<MarketBindingParametersBlock>) -> Vec<String> {
        validate_parameter_bounds(
            CONTEXT,
            &ParametersBlock {
                runtime: valid_runtime(),
                market_portfolio: valid_market_portfolio(),
                markets,
                backtest: valid_backtest(),
            },
        )
    }

    fn valid_result_contract_replay() -> BacktestResultContractReplayBlock {
        BacktestResultContractReplayBlock {
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
            execution_model = "nt_backtest_node"
            venue_queue_position = true
            catalog_data_types = ["OrderBookDelta", "TradeTick"]
            "#,
            current_test_build_head_sha(),
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

    const VALID_MARKETS_TOML: &str = r#"
            [[markets]]
            market_key = "eth-hourly"
            family_key = "updown"
            underlying_asset = "ETH"
            cadence_seconds = 3600
            cadence_slug_token = "1h"
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

            [[parameters.markets]]
            market_key = "eth-hourly"
            family_key = "updown"
            underlying_asset = "ETH"
            cadence_seconds = 3600
            cadence_slug_token = "1h"

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
                markets: valid_markets(),
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
                markets: valid_markets(),
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
                markets: valid_markets(),
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
    fn validate_parameter_bounds_accepts_valid_market_declarations() {
        assert!(
            market_declaration_errors(valid_markets()).is_empty(),
            "a single registered-family market within the cap must pass the go-live gate"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_empty_markets() {
        // Fail-closed: an empty declared set would silently idle the maker; the
        // go-live gate must reject it at load rather than start a no-op strategy.
        let errors = market_declaration_errors(Vec::new());
        assert!(
            errors
                .iter()
                .any(|error| error.contains("parameters.markets must declare at least one market")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_more_markets_than_concurrency_cap() {
        // The portfolio can never admit more declared markets than
        // market_portfolio.max_active_markets (here 3), so declaring 4 is a
        // configuration contradiction that must fail loud, not be silently truncated.
        let markets = vec![
            market_declaration("eth-1"),
            market_declaration("eth-2"),
            market_declaration("eth-3"),
            market_declaration("eth-4"),
        ];
        let errors = market_declaration_errors(markets);
        assert!(
            errors.iter().any(|error| error.contains(
                "declares 4 markets but parameters.market_portfolio.max_active_markets is 3"
            )),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_unregistered_family_key() {
        // A family_key absent from the shared market-family registry can never
        // resolve a market, so it is rejected at load — reusing the registry as the
        // single source of truth for the supported-family set.
        let markets = vec![MarketBindingParametersBlock {
            family_key: "not_a_registered_family".to_string(),
            ..market_declaration("eth-hourly")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("is not a registered market family")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_registered_but_binary_unsupported_family() {
        // FINDING A: a family can be REGISTERED yet unable to resolve a binary
        // up/down market (`outcome_group`, `hyperliquid_instrument` return
        // None/error unconditionally). A bare registry-membership check admitted
        // such a declaration at load and let it resolve to nothing at runtime. The
        // per-family maker-target validator must reject it at LOAD with the
        // binary-unsupported error. Pre-fix (membership-only check): these passed
        // the gate. Asserts the validation-error channel.
        for family_key in ["outcome_group", "hyperliquid_instrument"] {
            assert!(
                validation_bindings()
                    .iter()
                    .any(|binding| binding.key == family_key),
                "{family_key} must be a registered family for this test to be meaningful"
            );
            let markets = vec![MarketBindingParametersBlock {
                family_key: family_key.to_string(),
                ..market_declaration("eth-hourly")
            }];
            let errors = market_declaration_errors(markets);
            assert!(
                errors
                    .iter()
                    .any(|error| error.contains("does not support binary maker market selection")),
                "registered-but-binary-unsupported family `{family_key}` must fail the maker gate at load: {errors:?}"
            );
        }
    }

    #[test]
    fn validate_parameter_bounds_accepts_valid_binary_capable_family_declaration() {
        // The valid updown declaration (a binary-capable family with a well-formed
        // target) must still pass the per-family maker-target validator — no false
        // rejection of a correct binary maker market.
        assert!(
            market_declaration_errors(valid_markets()).is_empty(),
            "a valid binary-capable updown declaration must pass the maker target gate"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_updown_with_zero_cadence() {
        // FINDING B: target-shape fields were unvalidated at load — an updown
        // declaration with cadence_seconds=0 passed the gate and became a runtime
        // resolution-miss. The per-family validator (reusing updown's
        // validate_target_cadence) must reject it at LOAD. Asserts the cadence
        // error channel.
        let markets = vec![MarketBindingParametersBlock {
            cadence_seconds: 0,
            ..market_declaration("eth-hourly")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("cadence_secs must be a positive integer")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_updown_with_malformed_slug() {
        // FINDING B: a malformed cadence_slug_token (uppercase) passed the gate.
        let markets = vec![MarketBindingParametersBlock {
            cadence_slug_token: "BadSlug".to_string(),
            ..market_declaration("eth-hourly")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("cadence_slug_token must use only lowercase")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_updown_noncanonical_cadence_slug_pair() {
        // GLM F3 (PR #822): the #841 cadence->slug contract is wired into the maker
        // LOAD gate (updown::validate_maker_market_target -> validate_cadence_slug_contract).
        // No existing maker-gate test exercised it: a VALID-charset but NON-canonical
        // slug on a VALID cadence clears every other check (underlying/cadence/charset/
        // static), so deleting the contract call would go uncaught. Differential guard:
        // cadence_seconds=3600 with slug "2h" (lowercase + digit, non-empty, != the
        // canonical "1h") is rejected ONLY by the contract rule. A non-canonical slug
        // silently fails to resolve at runtime (selection derives the market from
        // expected_cadence_slug_token), so it must fail CLOSED at load. Removing the
        // contract call empties this error channel and fails the test.
        let markets = vec![MarketBindingParametersBlock {
            cadence_seconds: 3_600,
            cadence_slug_token: "2h".to_string(),
            ..market_declaration("eth-hourly")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("when target.cadence_secs is 3600")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_market_with_empty_underlying_asset() {
        // PR #822 review (gemini-code-assist): underlying_asset should be validated
        // non-empty at LOAD. It already is — the per-family maker-target validator
        // reuses the shared `validate_underlying_asset` rule rather than
        // re-implementing field checks inline, so an empty underlying fails CLOSED at
        // the gate. Pin that channel so the shared-engine wiring can't silently drop.
        let markets = vec![MarketBindingParametersBlock {
            underlying_asset: String::new(),
            ..market_declaration("eth-hourly")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("underlying_asset must not be empty")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_market_with_empty_cadence_slug_token() {
        // PR #822 review (gemini-code-assist): cadence_slug_token should be non-empty
        // at LOAD. The per-family validator reuses updown's validate_cadence_slug_token
        // (is-empty branch), so an empty slug fails CLOSED at the gate — distinct from
        // the malformed (uppercase) case already covered above.
        let markets = vec![MarketBindingParametersBlock {
            cadence_slug_token: String::new(),
            ..market_declaration("eth-hourly")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("cadence_slug_token must not be empty")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_market_with_cadence_seconds_exceeding_i64() {
        // PR #822 review (gemini-code-assist): cadence_seconds must fit a signed
        // 64-bit integer. The operator-facing u64 is range-checked at the archetype
        // gate before the discovery target is built, so a value past i64::MAX fails
        // CLOSED rather than wrapping to a negative cadence at runtime.
        let markets = vec![MarketBindingParametersBlock {
            cadence_seconds: u64::MAX,
            ..market_declaration("eth-hourly")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("exceeds the supported signed-integer cadence range")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_updown_with_static_fields() {
        // FINDING B: static-market override fields on a rotating-cadence family are
        // a misconfiguration; the validator must reject them at load.
        let markets = vec![MarketBindingParametersBlock {
            static_yes_outcome: Some("Up".to_string()),
            ..market_declaration("eth-hourly")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("not valid for the rotating-cadence `updown` family")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_accepts_valid_static_binary_event_declaration() {
        // static_binary_event is binary-capable: a well-formed static declaration
        // (both outcomes present + distinct, valid slug/underlying) must pass.
        let errors = market_declaration_errors(vec![static_market_declaration("eth-event")]);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn validate_parameter_bounds_rejects_static_binary_event_missing_outcomes() {
        // FINDING B: static_binary_event::select_binary_option_market returns None
        // when the static yes/no outcome labels are absent, so a declaration
        // lacking them was a runtime resolution-miss. The per-family validator must
        // reject it at LOAD.
        let markets = vec![MarketBindingParametersBlock {
            static_yes_outcome: None,
            static_no_outcome: None,
            ..static_market_declaration("eth-event")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error
                    .contains("requires both static_yes_outcome and static_no_outcome")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_static_binary_event_identical_outcomes() {
        // FINDING B: identical yes/no outcome labels cannot form a binary market.
        let markets = vec![MarketBindingParametersBlock {
            static_yes_outcome: Some("Same".to_string()),
            static_no_outcome: Some("Same".to_string()),
            ..static_market_declaration("eth-event")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("must be distinct")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_duplicate_market_key() {
        let markets = vec![market_declaration("eth-dup"), market_declaration("eth-dup")];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("market_key `eth-dup` is declared more than once")),
            "{errors:?}"
        );
    }

    #[test]
    fn validate_parameter_bounds_rejects_empty_market_key() {
        let markets = vec![MarketBindingParametersBlock {
            market_key: "   ".to_string(),
            ..market_declaration("placeholder")
        }];
        let errors = market_declaration_errors(markets);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("market_key must be a non-empty string")),
            "{errors:?}"
        );
    }

    fn loaded_strategy_from(config: BoltV3StrategyConfig) -> LoadedStrategy {
        LoadedStrategy {
            config_path: std::path::PathBuf::from("tests/maker.toml"),
            relative_path: "maker.toml".to_string(),
            config,
        }
    }

    #[test]
    fn declared_markets_projects_operator_markets_into_resolver_inputs() {
        // The PR-A → PR-B seam: the operator `[[parameters.markets]]` block must
        // surface as the resolver's `MakerMarketDeclaration` input type with every
        // field carried through (a field drop here would silently lose a declared
        // market's discovery identity). Asserts the projected declaration channel.
        let strategy = loaded_strategy_from(valid_strategy_config());
        let declarations =
            declared_markets(&strategy).expect("valid maker strategy yields declarations");
        assert_eq!(
            declarations,
            vec![MakerMarketDeclaration {
                market_key: "eth-hourly".to_string(),
                family_key: "updown".to_string(),
                underlying_asset: "ETH".to_string(),
                cadence_seconds: 3_600,
                cadence_slug_token: "1h".to_string(),
                static_condition_id: None,
                static_yes_outcome: None,
                static_no_outcome: None,
            }]
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

    fn parameters_with_markets(markets: Vec<MarketBindingParametersBlock>) -> ParametersBlock {
        ParametersBlock {
            runtime: valid_runtime(),
            market_portfolio: valid_market_portfolio(),
            markets,
            backtest: valid_backtest(),
        }
    }

    #[test]
    fn strategy_config_hash_covers_declared_market_set() {
        // FINDING C: parameters.markets was NOT part of the hashed raw config, so
        // changing a declared market did NOT change strategy_config_hash and the
        // go-live gate would accept a backtest captured for a DIFFERENT market set.
        // The canonical markets digest is now in the hashed table, so changing ANY
        // declared-market field — or the market count — must change the hash.
        // Pre-fix: every variant below produced the SAME hash. Asserts the
        // hash-changed channel directly.
        let strategy = valid_strategy_config();
        let baseline =
            maker_strategy_config_hash(&strategy, &parameters_with_markets(valid_markets()))
                .expect("baseline hash computes");

        let mutated_sets = [
            vec![MarketBindingParametersBlock {
                family_key: "static_binary_event".to_string(),
                ..market_declaration("eth-hourly")
            }],
            vec![MarketBindingParametersBlock {
                underlying_asset: "BTC".to_string(),
                ..market_declaration("eth-hourly")
            }],
            vec![MarketBindingParametersBlock {
                cadence_seconds: 7_200,
                ..market_declaration("eth-hourly")
            }],
            vec![MarketBindingParametersBlock {
                cadence_slug_token: "daily".to_string(),
                ..market_declaration("eth-hourly")
            }],
            vec![MarketBindingParametersBlock {
                static_yes_outcome: Some("Up".to_string()),
                ..market_declaration("eth-hourly")
            }],
            vec![
                market_declaration("eth-hourly"),
                market_declaration("eth-2"),
            ],
        ];
        for mutated in mutated_sets {
            let mutated_hash =
                maker_strategy_config_hash(&strategy, &parameters_with_markets(mutated.clone()))
                    .expect("mutated hash computes");
            assert_ne!(
                baseline, mutated_hash,
                "changing a declared market must invalidate the strategy_config_hash: {mutated:?}"
            );
        }
    }

    #[test]
    fn strategy_config_hash_is_independent_of_declared_market_order() {
        // The digest is CANONICAL (sorted by market_key), so the same declared set
        // in a different order hashes identically — only content changes the hash,
        // not operator TOML ordering.
        let strategy = valid_strategy_config();
        let a = vec![market_declaration("eth-a"), market_declaration("eth-b")];
        let b = vec![market_declaration("eth-b"), market_declaration("eth-a")];
        assert_eq!(
            maker_strategy_config_hash(&strategy, &parameters_with_markets(a))
                .expect("hash a computes"),
            maker_strategy_config_hash(&strategy, &parameters_with_markets(b))
                .expect("hash b computes"),
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
            "{}{}{}{}",
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
            VALID_MARKETS_TOML,
            valid_backtest_toml()
        );
        let parsed = parameters_from_str(&toml).expect("valid block deserializes");
        assert_eq!(parsed.runtime, valid_runtime());
        assert_eq!(parsed.market_portfolio, valid_market_portfolio());
        assert_eq!(parsed.markets, valid_markets());
        assert_eq!(parsed.backtest, valid_backtest());
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
        let expected_digest = markets_config_digest(&valid_markets()).expect("digest computes");
        table.insert(
            super::super::config::MARKETS_CONFIG_DIGEST_FIELD.to_string(),
            Value::String(expected_digest.clone()),
        );
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
        assert_eq!(config.markets_config_digest, expected_digest);
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

    fn first_market_row_mut(parameters: &mut Value) -> &mut Map<String, Value> {
        parameters
            .as_table_mut()
            .expect("parameters is a table")
            .get_mut(stringify!(markets))
            .and_then(Value::as_array_mut)
            .expect("markets is an array")
            .first_mut()
            .expect("at least one market row")
            .as_table_mut()
            .expect("market row is a table")
    }

    fn remove_first_market_slug_token(parameters: &mut Value) {
        first_market_row_mut(parameters).remove(stringify!(cadence_slug_token));
    }

    fn set_first_market_cadence_seconds(parameters: &mut Value, cadence_seconds: i64) {
        first_market_row_mut(parameters).insert(
            stringify!(cadence_seconds).to_string(),
            Value::Integer(cadence_seconds),
        );
    }

    fn first_market_slug_token(parameters: &Value) -> Option<&str> {
        parameters
            .as_table()?
            .get(stringify!(markets))?
            .as_array()?
            .first()?
            .as_table()?
            .get(stringify!(cadence_slug_token))?
            .as_str()
    }

    #[test]
    fn deserialize_parameters_block_derives_omitted_updown_market_slug_token() {
        // Drop the operator-supplied slug from the sole updown market row; the shared
        // seam must derive it (cadence_seconds = 3600 ⇒ "1h") so the full
        // [parameters] block still deserializes through the maker's own deny-unknown
        // path.
        let mut config = valid_strategy_config_with_hash(TEST_ARTIFACT_SHA256);
        remove_first_market_slug_token(&mut config.parameters);
        let parameters = deserialize_parameters_block(&config.parameters)
            .expect("an omitted updown slug derives and the block deserializes");
        assert_eq!(parameters.markets[0].cadence_slug_token, "1h");
    }

    #[test]
    fn deserialize_parameters_block_rejects_omitted_token_for_non_contract_cadence() {
        let mut config = valid_strategy_config_with_hash(TEST_ARTIFACT_SHA256);
        set_first_market_cadence_seconds(&mut config.parameters, 120);
        remove_first_market_slug_token(&mut config.parameters);
        let error = deserialize_parameters_block(&config.parameters)
            .expect_err("a non-contract cadence with no token must fail closed");
        assert!(
            error.contains("cadence_seconds=120")
                && error.contains("cadence_slug_token is required"),
            "error must name the offending cadence and the maker's field name: {error}"
        );
    }

    #[test]
    fn parameters_derivation_leaves_non_updown_market_untouched() {
        // A free-form-slug family declares no cadence→slug contract, so an omitted
        // token is NOT derived and NOT errored here: the maker surface dispatches
        // through the shared helper, which leaves the missing token for that family's
        // own required-field validator. (A non-updown row is used precisely because
        // its registry binding sets `derive_cadence_slug_token: None`.)
        let parameters: Value = toml::toml! {
            [[markets]]
            market_key = "eth-event"
            family_key = "static_binary_event"
            underlying_asset = "ETH"
            cadence_seconds = 3600
        }
        .into();
        let derived = parameters_with_derived_market_cadence_slug_tokens(&parameters)
            .expect("a free-form family row is left untouched, not errored");
        assert_eq!(
            first_market_slug_token(&derived),
            None,
            "a free-form-slug family must not receive a derived token"
        );
    }
}
