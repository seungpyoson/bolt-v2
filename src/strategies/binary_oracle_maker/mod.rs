//! `binary_oracle_maker` strategy (Slice 2, #488).
//!
//! The strategy compiles, is selectable by the `binary_oracle_maker` archetype
//! key, registers through the shared `production_strategy_registry()`, and
//! validates. Slice 2 adds the μ (informed-fraction) runtime state ([`mu`]): the
//! maker overrides `on_trade` to feed each instrument's signed trade-flow buffer,
//! from which the shared estimator and fail-closed health gate derive μ. Slice
//! 6c adds the strategy-owned shell method that routes a caller-resolved maker
//! quote tick through the shared quote planner, order compiler, and
//! execution/admission policy; the strategy still has no autonomous subscription
//! or quote loop until later runtime slices. The
//! NautilusTrader surface (`core: StrategyCore`, the shared strategy macro, the
//! `StrategyBuilder` impl) mirrors `binary_oracle_edge_taker` *structurally* —
//! it does not copy taker behaviour.

use std::time::Duration;

use anyhow::Result;
use nautilus_common::{actor::DataActor, timer::TimeEvent};
use nautilus_model::{
    data::TradeTick,
    enums::OmsType,
    identifiers::{ClientId, StrategyId},
    instruments::{Instrument, InstrumentAny},
};
use nautilus_trading::{StrategyConfig, StrategyCore};
use rust_decimal::{Decimal, prelude::FromPrimitive};
use toml::Value;

use crate::bolt_v3_strategy_context::StrategyBuildContext;
use binding::MakerConcreteMarketIdentity;

use crate::{
    bolt_v3_current_evidence::{
        EvidenceRequoteLeg, ObservationRecordOutcome, RequoteActionCostClass,
        RequoteThrottleBlockReason, RequoteThrottleBound, RequoteThrottleObservationFact,
    },
    bolt_v3_loss_governor::LossAdmissionDecision,
    bolt_v3_maker_market_selection::MakerMarketPortfolioPolicy,
    bolt_v3_maker_mu_estimator::{MuEstimatorConfig, MuHealthConfig},
    bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
    bolt_v3_maker_order_dispatch::{MakerOrderDispatchInput, MakerOrderDispatchOutcome},
    bolt_v3_maker_order_plan::{
        MakerLegBinding, MakerMarketActionOrderInput, maker_order_plan_from_market_action,
    },
    bolt_v3_maker_quote_control::QuoteControlBlockReason,
    bolt_v3_maker_quote_plan::MakerQuotePlanInputs,
    bolt_v3_maker_quote_set::QuoteSetLegDecision,
    bolt_v3_maker_risk::{
        MakerLossRiskPolicy, MakerRiskDecision, apply_maker_risk_mode,
        maker_risk_mode_for_loss_decision,
    },
    bolt_v3_maker_runtime_order::{
        MakerRuntimeLegOrderDispatchOutcome, MakerRuntimeOrderDispatchInput,
        MakerRuntimeOrderDispatchOutcome, dispatch_maker_runtime_order_plan_with_command_router,
    },
    bolt_v3_maker_runtime_quote::{
        MakerRuntimeQuoteBlockReason, MakerRuntimeQuoteDecision, MakerRuntimeQuoteInput,
        MakerRuntimeQuoteSetInput, MakerRuntimeReferenceFairValueBlockReason,
        MakerRuntimeReferenceFairValueDecision, MakerRuntimeReferenceFairValueInput,
        blocked_runtime_quote_decision, maker_reference_current_price_fair_value_decision,
        plan_maker_runtime_quote, runtime_window_contains,
    },
    bolt_v3_numeric::NANOS_PER_MILLI_U64,
    bolt_v3_order_execution::{
        BoltV3MakerOrderRoutingContext,
        route_maker_order_command as route_maker_order_command_through_policy,
    },
    bolt_v3_order_intent::NtOrderTemplate,
    bolt_v3_quote_lifecycle::{Leg, LegState, MarketAction, MarketQuote},
    bolt_v3_quoting::QuoteTargets,
    bolt_v3_realized_volatility::RealizedVolSnapshot,
    bolt_v3_reference_price::{ReferencePriceSelector, ReferenceQuote},
    bolt_v3_requote_budget::RequoteBudgetPair,
    bolt_v3_target_identity::stable_identity_field_is_canonical,
    bolt_v3_timestamp_domain::LocalReceiveMs,
    bolt_v3_trade_flow::SignedTradeFlowConfig,
    strategies::binary_oracle_maker::mu::MakerMuState,
    strategies::binary_oracle_maker::runtime::MakerRuntime,
    strategies::registry::{StrategyBuilder, ValidationError},
};

pub mod archetype;
pub mod binding;
mod config;
pub mod mu;
pub mod runtime;

pub use config::{
    BinaryOracleMakerBuilder, BinaryOracleMakerConfig, parse_config, validate_config,
};

/// The archetype key for the maker — its `StrategyBuilder::kind`,
/// `RUNTIME_BINDING.key`, validation-binding key, and operator TOML
/// `strategy_archetype` value are all this single constant.
pub const KEY: &str = "binary_oracle_maker";
const REQUOTE_THROTTLE_FRESH_SUBMIT_SUBMIT_COST: u64 = 1;
const REQUOTE_THROTTLE_FRESH_SUBMIT_REST_COST: u64 = 1;
const REQUOTE_THROTTLE_CANCEL_RESUBMIT_SUBMIT_COST: u64 = 1;
const REQUOTE_THROTTLE_CANCEL_RESUBMIT_REST_COST: u64 = 2;
const REQUOTE_THROTTLE_CANCEL_SUBMIT_COST: u64 = 0;
const REQUOTE_THROTTLE_CANCEL_REST_COST: u64 = 1;

/// Binary-oracle market-making strategy. Carries the NautilusTrader envelope
/// (`core`), its parsed config, and the per-instrument μ (informed-fraction)
/// runtime state. Compiled maker order commands route through the shared
/// execution policy using the retained build context; pricing/exposure loops
/// arrive in later slices.
pub struct BinaryOracleMaker {
    core: StrategyCore,
    config: BinaryOracleMakerConfig,
    context: StrategyBuildContext,
    mu: MakerMuState,
    runtime: MakerRuntime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRuntimeQuoteRouteInput<'a> {
    pub quote_plan: MakerQuotePlanInputs<'a>,
    pub quote_set: MakerRuntimeQuoteSetInput,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub submit_order_prefix: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRuntimeQuoteRouteOutcome {
    pub quote: MakerRuntimeQuoteDecision,
    pub orders: Option<MakerRuntimeOrderDispatchOutcome>,
}

/// The identity of a throttled-requote episode, which is what decides whether a
/// record is new -- not the observation attached to it.
///
/// `action_cost_class` and `bound_by` are deliberately absent. Both vary while
/// the blocked state does not: `action_cost_class` follows whatever the strategy
/// was attempting this tick, and `bound_by` is computed from `now_ms` against the
/// budget window, so together they give one blocked leg seventeen reachable
/// spellings. They remain on the emitted evidence, where they are diagnostics;
/// they are not identity.
///
/// `block_reason` stays. It is the blocker category, which is semantic state, and
/// it is what a second reason would have to differ in to deserve its own record.
///
/// The configured market key remains for active-set ownership, but is not the
/// concrete identity: a cadence successor or reissued instrument can resolve
/// under the same key. The validated venue identity, window start, and leg
/// instruments distinguish those successors.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RequoteThrottleEpisodeId {
    market_key: String,
    market: MakerConcreteMarketIdentity,
    leg: Leg,
    block_reason: RequoteThrottleBlockReason,
}

/// Which market a throttle record is about: the unique key, and the family it was
/// selected from.
///
/// They travel together because a record needs both -- the key to be identifiable
/// and the family to be groupable -- and they are one type because substituting
/// one for the other is exactly the mistake this replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ThrottledMarket<'a> {
    key: &'a str,
    family_key: &'a str,
    concrete_identity: &'a MakerConcreteMarketIdentity,
}

/// Runtime-owned coordinates that authorize a caller to address one active
/// market. Being authorized does not imply that the market is quotable at this
/// instant: rotating families deliberately preload a future window so its market
/// data can be subscribed before the window opens.
struct ActiveQuoteAuthority {
    family_key: String,
    underlying_asset: String,
    interval_start_ms: u64,
    interval_end_ms: u64,
}

impl ActiveQuoteAuthority {
    fn window_contains(&self, evaluation_ms: u64) -> bool {
        runtime_window_contains(self.interval_start_ms, self.interval_end_ms, evaluation_ms)
    }
}

#[allow(clippy::too_many_arguments)]
fn requote_throttle_observation(
    strategy_id: String,
    market: ThrottledMarket<'_>,
    leg: Leg,
    now_ms: u64,
    action_cost_class: RequoteActionCostClass,
    block_reason: RequoteThrottleBlockReason,
    bound_by: RequoteThrottleBound,
    budget: &RequoteBudgetPair,
) -> RequoteThrottleObservationFact {
    RequoteThrottleObservationFact {
        strategy_id,
        family_key: market.family_key.to_string(),
        market_id: Some(market.concrete_identity.gamma_market_id().to_string()),
        leg: match leg {
            Leg::Yes => EvidenceRequoteLeg::Yes,
            Leg::No => EvidenceRequoteLeg::No,
        },
        now_ms,
        observed_at_ns: now_ms.saturating_mul(NANOS_PER_MILLI_U64),
        action_cost_class,
        block_reason,
        bound_by,
        submit_commands_in_window: budget.submit_commands_in_window(),
        submit_command_cap: budget.submit_command_cap(),
        submit_window_ms: budget.submit_window_ms(),
        rest_cost_in_window: budget.rest_cost_in_window(),
        rest_cap_per_minute: budget.rest_cap_per_window(),
        rest_window_ms: budget.rest_window_ms(),
        min_interval_ms: budget.min_interval_ms(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRuntimeReferenceQuoteRouteInput<'a> {
    pub reference_fair_value: BinaryOracleMakerReferenceFairValueInput<'a>,
    pub quote_plan: MakerQuotePlanInputs<'a>,
    pub quote_set: MakerRuntimeQuoteSetInput,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub submit_order_prefix: &'a str,
}

/// A validated opening strike bound to one configured market, asset, and window.
///
/// The route compares all three identity fields with the active runtime binding
/// before fair-value evaluation. This prevents a numerically valid strike from a
/// sibling market or cadence window from being reused accidentally.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinaryOracleMakerStrikePrice<'a> {
    market_key: &'a str,
    underlying_asset: &'a str,
    interval_start_ms: u64,
    price: f64,
}

impl<'a> BinaryOracleMakerStrikePrice<'a> {
    pub fn try_new(
        market_key: &'a str,
        underlying_asset: &'a str,
        interval_start_ms: u64,
        price: f64,
    ) -> std::result::Result<Self, String> {
        if !stable_identity_field_is_canonical(market_key) {
            return Err("binary oracle maker strike market_key is invalid".to_string());
        }
        if underlying_asset.is_empty() || underlying_asset.chars().any(char::is_whitespace) {
            return Err("binary oracle maker strike underlying_asset is invalid".to_string());
        }
        if interval_start_ms == 0 {
            return Err(
                "binary oracle maker strike interval_start_ms must be positive".to_string(),
            );
        }
        if !price.is_finite() || price <= 0.0 {
            return Err("binary oracle maker strike price must be positive and finite".to_string());
        }
        Ok(Self {
            market_key,
            underlying_asset,
            interval_start_ms,
            price,
        })
    }

    #[must_use]
    pub const fn market_key(&self) -> &str {
        self.market_key
    }

    #[must_use]
    pub const fn underlying_asset(&self) -> &str {
        self.underlying_asset
    }

    #[must_use]
    pub const fn interval_start_ms(&self) -> u64 {
        self.interval_start_ms
    }

    #[must_use]
    pub const fn price(&self) -> f64 {
        self.price
    }
}

/// Reference observations and pricing parameters whose market identity is
/// validated against the active runtime binding inside the route. Family,
/// cadence window, asset, and time to expiry come from that binding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinaryOracleMakerReferenceFairValueInput<'a> {
    pub reference_quotes: &'a [ReferenceQuote],
    pub strike: Option<BinaryOracleMakerStrikePrice<'a>>,
    pub realized_volatility_snapshot: &'a RealizedVolSnapshot,
    pub realized_volatility_max_source_age_ms: Option<u64>,
    pub pricing_kurtosis: f64,
    pub evaluation_receive_ms: LocalReceiveMs,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRuntimeReferenceQuoteRouteOutcome {
    pub fair_value: MakerRuntimeReferenceFairValueDecision,
    pub quote: Option<MakerRuntimeQuoteDecision>,
    pub orders: Option<MakerRuntimeOrderDispatchOutcome>,
    pub blocked_by: Option<BinaryOracleMakerRuntimeReferenceQuoteBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOracleMakerRuntimeReferenceQuoteBlockReason {
    FairValue(MakerRuntimeReferenceFairValueBlockReason),
    Quote(MakerRuntimeQuoteBlockReason),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerMarketActionRouteInput<'a> {
    pub action: MakerMarketActionOrderInput,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub submit_order_prefix: &'a str,
    /// Strategy-owned gross value assumption for a submit action. Cancel-only
    /// actions do not consume it.
    pub gross_expected_value: Decimal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerMarketActionRouteOutcome {
    pub order: MakerRuntimeLegOrderDispatchOutcome,
    pub orders: MakerRuntimeOrderDispatchOutcome,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRiskRouteInput<'a> {
    pub loss_decision: &'a LossAdmissionDecision,
    pub policy: MakerLossRiskPolicy,
    pub targets: QuoteTargets,
    pub yes_quantity: f64,
    pub no_quantity: f64,
    pub yes: MakerLegBinding,
    pub no: MakerLegBinding,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub submit_order_prefix: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerRiskRouteOutcome {
    pub risk: MakerRiskDecision,
    pub orders: Option<MakerRuntimeOrderDispatchOutcome>,
}

fn maker_command_gross_expected_value(
    command: &MakerCompiledOrderCommand,
    fair_probability_up: f64,
) -> Result<Decimal> {
    let MakerCompiledOrderCommand::Submit { leg, inputs, .. } = command else {
        return Ok(Decimal::ZERO);
    };
    let fair_probability_up = Decimal::from_f64(fair_probability_up)
        .filter(|value| (Decimal::ZERO..=Decimal::ONE).contains(value))
        .ok_or_else(|| anyhow::anyhow!("maker economics fair probability is invalid"))?;
    let outcome_probability = match leg {
        Leg::Yes => fair_probability_up,
        Leg::No => Decimal::ONE - fair_probability_up,
    };
    let price = inputs
        .price
        .ok_or_else(|| anyhow::anyhow!("maker economics requires a limit price"))?
        .to_string()
        .parse::<Decimal>()?;
    let quantity = inputs.quantity.to_string().parse::<Decimal>()?;
    outcome_probability
        .checked_sub(price)
        .and_then(|edge| edge.checked_mul(quantity))
        .ok_or_else(|| anyhow::anyhow!("maker gross expected value arithmetic overflow"))
}

impl std::fmt::Debug for BinaryOracleMaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinaryOracleMaker")
            .field("core", &self.core)
            .field("config", &self.config)
            .field("mu", &self.mu)
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl BinaryOracleMaker {
    pub fn new(config: BinaryOracleMakerConfig, context: StrategyBuildContext) -> Self {
        let oms_type = config
            .oms_type
            .parse::<OmsType>()
            .expect("validated binary_oracle_maker oms_type");
        let mu = build_mu_state(&config);
        Self {
            core: StrategyCore::new(
                StrategyConfig::builder()
                    .strategy_id(StrategyId::from(config.strategy_id.as_str()))
                    .order_id_tag(config.order_id_tag.clone())
                    .oms_type(oms_type)
                    .build()
                    .expect("validated binary_oracle_maker strategy config"),
            ),
            config,
            context,
            mu,
            runtime: MakerRuntime::empty(),
        }
    }

    /// The parsed maker config (read by later slices once they add behaviour).
    pub fn maker_config(&self) -> &BinaryOracleMakerConfig {
        &self.config
    }

    /// The maker's per-market runtime state (active markets + their assigned order
    /// identities). Read by the integration tests and later runtime slices.
    pub fn runtime(&self) -> &MakerRuntime {
        &self.runtime
    }

    /// Validate the caller's market ownership against the active runtime before
    /// pricing, identity minting, or quote planning can mutate state. The returned
    /// window is availability, not authority: a valid preloaded future market is
    /// authorized but must wait until its half-open cadence window begins.
    fn ensure_active_quote_authority(
        &self,
        market_key: &str,
        input_family_key: &str,
    ) -> Result<ActiveQuoteAuthority> {
        let runtime_market = self.runtime.market(market_key).ok_or_else(|| {
            anyhow::anyhow!(
                "binary_oracle_maker cannot route quote for inactive market: market_key={market_key}"
            )
        })?;
        let runtime_family_key = runtime_market.family_key();
        anyhow::ensure!(
            input_family_key == runtime_family_key,
            "binary_oracle_maker quote family does not match active runtime binding: market_key={market_key} runtime_family_key={runtime_family_key} input_family_key={input_family_key}"
        );
        Ok(ActiveQuoteAuthority {
            family_key: runtime_family_key.to_string(),
            underlying_asset: runtime_market.underlying_asset().to_string(),
            interval_start_ms: runtime_market.start_timestamp_milliseconds(),
            interval_end_ms: runtime_market.expiration_timestamp_milliseconds(),
        })
    }

    pub fn route_maker_order_command(
        &mut self,
        command: &MakerCompiledOrderCommand,
        submit_order_prefix: &str,
        gross_expected_value: Decimal,
    ) -> Result<MakerOrderDispatchOutcome> {
        let policy = self.context.order_execution_policy();
        let decision_evidence = self
            .context
            .order_execution_evidence()
            .expect("maker strategy must own order-intent evidence");
        let submit_admission = self.context.submit_admission_arc();
        let strategy_id = self.config.strategy_id.clone();
        let execution_client_id = self.config.client_id.clone();
        let order_economics = self.context.order_economics().clone();
        route_maker_order_command_through_policy(
            policy,
            self,
            &decision_evidence,
            submit_admission.as_ref(),
            BoltV3MakerOrderRoutingContext {
                strategy_id: strategy_id.as_str(),
                execution_client_id: execution_client_id.as_str(),
                order_economics: &order_economics,
                gross_expected_value,
            },
            MakerOrderDispatchInput {
                command,
                submit_order_prefix,
            },
        )
    }

    /// `decision` is absent when the cycle produced no quote set at all -- at the
    /// position cap, or with no fair value to quote against. A leg that was never
    /// evaluated for a throttle is not throttled, so absence clears exactly as an
    /// unblocked decision does. Gating this call on the quote set instead left the
    /// clear unreachable on those paths, and a leg that blocked, hit the cap, and
    /// blocked again recorded the second episode as a duplicate of the first and
    /// emitted nothing -- the mirror of the flooding this dedupe exists to stop.
    fn update_requote_throttle_edge(
        &mut self,
        market_key: &str,
        quote_state: &MarketQuote,
        budget: &RequoteBudgetPair,
        leg: Leg,
        decision: Option<&QuoteSetLegDecision>,
        now_ms: u64,
    ) -> Result<()> {
        let block =
            decision.and_then(|decision| requote_throttle_block(quote_state, leg, decision));
        let Some((action_cost_class, block_reason)) = block else {
            // The leg is no longer blocked: the episode is over, so forget it.
            // This is what keeps the accumulated set bounded, and it is what
            // lets a leg that becomes blocked again record that as new.
            if let Some(concrete_identity) = self
                .runtime
                .market(market_key)
                .map(|market| market.concrete_identity())
            {
                self.runtime
                    .clear_throttle_episode(market_key, &concrete_identity, leg);
            }
            return Ok(());
        };
        let Some((concrete_identity, family_key)) = self
            .runtime
            .market(market_key)
            .map(|market| (market.concrete_identity(), market.family_key().to_string()))
        else {
            anyhow::bail!(
                "binary_oracle_maker cannot record requote throttle for inactive market: market_key={market_key}"
            );
        };
        let market = ThrottledMarket {
            key: market_key,
            family_key: &family_key,
            concrete_identity: &concrete_identity,
        };
        let bound_by = requote_throttle_bound(action_cost_class, budget, now_ms);
        let episode = RequoteThrottleEpisodeId {
            market_key: market.key.to_string(),
            market: market.concrete_identity.clone(),
            leg,
            block_reason,
        };
        if self.runtime.contains_throttle_episode(&episode) {
            return Ok(());
        }
        let evidence = requote_throttle_observation(
            self.config.strategy_id.clone(),
            market,
            leg,
            now_ms,
            action_cost_class,
            block_reason,
            bound_by,
            budget,
        );
        if let ObservationRecordOutcome::FailureReported(error) = self
            .context
            .maker_evidence()
            .expect("maker strategy must own maker evidence")
            .record_requote_throttle_observation(evidence)
        {
            // A requote throttle is declining/limiting action (no new risk): a
            // telemetry-write failure must never abort the maker quote routing.
            // Surface the lost write at the highest non-panicking severity and
            // let the throttle path proceed.
            log::error!(
                "binary_oracle_maker requote throttle evidence write failed: strategy_id={} error={error:#}",
                self.config.strategy_id
            );
        }
        self.runtime.record_throttle_episode(episode);
        Ok(())
    }

    /// `market_key` selects the active runtime market this cycle is quoting.
    /// `MarketQuote` is leg lifecycle state and carries no key; the runtime binding
    /// supplies the authoritative family, cadence, instruments, and order handles.
    /// This route validates that authority before minting identities, then applies
    /// each dispatch outcome back to the same runtime binding.
    pub fn route_maker_runtime_quote(
        &mut self,
        market_key: &str,
        market: &mut MarketQuote,
        budget: &mut RequoteBudgetPair,
        input: BinaryOracleMakerRuntimeQuoteRouteInput<'_>,
    ) -> Result<BinaryOracleMakerRuntimeQuoteRouteOutcome> {
        let BinaryOracleMakerRuntimeQuoteRouteInput {
            quote_plan,
            quote_set,
            submit_template,
            price_precision,
            quantity_precision,
            submit_order_prefix,
        } = input;

        let authority = self.ensure_active_quote_authority(market_key, quote_plan.family_key)?;
        if !authority.window_contains(quote_set.now_ms) {
            return Ok(BinaryOracleMakerRuntimeQuoteRouteOutcome {
                quote: blocked_runtime_quote_decision(
                    MakerRuntimeQuoteBlockReason::RuntimeWindowUnavailable,
                ),
                orders: None,
            });
        }

        let order_id_tag = self.config.order_id_tag.clone();
        let minted = self.runtime.mint_next_identities(market_key, &order_id_tag);
        debug_assert!(minted, "active market must accept identity minting");
        let order_plan = self
            .runtime
            .market(market_key)
            .expect("market presence checked before identity mint")
            .order_plan_input();
        let now_ms = quote_set.now_ms;
        let quote_decision = plan_maker_runtime_quote(
            market,
            budget,
            MakerRuntimeQuoteInput {
                quote_plan,
                quote_set,
                order_plan,
            },
        );
        let planned = quote_decision.quote_set.as_ref();
        for (leg, decision) in [
            (Leg::Yes, planned.map(|set| &set.yes)),
            (Leg::No, planned.map(|set| &set.no)),
        ] {
            self.update_requote_throttle_edge(market_key, market, budget, leg, decision, now_ms)?;
        }
        let orders = if let Some(order_plan) = quote_decision.order_plan.as_ref() {
            let fair_probability_up = quote_decision
                .quote_plan
                .as_ref()
                .expect("an order plan requires a quote plan")
                .fair_probability_up;
            let mut route_command =
                |command: &MakerCompiledOrderCommand, submit_order_prefix: &str| {
                    let gross_expected_value =
                        maker_command_gross_expected_value(command, fair_probability_up)?;
                    self.route_maker_order_command(
                        command,
                        submit_order_prefix,
                        gross_expected_value,
                    )
                };
            Some(dispatch_maker_runtime_order_plan_with_command_router(
                MakerRuntimeOrderDispatchInput {
                    order_plan,
                    submit_template,
                    price_precision,
                    quantity_precision,
                    submit_order_prefix,
                },
                &mut route_command,
            )?)
        } else {
            None
        };
        if let Some(orders) = orders.as_ref() {
            self.runtime.apply_dispatch_outcome(market_key, orders);
        }

        Ok(BinaryOracleMakerRuntimeQuoteRouteOutcome {
            quote: quote_decision,
            orders,
        })
    }

    pub fn route_maker_runtime_reference_quote(
        &mut self,
        market_key: &str,
        market: &mut MarketQuote,
        budget: &mut RequoteBudgetPair,
        reference_selector: &mut ReferencePriceSelector,
        input: BinaryOracleMakerRuntimeReferenceQuoteRouteInput<'_>,
    ) -> Result<BinaryOracleMakerRuntimeReferenceQuoteRouteOutcome> {
        let BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
            reference_fair_value,
            quote_plan,
            quote_set,
            submit_template,
            price_precision,
            quantity_precision,
            submit_order_prefix,
        } = input;

        let authority = self.ensure_active_quote_authority(market_key, quote_plan.family_key)?;
        anyhow::ensure!(
            reference_selector.asset() == authority.underlying_asset,
            "binary_oracle_maker reference selector asset does not match active runtime binding: market_key={market_key} runtime_underlying_asset={} selector_asset={}",
            authority.underlying_asset,
            reference_selector.asset()
        );
        let strike_price = match reference_fair_value.strike {
            Some(strike) => {
                anyhow::ensure!(
                    strike.market_key() == market_key,
                    "binary_oracle_maker strike market does not match active runtime binding: market_key={market_key} strike_market_key={}",
                    strike.market_key()
                );
                anyhow::ensure!(
                    strike.underlying_asset() == authority.underlying_asset,
                    "binary_oracle_maker strike asset does not match active runtime binding: market_key={market_key} runtime_underlying_asset={} strike_underlying_asset={}",
                    authority.underlying_asset,
                    strike.underlying_asset()
                );
                anyhow::ensure!(
                    strike.interval_start_ms() == authority.interval_start_ms,
                    "binary_oracle_maker strike window does not match active runtime binding: market_key={market_key} runtime_interval_start_ms={} strike_interval_start_ms={}",
                    authority.interval_start_ms,
                    strike.interval_start_ms()
                );
                Some(strike.price())
            }
            None => None,
        };

        let fair_value = maker_reference_current_price_fair_value_decision(
            reference_selector,
            quote_set.now_ms,
            MakerRuntimeReferenceFairValueInput {
                family_key: &authority.family_key,
                interval_start_ms: authority.interval_start_ms,
                interval_end_ms: authority.interval_end_ms,
                reference_quotes: reference_fair_value.reference_quotes,
                strike_price,
                seconds_to_market_end: Some(
                    Duration::from_millis(
                        authority.interval_end_ms.saturating_sub(quote_set.now_ms),
                    )
                    .as_secs(),
                ),
                realized_volatility_snapshot: reference_fair_value.realized_volatility_snapshot,
                realized_volatility_max_source_age_ms: reference_fair_value
                    .realized_volatility_max_source_age_ms,
                pricing_kurtosis: reference_fair_value.pricing_kurtosis,
                evaluation_receive_ms: reference_fair_value.evaluation_receive_ms,
            },
        );
        let Some(reference_fair_value_result) = fair_value.fair_value.as_ref() else {
            // A missing reference price ends any prior throttle episode because
            // the legs were considered but not throttled. Window unavailability
            // is different: no quote cycle occurred, so it must not manufacture
            // an episode edge; cadence rollover or market retirement owns cleanup.
            if fair_value.blocked_by
                != Some(MakerRuntimeReferenceFairValueBlockReason::RuntimeWindowUnavailable)
            {
                for leg in [Leg::Yes, Leg::No] {
                    self.update_requote_throttle_edge(
                        market_key,
                        market,
                        budget,
                        leg,
                        None,
                        quote_set.now_ms,
                    )?;
                }
            }
            return Ok(BinaryOracleMakerRuntimeReferenceQuoteRouteOutcome {
                blocked_by: fair_value
                    .blocked_by
                    .map(BinaryOracleMakerRuntimeReferenceQuoteBlockReason::FairValue),
                fair_value,
                quote: None,
                orders: None,
            });
        };
        let oracle_fair_probability_up = reference_fair_value_result.fair_probability_up;
        let quote_route = self.route_maker_runtime_quote(
            market_key,
            market,
            budget,
            BinaryOracleMakerRuntimeQuoteRouteInput {
                quote_plan: MakerQuotePlanInputs {
                    family_key: &authority.family_key,
                    oracle_fair_probability_up,
                    ..quote_plan
                },
                quote_set,
                submit_template,
                price_precision,
                quantity_precision,
                submit_order_prefix,
            },
        )?;
        // A routing error is now per-leg data, not a `?` abort; fail loud here to
        // preserve this reference-quote route's prior fail-closed behavior.
        if let Some(error) = quote_route
            .orders
            .as_ref()
            .and_then(|orders| orders.routing_error())
        {
            anyhow::bail!(
                "binary_oracle_maker reference-quote leg order routing failed: error={error}"
            );
        }
        let BinaryOracleMakerRuntimeQuoteRouteOutcome { quote, orders } = quote_route;
        let blocked_by = quote
            .blocked_by
            .map(BinaryOracleMakerRuntimeReferenceQuoteBlockReason::Quote);

        Ok(BinaryOracleMakerRuntimeReferenceQuoteRouteOutcome {
            fair_value,
            quote: Some(quote),
            orders,
            blocked_by,
        })
    }

    pub fn route_maker_market_action(
        &mut self,
        input: BinaryOracleMakerMarketActionRouteInput<'_>,
    ) -> Result<BinaryOracleMakerMarketActionRouteOutcome> {
        let BinaryOracleMakerMarketActionRouteInput {
            action,
            submit_template,
            price_precision,
            quantity_precision,
            submit_order_prefix,
            gross_expected_value,
        } = input;

        let action_kind = action.action;
        let order_plan = maker_order_plan_from_market_action(action);

        let mut route_command = |command: &MakerCompiledOrderCommand, submit_order_prefix: &str| {
            self.route_maker_order_command(command, submit_order_prefix, gross_expected_value)
        };
        let orders = dispatch_maker_runtime_order_plan_with_command_router(
            MakerRuntimeOrderDispatchInput {
                order_plan: &order_plan,
                submit_template,
                price_precision,
                quantity_precision,
                submit_order_prefix,
            },
            &mut route_command,
        )?;
        // A routing error is now per-leg data, not a `?` abort; fail loud here to
        // preserve this market-action route's prior fail-closed behavior.
        if let Some(error) = orders.routing_error() {
            anyhow::bail!(
                "binary_oracle_maker market-action leg order routing failed: error={error}"
            );
        }
        let order = match action_kind {
            MarketAction::Leg { leg: Leg::No, .. }
            | MarketAction::CancelAllOneSide { leg: Leg::No } => orders.no.clone(),
            MarketAction::Leg { leg: Leg::Yes, .. }
            | MarketAction::CancelAllOneSide { leg: Leg::Yes }
            | MarketAction::CancelAllBothLegs => orders.yes.clone(),
        };

        Ok(BinaryOracleMakerMarketActionRouteOutcome { order, orders })
    }

    pub fn route_maker_loss_risk(
        &mut self,
        market: &mut MarketQuote,
        input: BinaryOracleMakerRiskRouteInput<'_>,
    ) -> Result<BinaryOracleMakerRiskRouteOutcome> {
        let BinaryOracleMakerRiskRouteInput {
            loss_decision,
            policy,
            targets,
            yes_quantity,
            no_quantity,
            yes,
            no,
            submit_template,
            price_precision,
            quantity_precision,
            submit_order_prefix,
        } = input;
        let mode = maker_risk_mode_for_loss_decision(&policy, loss_decision);
        let risk = apply_maker_risk_mode(market, mode);
        let orders = if let Some(action) = risk.action {
            Some(
                self.route_maker_market_action(BinaryOracleMakerMarketActionRouteInput {
                    action: MakerMarketActionOrderInput {
                        action,
                        targets,
                        yes_quantity,
                        no_quantity,
                        yes,
                        no,
                    },
                    submit_template,
                    price_precision,
                    quantity_precision,
                    submit_order_prefix,
                    gross_expected_value: Decimal::ZERO,
                })?
                .orders,
            )
        } else {
            None
        };

        Ok(BinaryOracleMakerRiskRouteOutcome { risk, orders })
    }
}

/// Project the maker's flat μ runtime knobs into the three config views
/// [`MakerMuState`] holds — the estimator warmup threshold, the health-gate
/// bounds, and the shared trade-flow retention. The single place that maps a μ
/// config field to its runtime view, so each knob is wired in exactly one home.
fn build_mu_state(config: &BinaryOracleMakerConfig) -> MakerMuState {
    MakerMuState::new(
        MuEstimatorConfig {
            min_classified_samples: config.mu_min_classified_samples,
        },
        MuHealthConfig {
            stale_window_ms: config.mu_stale_window_ms,
            mu_min_floor: config.mu_min_floor,
        },
        SignedTradeFlowConfig {
            window_secs: config.trade_flow_window_secs,
            max_samples: config.trade_flow_max_samples,
        },
    )
}

fn requote_throttle_block(
    market: &MarketQuote,
    leg: Leg,
    decision: &QuoteSetLegDecision,
) -> Option<(RequoteActionCostClass, RequoteThrottleBlockReason)> {
    if decision.control.blocked_by == Some(QuoteControlBlockReason::RequoteBudgetExhausted) {
        return requote_action_cost_class(market, leg)
            .map(|class| (class, RequoteThrottleBlockReason::RequoteBudgetExhausted));
    }
    None
}

fn requote_action_cost_class(market: &MarketQuote, leg: Leg) -> Option<RequoteActionCostClass> {
    match market.leg_state(leg) {
        LegState::Idle => Some(RequoteActionCostClass::FreshSubmit),
        LegState::Resting if market.leg_supports_modify(leg) => {
            Some(RequoteActionCostClass::Cancel)
        }
        LegState::Resting => Some(RequoteActionCostClass::CancelResubmit),
        LegState::SubmitPending
        | LegState::RequotePending
        | LegState::ModifyPending
        | LegState::CancelPending => None,
    }
}

fn requote_throttle_bound(
    action_cost_class: RequoteActionCostClass,
    budget: &RequoteBudgetPair,
    now_ms: u64,
) -> RequoteThrottleBound {
    let (submit_cost, rest_cost) = match action_cost_class {
        RequoteActionCostClass::FreshSubmit => (
            REQUOTE_THROTTLE_FRESH_SUBMIT_SUBMIT_COST,
            REQUOTE_THROTTLE_FRESH_SUBMIT_REST_COST,
        ),
        RequoteActionCostClass::CancelResubmit => (
            REQUOTE_THROTTLE_CANCEL_RESUBMIT_SUBMIT_COST,
            REQUOTE_THROTTLE_CANCEL_RESUBMIT_REST_COST,
        ),
        RequoteActionCostClass::Cancel => (
            REQUOTE_THROTTLE_CANCEL_SUBMIT_COST,
            REQUOTE_THROTTLE_CANCEL_REST_COST,
        ),
    };
    if let Some(last_emit_ms) = budget.last_emit_ms() {
        if now_ms < last_emit_ms {
            return RequoteThrottleBound::OutOfOrderTs;
        }
        // Mirror `RequoteBudget::try_acquire`: the min interval applies only to
        // distinct requote ticks, so same-millisecond emits fall through to the
        // sliding-window cap checks below.
        if now_ms > last_emit_ms && now_ms - last_emit_ms < budget.min_interval_ms() {
            return RequoteThrottleBound::MinInterval;
        }
    }
    if submit_cost > 0 {
        let Some(next_submit_cost) =
            (budget.submit_commands_in_window() as u64).checked_add(submit_cost)
        else {
            return RequoteThrottleBound::Overflow;
        };
        if next_submit_cost > budget.submit_command_cap() {
            return RequoteThrottleBound::SubmitCommandWindow;
        }
    }
    let Some(next_rest_cost) = budget.rest_cost_in_window().checked_add(rest_cost) else {
        return RequoteThrottleBound::Overflow;
    };
    if next_rest_cost > budget.rest_cap_per_window() {
        return RequoteThrottleBound::RestCallWindow;
    }
    RequoteThrottleBound::WindowCap
}

/// Inputs for one intent-only quote cycle on an active market. Bundles the
/// fair-value-resolved quote plan, the quote-set sizing/lifecycle inputs, and the
/// order-build context the dispatch needs. The leg `order_plan` is NOT part of
/// this input: [`BinaryOracleMaker::run_quote_cycle`] mints fresh leg order
/// identities and builds it from the runtime's active binding for the cycle.
#[derive(Debug, Clone, PartialEq)]
pub struct BinaryOracleMakerQuoteCycleInput<'a> {
    pub quote_plan: MakerQuotePlanInputs<'a>,
    pub quote_set: MakerRuntimeQuoteSetInput,
    pub submit_template: &'a NtOrderTemplate,
    pub price_precision: u8,
    pub quantity_precision: u8,
    pub submit_order_prefix: &'a str,
}

impl BinaryOracleMaker {
    /// The NautilusTrader timer name for the maker's autonomous quote/refresh loop.
    fn quote_timer_name(&self) -> String {
        format!("{}:quote_loop", self.config.strategy_id)
    }

    /// The execution-client id the maker subscribes its market data on, built from
    /// config for each subscribe/unsubscribe call.
    fn data_client_id(&self) -> ClientId {
        ClientId::from(self.config.client_id.as_str())
    }

    /// The shared portfolio policy derived from the operator's market-portfolio
    /// knobs.
    fn market_portfolio_policy(&self) -> MakerMarketPortfolioPolicy {
        MakerMarketPortfolioPolicy {
            max_active_markets: self.config.market_portfolio_max_active_markets as usize,
            total_bankroll_notional: self.config.market_portfolio_total_bankroll_notional,
            min_slot_notional: self.config.market_portfolio_min_slot_notional,
        }
    }

    /// Current wall-clock in milliseconds from the NautilusTrader clock.
    fn now_milliseconds(&mut self) -> u64 {
        self.clock().timestamp_ms()
    }

    /// The execution-venue-scoped instrument snapshot the resolver consumes. Mirrors
    /// the taker's venue-scoped cache read: a real maker order can only route to the
    /// execution client's venue, so any instrument on another venue must be
    /// unselectable here, and the read fails closed on a wrong-venue market.
    fn execution_venue_instruments(&mut self) -> Vec<InstrumentAny> {
        let execution_venue = self.context.execution_venue();
        let cache = self.cache();
        // Scope the cache read to the execution venue so NT filters before
        // materialization; the trailing `.filter` is retained as a defensive
        // assertion of the same fail-closed wrong-venue invariant.
        cache
            .instrument_ids(Some(&execution_venue))
            .into_iter()
            .filter_map(|instrument_id| cache.instrument(&instrument_id))
            .filter(|instrument| instrument.id().venue == execution_venue)
            .collect()
    }

    /// Re-resolve the declared market set against the current instrument snapshot,
    /// re-plan the active portfolio, and reconcile trade subscriptions to match the
    /// new active set. The `on_start` / `on_time_event` driver. INTENT ONLY: this
    /// subscribes to market data and tracks per-market runtime state; it never
    /// submits. Declared markets that do not resolve are logged (a fail-closed
    /// surface), never silently dropped.
    fn refresh_active_markets(&mut self) {
        let now_milliseconds = self.now_milliseconds();
        let instruments = self.execution_venue_instruments();
        let policy = self.market_portfolio_policy();
        let refresh = self.runtime.refresh_active_markets(
            &self.config.markets,
            &instruments,
            now_milliseconds,
            policy,
        );
        let client_id = self.data_client_id();
        for instrument_id in refresh.unsubscribe {
            self.unsubscribe_trades(instrument_id, Some(client_id), None);
        }
        for instrument_id in refresh.subscribe {
            self.subscribe_trades(instrument_id, Some(client_id), None);
        }
        for miss in &refresh.misses {
            log::warn!(
                "binary_oracle_maker declared market did not resolve: strategy_id={} miss={:?}",
                self.config.strategy_id,
                miss,
            );
        }
    }

    /// Register the autonomous quote/refresh timer (period = `quote_interval_ms`).
    /// Fails loud: a `quote_interval_ms` that overflows the nanosecond clock unit, or
    /// a timer registration error, aborts `on_start` rather than leaving the maker
    /// running with resolved markets but no quote/refresh cadence (silently never
    /// reconciling a cadence roll). This timer is the sole driver of the maker's
    /// market resolution and requote cadence, so — unlike the edge taker's
    /// best-effort selection-retry timer, which logs and continues — a registration
    /// failure here is propagated rather than swallowed.
    fn register_quote_timer(&mut self) -> anyhow::Result<()> {
        let timer_name = self.quote_timer_name();
        let strategy_id = self.config.strategy_id.clone();
        let quote_interval_ms = self.config.quote_interval_ms;
        let interval_nanoseconds = quote_interval_ms
            .checked_mul(NANOS_PER_MILLI_U64)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "binary_oracle_maker quote_interval_ms is invalid; it overflows the nanosecond clock unit: strategy_id={strategy_id} quote_interval_ms={quote_interval_ms}"
                )
            })?;
        self.context
            .order_economics()
            .validate_resting_refresh_cadence(interval_nanoseconds)
            .map_err(|error| {
                anyhow::anyhow!(
                    "binary_oracle_maker quote timer cannot safely refresh resting economics: strategy_id={strategy_id} error={error:#}"
                )
            })?;
        self.clock()
            .set_timer_ns(
                &timer_name,
                interval_nanoseconds,
                None,
                None,
                None,
                None,
                None,
            )
            .map_err(|error| {
                anyhow::anyhow!(
                    "binary_oracle_maker quote timer registration failed: strategy_id={strategy_id} error={error:#}"
                )
            })?;
        Ok(())
    }

    /// Cancel the autonomous quote/refresh timer.
    fn deregister_quote_timer(&mut self) {
        let timer_name = self.quote_timer_name();
        self.clock().cancel_timer(timer_name.as_str());
    }

    /// Run one intent-only quote cycle for an active market: mint fresh leg order
    /// identities, drive the existing quote/order pipeline with the caller-supplied
    /// fair-value-resolved inputs, and reconcile the dispatched leg identities.
    /// Returns `None` only when the active market's cadence window is not currently
    /// quotable. Inactive market keys and authority mismatches remain errors from
    /// the shared route. INTENT ONLY: the dispatch routes through the global
    /// execution-policy chokepoint, which suppresses every venue mutation in
    /// shadow. The shared route validates the supplied family against the active
    /// binding and checks window availability before any identity mint.
    ///
    /// The fair-value + quote-math inputs are caller-supplied because the
    /// reference/realized-volatility feed that resolves them lands in a later slice
    /// (X2); until then the autonomous loop has no fair value to drive this, so it
    /// is exercised by the differential tests rather than the live timer.
    pub fn run_quote_cycle(
        &mut self,
        market_key: &str,
        market: &mut MarketQuote,
        budget: &mut RequoteBudgetPair,
        input: BinaryOracleMakerQuoteCycleInput<'_>,
    ) -> Result<Option<BinaryOracleMakerRuntimeQuoteRouteOutcome>> {
        let BinaryOracleMakerQuoteCycleInput {
            quote_plan,
            quote_set,
            submit_template,
            price_precision,
            quantity_precision,
            submit_order_prefix,
        } = input;
        let outcome = self.route_maker_runtime_quote(
            market_key,
            market,
            budget,
            BinaryOracleMakerRuntimeQuoteRouteInput {
                quote_plan,
                quote_set,
                submit_template,
                price_precision,
                quantity_precision,
                submit_order_prefix,
            },
        )?;
        if outcome.quote.blocked_by == Some(MakerRuntimeQuoteBlockReason::RuntimeWindowUnavailable)
        {
            return Ok(None);
        }
        if let Some(orders) = outcome.orders.as_ref() {
            // `route_maker_runtime_quote` reconciles whichever legs dispatched before
            // this fail-loud check, so a partial two-leg dispatch never orphans the
            // sibling leg's assigned identity from the runtime's view. The MarketQuote
            // FSM and requote budget remain advanced consistently with that outcome.
            if let Some(error) = orders.routing_error() {
                anyhow::bail!(
                    "binary_oracle_maker leg order routing failed after identity reconcile: market_key={market_key} error={error}"
                );
            }
        }
        Ok(Some(outcome))
    }
}

// The maker drives an autonomous, INTENT-ONLY runtime: `on_start` resolves the
// declared markets against the instrument cache, subscribes their trade feeds, and
// registers the quote timer; `on_time_event` re-resolves on each tick (following
// cadence-window rollover) and reconciles subscriptions; `on_stop` tears both down;
// `on_trade` feeds the per-instrument μ buffer. Order routing stays on the shadow
// chokepoint — nothing here submits to a venue.
impl DataActor for BinaryOracleMaker {
    fn on_start(&mut self) -> anyhow::Result<()> {
        // Register the quote timer first: it is the only fallible step here, so a
        // registration failure aborts on_start before refresh_active_markets emits
        // any subscription side effects, leaving no half-started runtime behind.
        self.register_quote_timer()?;
        self.refresh_active_markets();
        Ok(())
    }

    fn on_stop(&mut self) -> anyhow::Result<()> {
        self.deregister_quote_timer();
        let client_id = self.data_client_id();
        for instrument_id in self.runtime.active_instrument_ids() {
            self.unsubscribe_trades(instrument_id, Some(client_id), None);
        }
        // Deactivate every market so a restart re-resolves from an empty active set
        // and re-emits the full trade-subscription delta (leaving the active markets
        // here would make the next `on_start`'s refresh diff before == after and emit
        // no subscribe delta, leaving the maker active with no trade feeds). Use
        // `deactivate_all` rather than replacing the runtime with `empty()` so the
        // per-(market_key, leg) generation high-water survives a within-process
        // stop/start: a re-mint after restart cannot reproduce a `ClientOrderId` a
        // prior run consumed. (Cross-process restart durability needs a persisted
        // high-water — arming-time work, #869.)
        self.runtime.deactivate_all();
        let order_economics = self.context.order_economics().clone();
        let execution_policy = self.context.order_execution_policy();
        let execution_client_id = self.config.client_id.clone();
        order_economics.stop_resting_order_economics(
            execution_policy,
            self,
            execution_client_id.as_str(),
        )?;
        Ok(())
    }

    fn on_trade(&mut self, trade: &TradeTick) -> anyhow::Result<()> {
        self.mu.observe(trade);
        Ok(())
    }

    fn on_time_event(&mut self, event: &TimeEvent) -> anyhow::Result<()> {
        if event.name.as_str() == self.quote_timer_name() {
            let order_economics = self.context.order_economics().clone();
            let execution_policy = self.context.order_execution_policy();
            let execution_client_id = self.config.client_id.clone();
            let now_ms = self.now_milliseconds();
            order_economics.drive_resting_order_economics_at_ms(
                execution_policy,
                self,
                execution_client_id.as_str(),
                now_ms,
            )?;
            self.refresh_active_markets();
        }
        Ok(())
    }
}

nautilus_trading::nautilus_strategy!(BinaryOracleMaker, {});

impl StrategyBuilder for BinaryOracleMakerBuilder {
    type Strategy = BinaryOracleMaker;

    fn kind() -> &'static str {
        KEY
    }

    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
        validate_config(raw, field_prefix, errors);
    }

    fn build_typed(raw: &Value, context: &StrategyBuildContext) -> Result<Self::Strategy> {
        Ok(BinaryOracleMaker::new(parse_config(raw)?, context.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bolt_v3_current_evidence::{
            DecisionEvidenceRecorder, MakerEvidence, OrderExecutionEvidence,
        },
        bolt_v3_maker_mu_estimator::{MuHealthReason, UsableMu},
        bolt_v3_market_families::OutcomeSide,
        bolt_v3_numeric::NANOS_PER_MILLI_U64,
        bolt_v3_order_execution::BoltV3OrderExecutionPolicy,
        bolt_v3_position_contract::BoltV3PositionMarketLifecycle,
        bolt_v3_strategy_context::StrategyDecisionEvidence,
        bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
    };
    use nautilus_core::{Params, UnixNanos};
    use nautilus_model::{
        enums::{AggressorSide, AssetClass},
        identifiers::{InstrumentId, Symbol, TradeId, Venue},
        instruments::{BinaryOption, InstrumentAny},
        types::{Currency, Price, Quantity},
    };
    use std::sync::Arc;

    #[test]
    fn builder_kind_is_archetype_key() {
        assert_eq!(BinaryOracleMakerBuilder::kind(), "binary_oracle_maker");
        assert_eq!(BinaryOracleMakerBuilder::kind(), KEY);
    }

    #[test]
    fn maker_can_construct_shared_position_market_lifecycle_from_updown_instrument() {
        let lifecycle =
            BoltV3PositionMarketLifecycle::recover_from_instrument(Some(&maker_binary_option(
                "maker-condition-DOWN.POLYMARKET",
                "configuredasset-updown-5m-600",
                "maker-market-1",
                "maker-condition-1",
                "maker-question-1",
                "Down",
                600_000,
                900_000,
            )));

        assert_eq!(lifecycle.market_id(), Some("maker-market-1"));
        assert_eq!(lifecycle.outcome_side(), Some(OutcomeSide::Down));
        assert_eq!(lifecycle.interval_end_ms(), Some(900_000));
        assert!(lifecycle.matches_resolution_tick_ms(900_000));
    }

    const QUERY_NOW_MS: u64 = 50_000;
    const TEST_STALE_WINDOW_MS: u64 = 60_000;
    const TEST_MU_FLOOR: f64 = 0.05;
    const TEST_REQUOTE_MIN_INTERVAL_MS: u64 = 500;
    const TEST_QUOTE_INTERVAL_MS: u64 = 1_000;

    #[allow(clippy::too_many_arguments)]
    fn maker_binary_option(
        instrument_id: &str,
        market_slug: &str,
        market_id: &str,
        condition_id: &str,
        question_id: &str,
        outcome: &str,
        activation_ms: u64,
        expiration_ms: u64,
    ) -> InstrumentAny {
        let mut info = Params::new();
        info.insert(
            "market_slug".to_string(),
            serde_json::Value::String(market_slug.to_string()),
        );
        info.insert(
            "market_id".to_string(),
            serde_json::Value::String(market_id.to_string()),
        );
        info.insert(
            "condition_id".to_string(),
            serde_json::Value::String(condition_id.to_string()),
        );
        info.insert(
            "question_id".to_string(),
            serde_json::Value::String(question_id.to_string()),
        );
        InstrumentAny::BinaryOption(BinaryOption::new(
            InstrumentId::from(instrument_id),
            Symbol::from(instrument_id.split('.').next().unwrap_or(instrument_id)),
            AssetClass::Alternative,
            Currency::USDC(),
            (activation_ms.saturating_mul(NANOS_PER_MILLI_U64)).into(),
            (expiration_ms.saturating_mul(NANOS_PER_MILLI_U64)).into(),
            3,
            2,
            Price::from("0.001"),
            Quantity::from("0.01"),
            Some(ustr::Ustr::from(outcome)),
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
            Some(info),
            1.into(),
            1.into(),
        ))
    }

    fn test_context() -> StrategyBuildContext {
        let writer = Arc::new(DecisionEvidenceRecorder::recording());
        let evidence = StrategyDecisionEvidence::maker(
            MakerEvidence::new(&writer),
            OrderExecutionEvidence::new(&writer),
        );
        StrategyBuildContext::new(
            crate::bolt_v3_economics_test_support::fixture_order_economics(),
            evidence,
            Arc::new(BoltV3SubmitAdmissionState::new(writer)),
            BoltV3OrderExecutionPolicy::shadow(),
            Venue::from("MAKER.TEST"),
        )
    }

    fn maker_config(
        trade_flow_window_secs: u64,
        trade_flow_max_samples: u64,
        mu_min_classified_samples: u64,
    ) -> BinaryOracleMakerConfig {
        BinaryOracleMakerConfig {
            strategy_id: "binary_oracle_maker-001".to_string(),
            order_id_tag: "001".to_string(),
            oms_type: "netting".to_string(),
            client_id: "maker_execution_client".to_string(),
            trade_flow_window_secs,
            trade_flow_max_samples,
            mu_min_classified_samples,
            mu_stale_window_ms: TEST_STALE_WINDOW_MS,
            mu_min_floor: TEST_MU_FLOOR,
            requote_min_interval_ms: TEST_REQUOTE_MIN_INTERVAL_MS,
            quote_interval_ms: TEST_QUOTE_INTERVAL_MS,
            market_portfolio_max_active_markets: 3,
            market_portfolio_total_bankroll_notional: 1500.0,
            market_portfolio_min_slot_notional: 100.0,
            markets_config_digest:
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            // The μ-focused unit tests here drive `on_trade` only; market resolution
            // is exercised in the integration suite, so an empty declared set is the
            // right fixture (the non-empty bounds gate is an archetype concern).
            markets: Vec::new(),
        }
    }

    // Observe one trade per `side` at ts 1000, 2000, ... ms (so the newest is ~46s
    // before QUERY_NOW_MS), routing through the maker's own `MakerMuState`.
    fn observe_sides(state: &mut MakerMuState, instrument: InstrumentId, sides: &[AggressorSide]) {
        for (index, side) in sides.iter().enumerate() {
            let ts_ns = (1_000 + index as u64 * 1_000) * NANOS_PER_MILLI_U64;
            let trade = TradeTick::new_checked(
                instrument,
                Price::new(0.5, 2),
                Quantity::new(1.0, 0),
                *side,
                TradeId::from(format!("T{ts_ns}").as_str()),
                UnixNanos::from(ts_ns),
                UnixNanos::from(ts_ns),
            )
            .expect("valid trade tick");
            state.observe(&trade);
        }
    }

    #[test]
    fn build_mu_state_projects_warmup_and_stale_knobs() {
        // Pins the warmup (`mu_min_classified_samples`) and stale-window mappings
        // through the public gate read: a one-sided buy flow of exactly the warmup
        // count (newest ~46s before `now`) warms μ to 1.0 and clears the gate, so
        // `usable_mu_for` is `Ok(1.0)`. A mismapped warmup would starve μ to
        // `Err(Absent)`; a stale window mismapped below ~46s would flip it to
        // `Err(Stale)`. `window_secs`/`max_samples` are non-binding here — they are
        // pinned by the two tests below. The Stale and BelowFloor branches
        // themselves are exercised directly in `mu`'s own tests.
        let mut state = build_mu_state(&maker_config(600, 1000, 4));
        let instrument = InstrumentId::from("MAKER.SIM");
        observe_sides(&mut state, instrument, &[AggressorSide::Buyer; 4]);
        assert_eq!(
            state
                .usable_mu_for(&instrument, QUERY_NOW_MS)
                .map(UsableMu::get),
            Ok(1.0)
        );
    }

    #[test]
    fn build_mu_state_maps_trade_flow_window_secs() {
        // Pins `window_secs`: a 5s retention window ages out trades observed ~46s
        // ago, so `samples_within` is empty and μ is None. Were `build_mu_state`
        // to read the window from `trade_flow_max_samples` (a field swap), the
        // window would be 1000s, the trades would be retained, and the gate would
        // return `Ok(1.0)` — flipping this assertion. An empty in-window view makes
        // both μ and the staleness anchor absent, so the gate fails closed Absent.
        let mut state = build_mu_state(&maker_config(5, 1000, 1));
        let instrument = InstrumentId::from("MAKER.SIM");
        observe_sides(&mut state, instrument, &[AggressorSide::Buyer; 4]);
        assert_eq!(
            state.usable_mu_for(&instrument, QUERY_NOW_MS),
            Err(MuHealthReason::Absent)
        );
    }

    #[test]
    fn build_mu_state_maps_trade_flow_max_samples() {
        // Pins `max_samples`: a cap of 2 retains only the last two (buy) trades of
        // a sell,sell,buy,buy flow, so μ = 1.0. Were `build_mu_state` to read the
        // cap from `trade_flow_window_secs` (a field swap → cap 600), the buffer
        // would keep all four trades, balancing to μ = 0.0 and the gate to
        // `Err(BelowFloor)` — flipping this assertion. The wide 600s window keeps
        // all retained trades in-window so only the cap, not staleness, is exercised.
        let mut state = build_mu_state(&maker_config(600, 2, 2));
        let instrument = InstrumentId::from("MAKER.SIM");
        observe_sides(
            &mut state,
            instrument,
            &[
                AggressorSide::Seller,
                AggressorSide::Seller,
                AggressorSide::Buyer,
                AggressorSide::Buyer,
            ],
        );
        assert_eq!(
            state
                .usable_mu_for(&instrument, QUERY_NOW_MS)
                .map(UsableMu::get),
            Ok(1.0)
        );
    }

    #[test]
    fn on_trade_feeds_the_mu_buffer() {
        // Differential guard for the real `DataActor::on_trade` handler (not just
        // `MakerMuState::observe`): a no-op `on_trade` would leave the buffer empty
        // and the gate `Err(Absent)`, so the post-flow `Ok(1.0)` assertion fails on
        // that buggy variant. Asserts through the μ side-effect channel the handler
        // is supposed to drive.
        let mut maker = BinaryOracleMaker::new(maker_config(600, 1000, 4), test_context());
        let instrument = InstrumentId::from("MAKER.SIM");
        assert_eq!(
            maker.mu.usable_mu_for(&instrument, QUERY_NOW_MS),
            Err(MuHealthReason::Absent),
            "no trade observed yet must fail closed"
        );
        for index in 0..4u64 {
            let ts_ns = (1_000 + index * 1_000) * NANOS_PER_MILLI_U64;
            let tick = TradeTick::new_checked(
                instrument,
                Price::new(0.5, 2),
                Quantity::new(1.0, 0),
                AggressorSide::Buyer,
                TradeId::from(format!("T{ts_ns}").as_str()),
                UnixNanos::from(ts_ns),
                UnixNanos::from(ts_ns),
            )
            .expect("valid trade tick");
            maker
                .on_trade(&tick)
                .expect("maker on_trade should process");
        }
        assert_eq!(
            maker
                .mu
                .usable_mu_for(&instrument, QUERY_NOW_MS)
                .map(UsableMu::get),
            Ok(1.0),
            "on_trade must route each tick into the per-instrument μ buffer"
        );
    }

    #[test]
    fn requote_throttle_bound_throttles_same_millisecond_reemit() {
        use crate::bolt_v3_requote_budget::RequoteBudget;

        let mut remaining_budget = RequoteBudgetPair::new(
            RequoteBudget::new(40, 60_000, TEST_REQUOTE_MIN_INTERVAL_MS),
            RequoteBudget::new(100, 60_000, TEST_REQUOTE_MIN_INTERVAL_MS),
        );
        assert!(remaining_budget.try_reserve_fresh_submit(1_000));
        assert_eq!(remaining_budget.last_emit_ms(), Some(1_000));

        // Same-millisecond emits are exempt from the min-interval floor and must
        // fall through to the sliding-window classifier. With budget remaining,
        // that is the admitted-by-bound WindowCap sentinel, not MinInterval.
        assert_eq!(
            requote_throttle_bound(
                RequoteActionCostClass::CancelResubmit,
                &remaining_budget,
                1_000,
            ),
            RequoteThrottleBound::WindowCap,
            "same-millisecond emits with budget remaining must not be labeled MinInterval"
        );

        let mut exhausted_budget = RequoteBudgetPair::new(
            RequoteBudget::new(1, 60_000, TEST_REQUOTE_MIN_INTERVAL_MS),
            RequoteBudget::new(100, 60_000, TEST_REQUOTE_MIN_INTERVAL_MS),
        );
        assert!(exhausted_budget.try_reserve_fresh_submit(1_000));
        assert_eq!(exhausted_budget.last_emit_ms(), Some(1_000));

        // Same millisecond, exhausted submit-command budget: the live gate would
        // refuse on the window cap, so the evidence must name that cap.
        assert_eq!(
            requote_throttle_bound(
                RequoteActionCostClass::CancelResubmit,
                &exhausted_budget,
                1_000,
            ),
            RequoteThrottleBound::SubmitCommandWindow,
            "same-millisecond budget exhaustion must be labeled by the window cap"
        );

        assert_eq!(
            requote_throttle_bound(
                RequoteActionCostClass::CancelResubmit,
                &exhausted_budget,
                500,
            ),
            RequoteThrottleBound::OutOfOrderTs,
            "an earlier observation must expose the diagnostic bound oscillation"
        );

        // A strictly later tick inside the interval still matches the gate's
        // anti-flicker floor and is classified as MinInterval.
        assert_eq!(
            requote_throttle_bound(
                RequoteActionCostClass::CancelResubmit,
                &remaining_budget,
                1_001,
            ),
            RequoteThrottleBound::MinInterval,
            "strictly-later ticks inside the interval remain bounded by MinInterval"
        );
    }
}
