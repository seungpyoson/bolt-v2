//! TOML config struct + parse/validate for the `binary_oracle_maker` strategy.
//!
//! This is the **flat NautilusTrader config** the strategy consumes at build:
//! the `StrategyCore` envelope (`strategy_id`, `order_id_tag`, `oms_type`) plus
//! the μ-estimator / health-gate runtime knobs and market-portfolio policy the
//! archetype threads in from the operator `[strategies.<id>.parameters]` block
//! (Slices 2 and 9, #488). It is built by `archetype::raw_maker_config`, never
//! written by an operator directly.
//! `deny_unknown_fields` fails loud on any stray key.
//!
//! This struct validates the flat table's structure (field presence, TOML type,
//! unknown keys, `oms_type` parseability) and delegates the live archetype's
//! runtime bounds for the knobs threaded into this shape to the shared maker
//! validation helpers. That keeps registry manifest validation fail-closed before
//! backtest/build can turn malformed maker config into a green no-op run.

use anyhow::{Context, Result};
use nautilus_model::enums::OmsType;
use serde::Deserialize;
use toml::Value;

use super::binding::MakerMarketDeclaration;
use crate::{
    bolt_v3_maker_market_selection::{
        MakerMarketPortfolioBlocker, MakerMarketPortfolioDeclarationBlocker,
        MakerMarketPortfolioDeclarationInputs, MakerMarketPortfolioPolicy,
        MakerMarketPortfolioPolicyInputs, MakerRuntimeParameterBlocker,
        MakerRuntimeParameterBoundInputs, maker_market_portfolio_declaration_blockers,
        maker_market_portfolio_policy_input_blockers, maker_runtime_parameter_input_blockers,
    },
    bolt_v3_target_identity::stable_identity_field_is_canonical,
    strategies::registry::ValidationError,
};

trait TomlValueExt {
    fn as_float_or_integer(&self) -> Option<f64>;
}

impl TomlValueExt for Value {
    fn as_float_or_integer(&self) -> Option<f64> {
        self.as_float()
            .or_else(|| self.as_integer().map(|value| value as f64))
    }
}

/// Flat NautilusTrader config the maker consumes at build. The `StrategyConfig`
/// envelope fields `BinaryOracleMaker::new` feeds into `StrategyCore::new` plus
/// the μ runtime knobs `MakerMuState::new` projects into its estimator,
/// health-gate, and trade-flow config views, and the generic Slice 9
/// market-portfolio policy later runtime selection consumes. Every other
/// `StrategyConfig` field is left at NT's documented default (see
/// `StrategyConfig::default`).
/// `deny_unknown_fields` fails loud on any stray key so a typo in the operator
/// TOML cannot be silently ignored. `Eq` is intentionally not derived because
/// `mu_min_floor` is an `f64`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleMakerConfig {
    /// The NautilusTrader strategy id (becomes the running strategy's id).
    pub strategy_id: String,
    /// The unique order-id tag for the strategy.
    pub order_id_tag: String,
    /// The order-management-system type, parsed as a NautilusTrader `OmsType`.
    pub oms_type: String,
    /// The configured execution client id used for submit/cancel routing context.
    pub client_id: String,
    /// Signed-trade-flow retention window in seconds (μ estimator input).
    pub trade_flow_window_secs: u64,
    /// Signed-trade-flow retention sample cap (μ estimator input).
    pub trade_flow_max_samples: u64,
    /// Minimum classified (`Buyer`/`Seller`) samples before a μ is produced.
    pub mu_min_classified_samples: u64,
    /// Maximum age (ms) of the most recent trade before μ is considered stale.
    pub mu_stale_window_ms: u64,
    /// Lower bound μ must reach to be healthy (the degenerate-flow floor).
    pub mu_min_floor: f64,
    /// Minimum interval (ms) between requote actions on a leg. Sourced into the
    /// requote budget's same-tick throttle via `build_requote_budget_pair`; the
    /// submit-rate and venue REST caps are NOT config knobs — they come from
    /// `risk.nautilus.max_order_submit_rate` and the venue egress model.
    pub requote_min_interval_ms: u64,
    /// Interval (ms) of the maker's autonomous quote/refresh timer — how often the
    /// runtime re-resolves its active markets and (in later slices) requotes. The
    /// loop cadence; distinct from `requote_min_interval_ms`, which is a per-leg
    /// throttle floor inside a cycle, not the scheduling period.
    pub quote_interval_ms: u64,
    /// Maximum number of markets the portfolio selector may quote concurrently.
    pub market_portfolio_max_active_markets: u64,
    /// Total bankroll notional allocated to the market-selection portfolio.
    pub market_portfolio_total_bankroll_notional: f64,
    /// Minimum per-market slot notional after the bankroll split.
    pub market_portfolio_min_slot_notional: f64,
    /// Deterministic digest of the operator-declared `[[parameters.markets]]` set
    /// (canonical, sorted by `market_key`, all fields). It is threaded into this
    /// flat config by `archetype::raw_maker_config` PRECISELY so the declared
    /// market set is covered by the strategy-config hash the go-live gate binds:
    /// changing any declared market's family/underlying/cadence/slug/static field
    /// or the market count changes this digest, which changes the
    /// `strategy_config_hash`, which invalidates a backtest run captured for a
    /// DIFFERENT market set. Without it the gate would accept stale evidence for
    /// an untested market set.
    pub markets_config_digest: String,
    /// The operator-declared markets the maker quotes, threaded verbatim from the
    /// `[[parameters.markets]]` block by `archetype::raw_maker_config` so the
    /// runtime (`runtime::MakerRuntime`) can resolve them at `on_start` against the
    /// live instrument snapshot. `markets_config_digest` above binds the same set
    /// into the go-live hash; this carries the data the digest summarizes.
    pub markets: Vec<MakerMarketDeclaration>,
}

/// Zero-sized factory the `StrategyBuilder` trait is implemented for (in
/// `mod.rs`). Mirrors `BinaryOracleEdgeTakerBuilder`.
#[derive(Debug)]
pub struct BinaryOracleMakerBuilder;

const WRONG_TYPE_CODE: &str = "wrong_type";
const MISSING_STRATEGY_ID_CODE: &str = "missing_strategy_id";
const MISSING_ORDER_ID_TAG_CODE: &str = "missing_order_id_tag";
const MISSING_OMS_TYPE_CODE: &str = "missing_oms_type";
const MISSING_CLIENT_ID_CODE: &str = "missing_client_id";
const INVALID_OMS_TYPE_CODE: &str = "invalid_oms_type";
const INVALID_ORDER_ID_TAG_CODE: &str = "invalid_order_id_tag";
const INVALID_STRATEGY_ID_CODE: &str = "invalid_strategy_id";
const UNKNOWN_FIELD_CODE: &str = "unknown_field";
const POSITIVE_REQUIRED_CODE: &str = stringify!(positive_required);
const VALUE_OUT_OF_RANGE_CODE: &str = stringify!(value_out_of_range);
const CONVERSION_OVERFLOW_CODE: &str = stringify!(conversion_overflow);
const UNSATISFIABLE_WARMUP_CODE: &str = stringify!(unsatisfiable_warmup);
const BANKROLL_BELOW_MIN_SLOT_CODE: &str = stringify!(bankroll_below_min_slot);
const MISSING_MARKETS_CODE: &str = concat!(stringify!(missing_), stringify!(markets));
const EMPTY_MARKETS_CODE: &str = stringify!(empty_markets);
const MARKETS_ABOVE_ACTIVE_CAP_CODE: &str = stringify!(markets_above_active_cap);
const MISSING_TRADE_FLOW_WINDOW_SECS_CODE: &str =
    concat!(stringify!(missing_), stringify!(trade_flow_window_secs));
const MISSING_TRADE_FLOW_MAX_SAMPLES_CODE: &str =
    concat!(stringify!(missing_), stringify!(trade_flow_max_samples));
const MISSING_MU_MIN_CLASSIFIED_SAMPLES_CODE: &str =
    concat!(stringify!(missing_), stringify!(mu_min_classified_samples));
const MISSING_MU_STALE_WINDOW_MS_CODE: &str =
    concat!(stringify!(missing_), stringify!(mu_stale_window_ms));
const MISSING_MU_MIN_FLOOR_CODE: &str = concat!(stringify!(missing_), stringify!(mu_min_floor));
const MISSING_REQUOTE_MIN_INTERVAL_MS_CODE: &str =
    concat!(stringify!(missing_), stringify!(requote_min_interval_ms));
const MISSING_QUOTE_INTERVAL_MS_CODE: &str =
    concat!(stringify!(missing_), stringify!(quote_interval_ms));
const MISSING_MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_CODE: &str = concat!(
    stringify!(missing_),
    stringify!(market_portfolio_max_active_markets)
);
const MISSING_MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_CODE: &str = concat!(
    stringify!(missing_),
    stringify!(market_portfolio_total_bankroll_notional)
);
const MISSING_MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_CODE: &str = concat!(
    stringify!(missing_),
    stringify!(market_portfolio_min_slot_notional)
);

const STRATEGY_ID_FIELD: &str = "strategy_id";
const ORDER_ID_TAG_FIELD: &str = "order_id_tag";
const OMS_TYPE_FIELD: &str = "oms_type";
const CLIENT_ID_FIELD: &str = "client_id";
const TRADE_FLOW_WINDOW_SECS_FIELD: &str = "trade_flow_window_secs";
const TRADE_FLOW_MAX_SAMPLES_FIELD: &str = "trade_flow_max_samples";
const MU_MIN_CLASSIFIED_SAMPLES_FIELD: &str = "mu_min_classified_samples";
const MU_STALE_WINDOW_MS_FIELD: &str = "mu_stale_window_ms";
const MU_MIN_FLOOR_FIELD: &str = "mu_min_floor";
const REQUOTE_MIN_INTERVAL_MS_FIELD: &str = "requote_min_interval_ms";
const QUOTE_INTERVAL_MS_FIELD: &str = "quote_interval_ms";
const MARKETS_FIELD: &str = "markets";
const MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD: &str = "market_portfolio_max_active_markets";
const MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD: &str =
    "market_portfolio_total_bankroll_notional";
const MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD: &str = "market_portfolio_min_slot_notional";
pub(crate) const MARKETS_CONFIG_DIGEST_FIELD: &str = "markets_config_digest";
const MISSING_MARKETS_CONFIG_DIGEST_CODE: &str = "missing_markets_config_digest";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinaryOracleMakerFieldType {
    String,
    Integer,
    Float,
    Array,
}

impl BinaryOracleMakerFieldType {
    fn expected(self) -> &'static str {
        match self {
            Self::String => stringify!(string),
            Self::Integer => stringify!(integer),
            Self::Float => stringify!(float),
            Self::Array => stringify!(array),
        }
    }

    fn article(self) -> &'static str {
        match self {
            Self::String | Self::Float => stringify!(a),
            Self::Integer | Self::Array => stringify!(an),
        }
    }
}

/// Deserialize the maker config from its TOML table. Fails loud if the table is
/// missing required envelope fields or carries unknown keys (via
/// `deny_unknown_fields`).
pub fn parse_config(raw: &Value) -> Result<BinaryOracleMakerConfig> {
    let config: BinaryOracleMakerConfig = raw
        .clone()
        .try_into()
        .context("binary_oracle_maker builder requires a valid config table")?;
    anyhow::ensure!(
        stable_identity_field_is_canonical(config.strategy_id.as_str()),
        "binary_oracle_maker strategy_id must be a non-empty, unpadded string"
    );
    anyhow::ensure!(
        stable_identity_field_is_canonical(config.order_id_tag.as_str()),
        "binary_oracle_maker order_id_tag must be a non-empty, unpadded string"
    );
    Ok(config)
}

/// Push validation errors for the flat maker config into `errors`: unknown keys,
/// required field presence, TOML types, `oms_type` parseability, and the same
/// runtime bounds the live archetype applies before constructing a maker.
pub fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
    let Some(table) = raw.as_table() else {
        errors.push(ValidationError {
            field: field_prefix.to_string(),
            code: WRONG_TYPE_CODE,
            message: format!("must be a table, got {} value", raw.type_str()),
        });
        return;
    };

    for key in table.keys() {
        if !matches!(
            key.as_str(),
            STRATEGY_ID_FIELD
                | ORDER_ID_TAG_FIELD
                | OMS_TYPE_FIELD
                | CLIENT_ID_FIELD
                | TRADE_FLOW_WINDOW_SECS_FIELD
                | TRADE_FLOW_MAX_SAMPLES_FIELD
                | MU_MIN_CLASSIFIED_SAMPLES_FIELD
                | MU_STALE_WINDOW_MS_FIELD
                | MU_MIN_FLOOR_FIELD
                | REQUOTE_MIN_INTERVAL_MS_FIELD
                | QUOTE_INTERVAL_MS_FIELD
                | MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD
                | MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD
                | MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD
                | MARKETS_CONFIG_DIGEST_FIELD
                | MARKETS_FIELD
        ) {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{key}"),
                code: UNKNOWN_FIELD_CODE,
                message: format!("unknown field `{key}`"),
            });
        }
    }

    validate_string_field(
        table,
        field_prefix,
        STRATEGY_ID_FIELD,
        MISSING_STRATEGY_ID_CODE,
        errors,
    );
    validate_canonical_identity_field(
        table,
        field_prefix,
        STRATEGY_ID_FIELD,
        INVALID_STRATEGY_ID_CODE,
        errors,
    );
    validate_string_field(
        table,
        field_prefix,
        ORDER_ID_TAG_FIELD,
        MISSING_ORDER_ID_TAG_CODE,
        errors,
    );
    validate_canonical_identity_field(
        table,
        field_prefix,
        ORDER_ID_TAG_FIELD,
        INVALID_ORDER_ID_TAG_CODE,
        errors,
    );
    validate_string_field(
        table,
        field_prefix,
        OMS_TYPE_FIELD,
        MISSING_OMS_TYPE_CODE,
        errors,
    );
    validate_string_field(
        table,
        field_prefix,
        CLIENT_ID_FIELD,
        MISSING_CLIENT_ID_CODE,
        errors,
    );
    validate_string_field(
        table,
        field_prefix,
        MARKETS_CONFIG_DIGEST_FIELD,
        MISSING_MARKETS_CONFIG_DIGEST_CODE,
        errors,
    );
    let trade_flow_window_secs = validate_non_negative_u64_field(
        table,
        field_prefix,
        TRADE_FLOW_WINDOW_SECS_FIELD,
        MISSING_TRADE_FLOW_WINDOW_SECS_CODE,
        errors,
    );
    let trade_flow_max_samples = validate_non_negative_u64_field(
        table,
        field_prefix,
        TRADE_FLOW_MAX_SAMPLES_FIELD,
        MISSING_TRADE_FLOW_MAX_SAMPLES_CODE,
        errors,
    );
    let mu_min_classified_samples = validate_non_negative_u64_field(
        table,
        field_prefix,
        MU_MIN_CLASSIFIED_SAMPLES_FIELD,
        MISSING_MU_MIN_CLASSIFIED_SAMPLES_CODE,
        errors,
    );
    let mu_stale_window_ms = validate_non_negative_u64_field(
        table,
        field_prefix,
        MU_STALE_WINDOW_MS_FIELD,
        MISSING_MU_STALE_WINDOW_MS_CODE,
        errors,
    );
    let mu_min_floor = validate_float_field(
        table,
        field_prefix,
        MU_MIN_FLOOR_FIELD,
        MISSING_MU_MIN_FLOOR_CODE,
        errors,
    );
    let requote_min_interval_ms = validate_non_negative_u64_field(
        table,
        field_prefix,
        REQUOTE_MIN_INTERVAL_MS_FIELD,
        MISSING_REQUOTE_MIN_INTERVAL_MS_CODE,
        errors,
    );
    let quote_interval_ms = validate_non_negative_u64_field(
        table,
        field_prefix,
        QUOTE_INTERVAL_MS_FIELD,
        MISSING_QUOTE_INTERVAL_MS_CODE,
        errors,
    );
    let market_portfolio_max_active_markets = validate_non_negative_u64_field(
        table,
        field_prefix,
        MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD,
        MISSING_MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_CODE,
        errors,
    );
    let market_portfolio_total_bankroll_notional = validate_float_field(
        table,
        field_prefix,
        MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD,
        MISSING_MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_CODE,
        errors,
    );
    let market_portfolio_min_slot_notional = validate_float_field(
        table,
        field_prefix,
        MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD,
        MISSING_MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_CODE,
        errors,
    );
    let markets = validate_markets_array_field(table, field_prefix, errors);
    validate_runtime_bounds(
        field_prefix,
        RuntimeBoundInputs {
            trade_flow_window_secs,
            trade_flow_max_samples,
            mu_min_classified_samples,
            mu_stale_window_ms,
            mu_min_floor,
            requote_min_interval_ms,
            quote_interval_ms,
            market_portfolio_max_active_markets,
            market_portfolio_total_bankroll_notional,
            market_portfolio_min_slot_notional,
            declared_market_count: markets.map(|markets| markets.len()),
        },
        errors,
    );
    validate_order_id_tag_delimiter_free(table, field_prefix, errors);
    validate_oms_type_parses(table, field_prefix, errors);
}

#[derive(Debug, Clone, Copy)]
struct RuntimeBoundInputs {
    trade_flow_window_secs: Option<u64>,
    trade_flow_max_samples: Option<u64>,
    mu_min_classified_samples: Option<u64>,
    mu_stale_window_ms: Option<u64>,
    mu_min_floor: Option<f64>,
    requote_min_interval_ms: Option<u64>,
    quote_interval_ms: Option<u64>,
    market_portfolio_max_active_markets: Option<u64>,
    market_portfolio_total_bankroll_notional: Option<f64>,
    market_portfolio_min_slot_notional: Option<f64>,
    declared_market_count: Option<usize>,
}

fn validate_runtime_bounds(
    field_prefix: &str,
    inputs: RuntimeBoundInputs,
    errors: &mut Vec<ValidationError>,
) {
    let runtime_bound_inputs = MakerRuntimeParameterBoundInputs {
        trade_flow_window_secs: inputs.trade_flow_window_secs,
        trade_flow_max_samples: inputs.trade_flow_max_samples,
        mu_min_classified_samples: inputs.mu_min_classified_samples,
        mu_stale_window_ms: inputs.mu_stale_window_ms,
        mu_min_floor: inputs.mu_min_floor,
        requote_min_interval_ms: inputs.requote_min_interval_ms,
        quote_interval_ms: inputs.quote_interval_ms,
    };
    for blocker in maker_runtime_parameter_input_blockers(runtime_bound_inputs) {
        push_runtime_parameter_blocker(field_prefix, blocker, errors);
    }
    validate_market_portfolio_bounds(field_prefix, inputs, errors);
}

fn push_runtime_parameter_blocker(
    field_prefix: &str,
    blocker: MakerRuntimeParameterBlocker,
    errors: &mut Vec<ValidationError>,
) {
    let (field, code, message) = match blocker {
        MakerRuntimeParameterBlocker::ZeroTradeFlowWindowSecs => (
            TRADE_FLOW_WINDOW_SECS_FIELD,
            POSITIVE_REQUIRED_CODE,
            "must be > 0 (a zero retention window holds no trades, so a μ can never be produced)"
                .to_string(),
        ),
        MakerRuntimeParameterBlocker::TradeFlowWindowMillisOverflow { window_secs } => (
            TRADE_FLOW_WINDOW_SECS_FIELD,
            CONVERSION_OVERFLOW_CODE,
            format!(
                "({window_secs}) must be small enough that its second-to-millisecond conversion does not overflow u64"
            ),
        ),
        MakerRuntimeParameterBlocker::ZeroTradeFlowMaxSamples => (
            TRADE_FLOW_MAX_SAMPLES_FIELD,
            POSITIVE_REQUIRED_CODE,
            "must be > 0 (a zero sample cap retains no trades, so a μ can never be produced)"
                .to_string(),
        ),
        MakerRuntimeParameterBlocker::ZeroMuMinClassifiedSamples => (
            MU_MIN_CLASSIFIED_SAMPLES_FIELD,
            POSITIVE_REQUIRED_CODE,
            "must be > 0 (a zero warmup threshold would admit a μ from an empty window)".to_string(),
        ),
        MakerRuntimeParameterBlocker::MuMinClassifiedSamplesAboveMax {
            min_classified_samples,
            max_samples,
        } => (
            MU_MIN_CLASSIFIED_SAMPLES_FIELD,
            UNSATISFIABLE_WARMUP_CODE,
            format!(
                "({min_classified_samples}) must be <= {TRADE_FLOW_MAX_SAMPLES_FIELD} ({max_samples})"
            ),
        ),
        MakerRuntimeParameterBlocker::ZeroMuStaleWindowMs => (
            MU_STALE_WINDOW_MS_FIELD,
            POSITIVE_REQUIRED_CODE,
            "must be > 0 (a zero staleness window marks every reading stale, blocking μ permanently)"
                .to_string(),
        ),
        MakerRuntimeParameterBlocker::MuMinFloorOutOfRange { floor } => (
            MU_MIN_FLOOR_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            format!("({floor}) must be finite and in the open interval (0, 1)"),
        ),
        MakerRuntimeParameterBlocker::ZeroRequoteMinIntervalMs => (
            REQUOTE_MIN_INTERVAL_MS_FIELD,
            POSITIVE_REQUIRED_CODE,
            "must be > 0 (a zero requote interval disables the same-tick throttle)".to_string(),
        ),
        MakerRuntimeParameterBlocker::ZeroQuoteIntervalMs => (
            QUOTE_INTERVAL_MS_FIELD,
            POSITIVE_REQUIRED_CODE,
            "must be > 0 (a zero quote-loop interval would schedule a degenerate timer)".to_string(),
        ),
        MakerRuntimeParameterBlocker::QuoteIntervalNanosOverflow { quote_interval_ms } => (
            QUOTE_INTERVAL_MS_FIELD,
            CONVERSION_OVERFLOW_CODE,
            format!(
                "({quote_interval_ms}) must be small enough that its millisecond-to-nanosecond conversion does not overflow u64"
            ),
        ),
    };
    errors.push(ValidationError {
        field: format!("{field_prefix}.{field}"),
        code,
        message,
    });
}

fn validate_market_portfolio_bounds(
    field_prefix: &str,
    inputs: RuntimeBoundInputs,
    errors: &mut Vec<ValidationError>,
) {
    let max_active_markets = inputs
        .market_portfolio_max_active_markets
        .and_then(|value| match usize::try_from(value) {
            Ok(value) => Some(value),
            Err(_) => {
                errors.push(ValidationError {
                    field: format!("{field_prefix}.{MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD}"),
                    code: VALUE_OUT_OF_RANGE_CODE,
                    message: "must fit the platform usize used by the portfolio planner"
                        .to_string(),
                });
                None
            }
        });
    let policy_inputs = MakerMarketPortfolioPolicyInputs {
        max_active_markets,
        total_bankroll_notional: inputs.market_portfolio_total_bankroll_notional,
        min_slot_notional: inputs.market_portfolio_min_slot_notional,
    };
    for blocker in maker_market_portfolio_policy_input_blockers(policy_inputs) {
        push_market_portfolio_policy_blocker(field_prefix, blocker, errors);
    }
    let policy = match (
        max_active_markets,
        inputs.market_portfolio_total_bankroll_notional,
        inputs.market_portfolio_min_slot_notional,
    ) {
        (Some(max_active_markets), Some(total_bankroll_notional), Some(min_slot_notional)) => {
            Some(MakerMarketPortfolioPolicy {
                max_active_markets,
                total_bankroll_notional,
                min_slot_notional,
            })
        }
        _ => None,
    };
    if let Some(declared_market_count) = inputs.declared_market_count {
        for blocker in
            maker_market_portfolio_declaration_blockers(MakerMarketPortfolioDeclarationInputs {
                policy,
                declared_market_count,
            })
        {
            push_market_portfolio_declaration_blocker(field_prefix, blocker, errors);
        }
    }
}

fn push_market_portfolio_policy_blocker(
    field_prefix: &str,
    blocker: MakerMarketPortfolioBlocker<'_>,
    errors: &mut Vec<ValidationError>,
) {
    let (field, code, message) = match blocker {
        MakerMarketPortfolioBlocker::InvalidMaxActiveMarkets => (
            MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD,
            POSITIVE_REQUIRED_CODE,
            "must be > 0 so the maker can select at least one market",
        ),
        MakerMarketPortfolioBlocker::InvalidTotalBankroll => (
            MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            "must be a positive finite bankroll notional",
        ),
        MakerMarketPortfolioBlocker::InvalidMinSlotNotional => (
            MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD,
            VALUE_OUT_OF_RANGE_CODE,
            "must be a positive finite per-market slot notional",
        ),
        MakerMarketPortfolioBlocker::EmptyCandidateMarketKey
        | MakerMarketPortfolioBlocker::DuplicateCandidateMarket { .. }
        | MakerMarketPortfolioBlocker::EmptyActiveMarketKey
        | MakerMarketPortfolioBlocker::DuplicateActiveMarket { .. }
        | MakerMarketPortfolioBlocker::NoEligibleCandidates
        | MakerMarketPortfolioBlocker::InsufficientSlotAllocation => return,
    };
    errors.push(ValidationError {
        field: format!("{field_prefix}.{field}"),
        code,
        message: message.to_string(),
    });
}

fn push_market_portfolio_declaration_blocker(
    field_prefix: &str,
    blocker: MakerMarketPortfolioDeclarationBlocker,
    errors: &mut Vec<ValidationError>,
) {
    let (field, code, message) = match blocker {
        MakerMarketPortfolioDeclarationBlocker::EmptyMarkets => (
            MARKETS_FIELD,
            EMPTY_MARKETS_CODE,
            "must declare at least one market".to_string(),
        ),
        MakerMarketPortfolioDeclarationBlocker::MarketsAboveActiveCap {
            declared_market_count,
            max_active_markets,
        } => (
            MARKETS_FIELD,
            MARKETS_ABOVE_ACTIVE_CAP_CODE,
            format!(
                "declares {declared_market_count} markets but {MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD} is {max_active_markets}"
            ),
        ),
        MakerMarketPortfolioDeclarationBlocker::BankrollBelowMinSlotFloor {
            total_bankroll_notional,
            min_slot_notional,
            ..
        } => (
            MARKET_PORTFOLIO_TOTAL_BANKROLL_NOTIONAL_FIELD,
            BANKROLL_BELOW_MIN_SLOT_CODE,
            format!(
                "must be >= min(declared markets, max_active_markets) * {MARKET_PORTFOLIO_MIN_SLOT_NOTIONAL_FIELD}; configured total_bankroll_notional={total_bankroll_notional}, min_slot_notional={min_slot_notional}"
            ),
        ),
    };
    errors.push(ValidationError {
        field: format!("{field_prefix}.{field}"),
        code,
        message,
    });
}

fn validate_non_negative_u64_field(
    table: &toml::map::Map<String, Value>,
    field_prefix: &str,
    field_name: &'static str,
    missing_code: &'static str,
    errors: &mut Vec<ValidationError>,
) -> Option<u64> {
    let value = validate_required_field(
        table,
        field_prefix,
        field_name,
        missing_code,
        BinaryOracleMakerFieldType::Integer,
        errors,
    )?;
    let value = value
        .as_integer()
        .expect("integer field was type-checked before extraction");
    if value < 0 {
        errors.push(ValidationError {
            field: format!("{field_prefix}.{field_name}"),
            code: VALUE_OUT_OF_RANGE_CODE,
            message: "must be a non-negative integer that fits u64".to_string(),
        });
        return None;
    }
    Some(value as u64)
}

fn validate_float_field(
    table: &toml::map::Map<String, Value>,
    field_prefix: &str,
    field_name: &'static str,
    missing_code: &'static str,
    errors: &mut Vec<ValidationError>,
) -> Option<f64> {
    let value = validate_required_field(
        table,
        field_prefix,
        field_name,
        missing_code,
        BinaryOracleMakerFieldType::Float,
        errors,
    )?;
    value.as_float_or_integer()
}

fn validate_markets_array_field<'a>(
    table: &'a toml::map::Map<String, Value>,
    field_prefix: &str,
    errors: &mut Vec<ValidationError>,
) -> Option<&'a [Value]> {
    let value = validate_required_field(
        table,
        field_prefix,
        MARKETS_FIELD,
        MISSING_MARKETS_CODE,
        BinaryOracleMakerFieldType::Array,
        errors,
    )?;
    let markets = value
        .as_array()
        .expect("array field was type-checked before extraction");
    Some(markets.as_slice())
}

fn validate_required_field<'a>(
    table: &'a toml::map::Map<String, Value>,
    field_prefix: &str,
    field_name: &'static str,
    missing_code: &'static str,
    field_type: BinaryOracleMakerFieldType,
    errors: &mut Vec<ValidationError>,
) -> Option<&'a Value> {
    let field = format!("{field_prefix}.{field_name}");
    let Some(value) = table.get(field_name) else {
        push_missing_field(errors, field, missing_code, field_type);
        return None;
    };
    if !field_type_matches(field_type, value) {
        push_wrong_type(errors, field, field_type, value);
        return None;
    }
    Some(value)
}

fn push_missing_field(
    errors: &mut Vec<ValidationError>,
    field: String,
    code: &'static str,
    field_type: BinaryOracleMakerFieldType,
) {
    errors.push(ValidationError {
        field,
        code,
        message: format!("is missing required {} field", field_type.expected()),
    });
}

fn push_wrong_type(
    errors: &mut Vec<ValidationError>,
    field: String,
    field_type: BinaryOracleMakerFieldType,
    value: &Value,
) {
    errors.push(ValidationError {
        field,
        code: WRONG_TYPE_CODE,
        message: format!(
            "must be {} {}, got {} value",
            field_type.article(),
            field_type.expected(),
            value.type_str()
        ),
    });
}

fn field_type_matches(field_type: BinaryOracleMakerFieldType, value: &Value) -> bool {
    match field_type {
        BinaryOracleMakerFieldType::String => value.as_str().is_some(),
        BinaryOracleMakerFieldType::Integer => value.as_integer().is_some(),
        BinaryOracleMakerFieldType::Float => value.as_float_or_integer().is_some(),
        BinaryOracleMakerFieldType::Array => value.as_array().is_some(),
    }
}

fn validate_canonical_identity_field(
    table: &toml::map::Map<String, Value>,
    field_prefix: &str,
    field: &str,
    code: &'static str,
    errors: &mut Vec<ValidationError>,
) {
    let Some(value) = table.get(field).and_then(Value::as_str) else {
        return;
    };
    if !stable_identity_field_is_canonical(value) {
        errors.push(ValidationError {
            field: format!("{field_prefix}.{field}"),
            code,
            message: "must be a non-empty, unpadded string".to_string(),
        });
    }
}

/// Fail loud at load when `order_id_tag` contains the `-` delimiter the maker's
/// per-leg client order id is joined on (`runtime::make_leg_identity`). The tag is
/// the leading, free-form id component; constraining it to be delimiter-free keeps
/// the positional id encoding unambiguous (the remaining free-form component,
/// `market_key`, is right-anchored by the decimal window-start/generation and the
/// `yes`/`no` leg), so two distinct markets can never mint the same client order
/// id. A missing or non-string `order_id_tag` is already reported by
/// `validate_string_field`, so it is skipped here.
fn validate_order_id_tag_delimiter_free(
    table: &toml::map::Map<String, Value>,
    field_prefix: &str,
    errors: &mut Vec<ValidationError>,
) {
    let Some(value) = table.get(ORDER_ID_TAG_FIELD).and_then(Value::as_str) else {
        return;
    };
    if value.contains('-') {
        errors.push(ValidationError {
            field: format!("{field_prefix}.{ORDER_ID_TAG_FIELD}"),
            code: INVALID_ORDER_ID_TAG_CODE,
            message: format!("must not contain the `-` order-id delimiter, got `{value}`"),
        });
    }
}

/// Fail loud at load when `oms_type` is a string that does not parse as a
/// NautilusTrader `OmsType`. The string-presence/type check above only proves
/// `oms_type` is a string; `BinaryOracleMaker::new` then parses it with
/// `.expect(...)`, so an unparseable value that passed validation would panic at
/// build instead of surfacing as a clean validation error. Mirrors the taker's
/// `parse_configured_oms_type` (`value.parse::<OmsType>()`) so validation
/// guarantees exactly what build assumes. A missing or non-string `oms_type` is
/// already reported by `validate_string_field`, so it is skipped here.
fn validate_oms_type_parses(
    table: &toml::map::Map<String, Value>,
    field_prefix: &str,
    errors: &mut Vec<ValidationError>,
) {
    let Some(value) = table.get(OMS_TYPE_FIELD).and_then(Value::as_str) else {
        return;
    };
    if value.parse::<OmsType>().is_err() {
        errors.push(ValidationError {
            field: format!("{field_prefix}.{OMS_TYPE_FIELD}"),
            code: INVALID_OMS_TYPE_CODE,
            message: format!("must be a NautilusTrader OmsType, got `{value}`"),
        });
    }
}

fn validate_string_field(
    table: &toml::map::Map<String, Value>,
    field_prefix: &str,
    field_name: &'static str,
    missing_code: &'static str,
    errors: &mut Vec<ValidationError>,
) {
    let _ = validate_required_field(
        table,
        field_prefix,
        field_name,
        missing_code,
        BinaryOracleMakerFieldType::String,
        errors,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_raw() -> Value {
        toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            client_id = "maker_execution_client"
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            quote_interval_ms = 1000
            market_portfolio_max_active_markets = 3
            market_portfolio_total_bankroll_notional = 1500.0
            market_portfolio_min_slot_notional = 100.0
            markets_config_digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

            [[markets]]
            market_key = "eth-hourly"
            family_key = "updown"
            underlying_asset = "ETH"
            cadence_seconds = 3600
            cadence_slug_token = "1h"
        }
        .into()
    }

    #[test]
    fn parse_config_round_trips_full_config() {
        let config = parse_config(&valid_raw()).expect("valid config parses");
        assert_eq!(config.strategy_id, "BINARY-ORACLE-MAKER-001");
        assert_eq!(config.order_id_tag, "001");
        assert_eq!(config.oms_type, "netting");
        assert_eq!(config.client_id, "maker_execution_client");
        assert_eq!(config.trade_flow_window_secs, 600);
        assert_eq!(config.trade_flow_max_samples, 1000);
        assert_eq!(config.mu_min_classified_samples, 4);
        assert_eq!(config.mu_stale_window_ms, 60_000);
        assert_eq!(config.mu_min_floor, 0.05);
        assert_eq!(config.requote_min_interval_ms, 500);
        assert_eq!(config.quote_interval_ms, 1000);
        assert_eq!(config.market_portfolio_max_active_markets, 3);
        assert_eq!(config.market_portfolio_total_bankroll_notional, 1500.0);
        assert_eq!(config.market_portfolio_min_slot_notional, 100.0);
        assert_eq!(
            config.markets_config_digest,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert_eq!(config.markets.len(), 1);
        assert_eq!(config.markets[0].market_key, "eth-hourly");
        assert_eq!(config.markets[0].family_key, "updown");
        assert_eq!(config.markets[0].underlying_asset, "ETH");
        assert_eq!(config.markets[0].cadence_seconds, 3600);
        assert_eq!(config.markets[0].cadence_slug_token, "1h");
        assert_eq!(config.markets[0].static_condition_id, None);
    }

    #[test]
    fn validate_config_accepts_valid_config() {
        let mut errors = Vec::new();
        validate_config(&valid_raw(), "strategy", &mut errors);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn parse_and_validation_reject_noncanonical_strategy_and_order_identities() {
        for (field, value) in [
            (STRATEGY_ID_FIELD, ""),
            (STRATEGY_ID_FIELD, "   "),
            (STRATEGY_ID_FIELD, " maker"),
            (STRATEGY_ID_FIELD, "maker "),
            (ORDER_ID_TAG_FIELD, ""),
            (ORDER_ID_TAG_FIELD, "   "),
            (ORDER_ID_TAG_FIELD, " 001"),
            (ORDER_ID_TAG_FIELD, "001 "),
        ] {
            let mut raw = valid_raw();
            raw.as_table_mut()
                .expect("config is a table")
                .insert(field.to_string(), Value::String(value.to_string()));
            let mut errors = Vec::new();
            validate_config(&raw, "strategy", &mut errors);
            assert!(
                errors
                    .iter()
                    .any(|error| error.field == format!("strategy.{field}")),
                "validation accepted {field}={value:?}: {errors:?}"
            );
            assert!(
                parse_config(&raw).is_err(),
                "builder parsing accepted {field}={value:?}"
            );
        }
    }

    #[test]
    fn parse_and_validation_accept_internal_identity_whitespace() {
        let mut raw = valid_raw();
        raw.as_table_mut().expect("config is a table").insert(
            STRATEGY_ID_FIELD.to_string(),
            Value::String("Maker New York".to_string()),
        );
        let mut errors = Vec::new();
        validate_config(&raw, "strategy", &mut errors);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert!(parse_config(&raw).is_ok());
    }

    #[test]
    fn validate_config_rejects_order_id_tag_with_delimiter() {
        // The maker joins the per-leg client order id on `-`
        // (`runtime::make_leg_identity`); an `order_id_tag` containing the delimiter
        // would make the positional encoding ambiguous, so it must fail loud at
        // config load rather than risk minting colliding ids. Without the
        // delimiter-free check this passes validation, so the assertion is
        // differential.
        let mut raw = valid_raw();
        raw.as_table_mut().expect("config is a table").insert(
            "order_id_tag".to_string(),
            Value::String("00-1".to_string()),
        );
        let mut errors = Vec::new();
        validate_config(&raw, "strategy", &mut errors);
        assert!(
            errors
                .iter()
                .any(|error| error.code == INVALID_ORDER_ID_TAG_CODE),
            "a hyphenated order_id_tag must be rejected: {errors:?}"
        );
    }

    #[test]
    fn validate_config_rejects_trade_flow_window_secs_wrong_type() {
        let mut raw = valid_raw();
        raw.as_table_mut().expect("config is a table").insert(
            TRADE_FLOW_WINDOW_SECS_FIELD.to_string(),
            Value::String("abc".to_string()),
        );
        let mut errors = Vec::new();

        validate_config(&raw, "strategy", &mut errors);

        assert!(
            errors.iter().any(|error| {
                error.field == format!("strategy.{TRADE_FLOW_WINDOW_SECS_FIELD}")
                    && error.code == WRONG_TYPE_CODE
            }),
            "trade_flow_window_secs string must fail registry validation: {errors:?}"
        );
    }

    #[test]
    fn validate_config_rejects_zero_market_portfolio_max_active_markets() {
        let mut raw = valid_raw();
        raw.as_table_mut().expect("config is a table").insert(
            MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD.to_string(),
            Value::Integer(0),
        );
        let mut errors = Vec::new();

        validate_config(&raw, "strategy", &mut errors);

        assert!(
            errors.iter().any(|error| {
                error.field == format!("strategy.{MARKET_PORTFOLIO_MAX_ACTIVE_MARKETS_FIELD}")
                    && error.code == POSITIVE_REQUIRED_CODE
            }),
            "zero market_portfolio_max_active_markets must fail registry validation: {errors:?}"
        );
    }

    #[test]
    fn parse_config_rejects_missing_mu_knob() {
        // The flat config requires every μ knob (non-Option, deny_unknown_fields);
        // a table missing one fails loud at parse rather than building a maker
        // with an unspecified μ knob.
        let raw: Value = toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            client_id = "maker_execution_client"
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            requote_min_interval_ms = 500
            market_portfolio_max_active_markets = 3
            market_portfolio_total_bankroll_notional = 1500.0
            market_portfolio_min_slot_notional = 100.0
        }
        .into();
        assert!(
            parse_config(&raw).is_err(),
            "missing mu_min_floor must fail to parse"
        );
    }

    #[test]
    fn parse_config_rejects_missing_market_portfolio_knob() {
        let raw: Value = toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            client_id = "maker_execution_client"
            trade_flow_window_secs = 600
            trade_flow_max_samples = 1000
            mu_min_classified_samples = 4
            mu_stale_window_ms = 60000
            mu_min_floor = 0.05
            requote_min_interval_ms = 500
            market_portfolio_max_active_markets = 3
            market_portfolio_total_bankroll_notional = 1500.0
        }
        .into();
        assert!(
            parse_config(&raw).is_err(),
            "missing market_portfolio_min_slot_notional must fail to parse"
        );
    }

    #[test]
    fn validate_config_flags_missing_field() {
        let raw: Value = toml::toml! {
            order_id_tag = "001"
            oms_type = "netting"
        }
        .into();
        let mut errors = Vec::new();
        validate_config(&raw, "strategy", &mut errors);
        assert!(
            errors
                .iter()
                .any(|error| error.code == MISSING_STRATEGY_ID_CODE),
            "expected missing_strategy_id, got: {errors:?}"
        );
    }

    #[test]
    fn validate_config_flags_unparseable_oms_type() {
        let raw: Value = toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "not-an-oms-type"
            client_id = "maker_execution_client"
        }
        .into();
        let mut errors = Vec::new();
        validate_config(&raw, "strategy", &mut errors);
        assert!(
            errors
                .iter()
                .any(|error| error.code == INVALID_OMS_TYPE_CODE),
            "expected invalid_oms_type, got: {errors:?}"
        );
    }

    #[test]
    fn validate_config_accepts_parseable_oms_type() {
        // The minimal envelope's `oms_type = "netting"` must parse as an NT
        // OmsType so build's `.expect(...)` can never fire on validated config.
        assert!(
            "netting".parse::<OmsType>().is_ok(),
            "minimal envelope oms_type must parse as an NT OmsType"
        );
        let mut errors = Vec::new();
        validate_config(&valid_raw(), "strategy", &mut errors);
        assert!(
            !errors
                .iter()
                .any(|error| error.code == INVALID_OMS_TYPE_CODE),
            "unexpected invalid_oms_type: {errors:?}"
        );
    }

    #[test]
    fn validate_config_flags_unknown_field() {
        let raw: Value = toml::toml! {
            strategy_id = "BINARY-ORACLE-MAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            client_id = "maker_execution_client"
            unexpected = "value"
        }
        .into();
        let mut errors = Vec::new();
        validate_config(&raw, "strategy", &mut errors);
        assert!(
            errors.iter().any(|error| error.code == UNKNOWN_FIELD_CODE),
            "expected unknown_field, got: {errors:?}"
        );
    }
}
