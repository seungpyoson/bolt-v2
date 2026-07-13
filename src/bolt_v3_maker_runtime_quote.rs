//! Shared maker runtime quote orchestrator.
//!
//! This is the thin runtime bridge over the pure maker pipeline:
//! quote-plan inputs resolve family quote targets, those targets drive the
//! quote-set lifecycle controller, and approved lifecycle actions map to maker
//! order intents. It owns no market-family selection, no venue facts, no clocks,
//! and no config defaults; callers supply already-resolved runtime inputs.

use crate::{
    bolt_v3_fair_value_pricing::{
        FairValuePricingBlockReason, FairValuePricingConfig, FairValuePricingRequest,
        FairValuePricingState, FastSpotObservation,
    },
    bolt_v3_maker_order_plan::{
        MakerLegBinding, MakerOrderPlan, MakerOrderPlanInput, maker_order_intents_from_quote_set,
    },
    bolt_v3_maker_quote_plan::{MakerQuotePlan, MakerQuotePlanInputs, plan_maker_quote_targets},
    bolt_v3_maker_quote_set::{QuoteSetDecision, QuoteSetInput, drive_binary_quote_set},
    bolt_v3_maker_reservation::BuyCommitment,
    bolt_v3_quote_lifecycle::MarketQuote,
    bolt_v3_realized_volatility::RealizedVolSnapshot,
    bolt_v3_reference_price::{ReferencePriceSelector, ReferenceQuote},
    bolt_v3_requote_budget::RequoteBudgetPair,
    bolt_v3_timestamp_domain::{LocalReceiveMs, VenueEventMs},
};

#[derive(Debug, Clone, PartialEq)]
pub struct MakerRuntimeQuoteInput<'a> {
    pub quote_plan: MakerQuotePlanInputs<'a>,
    pub quote_set: MakerRuntimeQuoteSetInput<'a>,
    pub order_plan: MakerRuntimeOrderPlanInput,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerRuntimeQuoteSetInput<'a> {
    pub yes_quantity: f64,
    pub no_quantity: f64,
    pub yes_resting_price: Option<f64>,
    pub no_resting_price: Option<f64>,
    pub open_commitments: &'a [BuyCommitment],
    pub max_fee_bps: f64,
    pub available_collateral: f64,
    pub requote_threshold: f64,
    pub eps: f64,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerRuntimeOrderPlanInput {
    pub yes: MakerLegBinding,
    pub no: MakerLegBinding,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerRuntimeReferenceFairValueInput<'a> {
    pub family_key: &'a str,
    pub interval_start_ms: u64,
    pub interval_end_ms: u64,
    pub reference_quotes: &'a [ReferenceQuote],
    pub strike_price: Option<f64>,
    pub seconds_to_market_end: Option<u64>,
    pub realized_volatility_snapshot: &'a RealizedVolSnapshot,
    pub realized_volatility_max_source_age_ms: Option<u64>,
    pub pricing_kurtosis: f64,
    pub evaluation_receive_ms: LocalReceiveMs,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerRuntimeReferenceFairValue {
    pub source_id: String,
    pub reference_current_price_source_id: String,
    pub reference_current_price: f64,
    pub reference_current_price_observed_ts_ms: u64,
    pub failed_over: bool,
    pub reference_current_price_failed_over: bool,
    pub spot_price: f64,
    pub strike_price: f64,
    pub seconds_to_market_end: u64,
    pub realized_vol: f64,
    pub realized_vol_surface_id: Option<String>,
    pub realized_vol_source_venue: Option<String>,
    pub realized_vol_source_ts_ms: Option<u64>,
    pub pricing_kurtosis: f64,
    pub fair_probability_up: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerRuntimeReferenceFairValueDecision {
    pub fair_value: Option<MakerRuntimeReferenceFairValue>,
    pub blocked_by: Option<MakerRuntimeReferenceFairValueBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerRuntimeReferenceFairValueBlockReason {
    ReferenceCurrentPriceUnavailable,
    SpotPriceMissing,
    StrikePriceMissing,
    SecondsToExpiryMissing,
    RealizedVolNotReady,
    FairProbabilityUnavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerRuntimeQuoteDecision {
    pub quote_plan: Option<MakerQuotePlan>,
    pub quote_set: Option<QuoteSetDecision>,
    pub order_plan: Option<MakerOrderPlan>,
    pub blocked_by: Option<MakerRuntimeQuoteBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerRuntimeQuoteBlockReason {
    QuotePlanUnavailable,
}

pub fn maker_reference_current_price_fair_value(
    selector: &mut ReferencePriceSelector,
    now_ms: u64,
    input: MakerRuntimeReferenceFairValueInput<'_>,
) -> Option<MakerRuntimeReferenceFairValue> {
    maker_reference_current_price_fair_value_decision(selector, now_ms, input).fair_value
}

pub fn maker_reference_current_price_fair_value_decision(
    selector: &mut ReferencePriceSelector,
    now_ms: u64,
    input: MakerRuntimeReferenceFairValueInput<'_>,
) -> MakerRuntimeReferenceFairValueDecision {
    let Some(selection) = selector.select(
        input.interval_start_ms,
        input.interval_end_ms,
        now_ms,
        input.reference_quotes,
    ) else {
        return fair_value_blocked(
            MakerRuntimeReferenceFairValueBlockReason::ReferenceCurrentPriceUnavailable,
        );
    };

    let source_id = selection.source_id().to_string();
    let Some(selected_quote) =
        selected_reference_quote_for_selection(&input, selection.source_id())
    else {
        return fair_value_blocked(
            MakerRuntimeReferenceFairValueBlockReason::ReferenceCurrentPriceUnavailable,
        );
    };
    let reference_observation = FastSpotObservation {
        venue: source_id.clone(),
        price: selection.price(),
        observed_ts_ms: selected_quote.observed_ts_ms(),
        received_ts_ms: Some(selected_quote.received_ts_ms()),
    };
    let mut pricing = FairValuePricingState::from_realized_volatility_surface_id(
        input.realized_volatility_snapshot.surface_id.clone(),
    );
    pricing.observe_reference_current_price(&reference_observation);
    pricing.observe_pricing_spot(&reference_observation);
    pricing.observe_realized_vol_snapshot((*input.realized_volatility_snapshot).clone());
    let config = FairValuePricingConfig {
        realized_volatility_surface_id: input.realized_volatility_snapshot.surface_id.as_str(),
        realized_volatility_max_source_age_ms: input.realized_volatility_max_source_age_ms,
        pricing_kurtosis: input.pricing_kurtosis,
        market_family: input.family_key,
    };
    let request = FairValuePricingRequest {
        now_ms,
        realized_vol_gate_receive_ms: Some(input.evaluation_receive_ms),
        strike_price: input.strike_price,
        seconds_to_market_end: input.seconds_to_market_end,
    };
    let pricing_result = match pricing.fair_value_pricing_at(&config, request) {
        Ok(pricing_result) => pricing_result,
        Err(blocked_by) => {
            let reason = blocked_by
                .into_iter()
                .map(maker_fair_value_block_reason_from_shared)
                .next()
                .unwrap_or(MakerRuntimeReferenceFairValueBlockReason::FairProbabilityUnavailable);
            return fair_value_blocked(reason);
        }
    };

    let failed_over = selection.failed_over();
    MakerRuntimeReferenceFairValueDecision {
        fair_value: Some(MakerRuntimeReferenceFairValue {
            source_id: source_id.clone(),
            reference_current_price_source_id: source_id,
            reference_current_price: pricing_result.spot_price,
            reference_current_price_observed_ts_ms: selected_quote.observed_ts_ms(),
            failed_over,
            reference_current_price_failed_over: failed_over,
            spot_price: pricing_result.spot_price,
            strike_price: pricing_result.strike_price,
            seconds_to_market_end: pricing_result.seconds_to_market_end,
            realized_vol: pricing_result.realized_vol,
            realized_vol_surface_id: pricing_result.realized_vol_surface_id,
            realized_vol_source_venue: pricing_result.realized_vol_source_venue,
            realized_vol_source_ts_ms: pricing_result.realized_vol_source_ts_ms,
            pricing_kurtosis: input.pricing_kurtosis,
            fair_probability_up: pricing_result.fair_probability_up,
        }),
        blocked_by: None,
    }
}

fn selected_reference_quote_for_selection<'a>(
    input: &MakerRuntimeReferenceFairValueInput<'a>,
    source_id: &str,
) -> Option<&'a ReferenceQuote> {
    input
        .reference_quotes
        .iter()
        .filter(|quote| {
            let observed_ts_ms = VenueEventMs::new(quote.observed_ts_ms());
            quote.source_id() == source_id
                && observed_ts_ms >= VenueEventMs::new(input.interval_start_ms)
                && observed_ts_ms <= VenueEventMs::new(input.interval_end_ms)
        })
        .max_by_key(|quote| quote.observed_ts_ms())
}

fn maker_fair_value_block_reason_from_shared(
    reason: FairValuePricingBlockReason,
) -> MakerRuntimeReferenceFairValueBlockReason {
    match reason {
        FairValuePricingBlockReason::SpotPriceMissing => {
            MakerRuntimeReferenceFairValueBlockReason::SpotPriceMissing
        }
        FairValuePricingBlockReason::StrikePriceMissing => {
            MakerRuntimeReferenceFairValueBlockReason::StrikePriceMissing
        }
        FairValuePricingBlockReason::SecondsToExpiryMissing => {
            MakerRuntimeReferenceFairValueBlockReason::SecondsToExpiryMissing
        }
        FairValuePricingBlockReason::RealizedVolNotReady => {
            MakerRuntimeReferenceFairValueBlockReason::RealizedVolNotReady
        }
        FairValuePricingBlockReason::FairProbabilityUnavailable => {
            MakerRuntimeReferenceFairValueBlockReason::FairProbabilityUnavailable
        }
    }
}

fn fair_value_blocked(
    reason: MakerRuntimeReferenceFairValueBlockReason,
) -> MakerRuntimeReferenceFairValueDecision {
    MakerRuntimeReferenceFairValueDecision {
        fair_value: None,
        blocked_by: Some(reason),
    }
}

pub fn plan_maker_runtime_quote(
    market: &mut MarketQuote,
    budget: &mut RequoteBudgetPair,
    input: MakerRuntimeQuoteInput<'_>,
) -> MakerRuntimeQuoteDecision {
    let Some(quote_plan) = plan_maker_quote_targets(input.quote_plan) else {
        return blocked(MakerRuntimeQuoteBlockReason::QuotePlanUnavailable);
    };

    let quote_set = drive_binary_quote_set(
        market,
        budget,
        QuoteSetInput {
            targets: quote_plan.targets,
            yes_quantity: input.quote_set.yes_quantity,
            no_quantity: input.quote_set.no_quantity,
            yes_resting_price: input.quote_set.yes_resting_price,
            no_resting_price: input.quote_set.no_resting_price,
            open_commitments: input.quote_set.open_commitments,
            max_fee_bps: input.quote_set.max_fee_bps,
            available_collateral: input.quote_set.available_collateral,
            requote_threshold: input.quote_set.requote_threshold,
            eps: input.quote_set.eps,
            now_ms: input.quote_set.now_ms,
        },
    );
    let order_plan = maker_order_intents_from_quote_set(MakerOrderPlanInput {
        quote_set: &quote_set,
        targets: quote_plan.targets,
        yes_quantity: input.quote_set.yes_quantity,
        no_quantity: input.quote_set.no_quantity,
        yes: input.order_plan.yes,
        no: input.order_plan.no,
    });

    MakerRuntimeQuoteDecision {
        quote_plan: Some(quote_plan),
        quote_set: Some(quote_set),
        order_plan: Some(order_plan),
        blocked_by: None,
    }
}

fn blocked(reason: MakerRuntimeQuoteBlockReason) -> MakerRuntimeQuoteDecision {
    MakerRuntimeQuoteDecision {
        quote_plan: None,
        quote_set: None,
        order_plan: None,
        blocked_by: Some(reason),
    }
}
