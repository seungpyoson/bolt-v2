//! Shared taker pricing state extracted from `binary_oracle_edge_taker`.
//!
//! This module mirrors the current RV-based taker pricing path: reference and
//! lead-venue observations warm realized volatility, then current spot/strike/
//! expiry/config are assembled into the existing market-family fair probability.
//! It deliberately does not introduce IV, maker spread logic, or submit policy.

use std::collections::BTreeMap;

use crate::{
    bolt_v3_market_families::{self, FairProbabilityInputs},
    bolt_v3_numeric::{
        MILLIS_PER_SECOND_U64, UNIT_F64, ZERO_F64, clamp_probability, is_positive_finite,
        sanitize_probability,
    },
    bolt_v3_taker_signal::{
        ThetaScalerInputs, compute_theta_scaler, price_agreement_corr, price_gap_probability,
    },
    bolt_v3_volatility::{RealizedVolConfig, RealizedVolEstimator},
};

const INITIAL_COUNTER_U64: u64 = u64::MIN;

#[derive(Debug, Clone, PartialEq)]
pub struct FastSpotObservation {
    pub venue: String,
    pub price: f64,
    pub observed_ts_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TakerPricingConfig<'a> {
    pub realized_vol: RealizedVolConfig,
    pub lead_agreement_min_corr: f64,
    pub lead_jitter_max_ms: u64,
    pub spike_guard_return_threshold: f64,
    pub spike_guard_cooldown_secs: u64,
    pub cadence_seconds: u64,
    pub theta_decay_factor: f64,
    pub edge_threshold_basis_points: i64,
    pub pricing_kurtosis: f64,
    pub rotating_market_family: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TakerPricingRequest {
    pub now_ms: u64,
    pub strike_price: Option<f64>,
    pub seconds_to_market_end: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TakerPricingResult {
    pub spot_price: f64,
    pub strike_price: f64,
    pub seconds_to_market_end: u64,
    pub realized_vol: f64,
    pub realized_vol_source_venue: Option<String>,
    pub realized_vol_source_ts_ms: Option<u64>,
    pub theta_scaled_min_edge_bps: f64,
    pub fair_probability_up: f64,
    pub fair_probability_down: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TakerPricingInputs {
    pub spot_price: f64,
    pub strike_price: f64,
    pub seconds_to_market_end: u64,
    pub realized_vol: f64,
    pub theta_scaled_min_edge_bps: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakerPricingBlockReason {
    SpotPriceMissing,
    StrikePriceMissing,
    SecondsToExpiryMissing,
    RealizedVolNotReady,
    ThetaScalerUnavailable,
    FairProbabilityUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VenueTimingState {
    pub(crate) last_observed_ts_ms: Option<u64>,
    pub(crate) last_interval_ms: Option<u64>,
}

impl VenueTimingState {
    pub(crate) fn empty() -> Self {
        Self {
            last_observed_ts_ms: None,
            last_interval_ms: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TakerPricingState {
    pub(crate) last_reference_fair_value: Option<f64>,
    pub(crate) fast_spot: Option<FastSpotObservation>,
    pub(crate) realized_vol: RealizedVolEstimator,
    pub(crate) realized_vol_source_venue: Option<String>,
    pub(crate) realized_vol_by_venue: BTreeMap<String, RealizedVolEstimator>,
    pub(crate) venue_timing: BTreeMap<String, VenueTimingState>,
    pub(crate) last_lead_gap_probability: Option<f64>,
    pub(crate) last_jitter_penalty_probability: Option<f64>,
    pub(crate) last_lead_agreement_corr: Option<f64>,
    pub(crate) last_fast_venue_age_ms: Option<u64>,
    pub(crate) last_fast_venue_jitter_ms: Option<u64>,
    pub(crate) fast_venue_incoherent: bool,
    pub(crate) lead_quality_policy_applied: bool,
    /// Reference-spot spike cooldown deadline (ms). When set, entry is blocked
    /// until `now_ms >= spike_until_ms`.
    pub(crate) spike_until_ms: Option<u64>,
}

impl TakerPricingState {
    pub fn from_config(config: &TakerPricingConfig<'_>) -> Self {
        Self {
            last_reference_fair_value: None,
            fast_spot: None,
            realized_vol: RealizedVolEstimator::from_config(&config.realized_vol),
            realized_vol_source_venue: None,
            realized_vol_by_venue: BTreeMap::new(),
            venue_timing: BTreeMap::new(),
            last_lead_gap_probability: None,
            last_jitter_penalty_probability: None,
            last_lead_agreement_corr: None,
            last_fast_venue_age_ms: None,
            last_fast_venue_jitter_ms: None,
            fast_venue_incoherent: false,
            lead_quality_policy_applied: false,
            spike_until_ms: None,
        }
    }

    pub fn observe_reference_quote(&mut self, quote: &FastSpotObservation) {
        if !is_positive_finite(quote.price) {
            return;
        }

        self.last_reference_fair_value = Some(quote.price);
    }

    pub fn observe_signal_quote(
        &mut self,
        quote: &FastSpotObservation,
        config: &TakerPricingConfig<'_>,
    ) {
        if !is_positive_finite(quote.price) {
            return;
        }

        self.detect_signal_spike(
            quote,
            config.spike_guard_return_threshold,
            config.spike_guard_cooldown_secs,
        );

        self.lead_quality_policy_applied = true;

        let jitter_ms = self.record_signal_quote_timing(&quote.venue, quote.observed_ts_ms);
        let Some(reference_fair_value) = self
            .last_reference_fair_value
            .filter(|value| is_positive_finite(*value))
        else {
            self.fast_spot = None;
            self.last_lead_gap_probability = None;
            self.last_jitter_penalty_probability = None;
            self.last_lead_agreement_corr = None;
            self.last_fast_venue_age_ms = Some(INITIAL_COUNTER_U64);
            self.last_fast_venue_jitter_ms = Some(jitter_ms);
            self.fast_venue_incoherent = true;
            return;
        };
        let agreement_corr = price_agreement_corr(quote.price, reference_fair_value)
            .expect("validated signal/reference prices should yield agreement");
        let lead_gap_probability = price_gap_probability(quote.price, reference_fair_value)
            .expect("validated signal/reference prices should yield a gap");
        let eligible = agreement_corr >= config.lead_agreement_min_corr
            && jitter_ms <= config.lead_jitter_max_ms
            && sanitize_probability(lead_gap_probability).is_some();

        if eligible {
            let selected_realized_vol = {
                let estimator_template = self.realized_vol.empty_like();
                let estimator = self
                    .realized_vol_by_venue
                    .entry(quote.venue.clone())
                    .or_insert_with(|| estimator_template.clone());
                let _ = estimator.observe(&quote.venue, quote.price, quote.observed_ts_ms);
                estimator.clone()
            };
            self.realized_vol = selected_realized_vol;
            self.realized_vol_source_venue = Some(quote.venue.clone());
            self.fast_spot = Some(quote.clone());
            self.last_lead_gap_probability = Some(lead_gap_probability);
            self.last_jitter_penalty_probability = Some(if config.lead_jitter_max_ms == 0 {
                ZERO_F64
            } else {
                clamp_probability(jitter_ms as f64 / config.lead_jitter_max_ms as f64)
            });
            self.last_lead_agreement_corr = Some(agreement_corr);
            self.last_fast_venue_age_ms = Some(INITIAL_COUNTER_U64);
            self.last_fast_venue_jitter_ms = Some(jitter_ms);
            self.fast_venue_incoherent = false;
        } else {
            self.fast_spot = None;
            self.last_lead_gap_probability = Some(lead_gap_probability);
            self.last_jitter_penalty_probability = Some(if config.lead_jitter_max_ms == 0 {
                ZERO_F64
            } else {
                clamp_probability(jitter_ms as f64 / config.lead_jitter_max_ms as f64)
            });
            self.last_lead_agreement_corr = Some(agreement_corr);
            self.last_fast_venue_age_ms = Some(INITIAL_COUNTER_U64);
            self.last_fast_venue_jitter_ms = Some(jitter_ms);
            self.fast_venue_incoherent = true;
        }
    }

    pub(crate) fn spot_price(&self) -> Option<f64> {
        self.fast_spot.as_ref().map(|spot| spot.price)
    }

    pub fn current_realized_vol_at(&self, now_ms: u64) -> Option<f64> {
        self.realized_vol.current_vol_at(now_ms)
    }

    pub fn current_realized_vol_source_at(&self, now_ms: u64) -> (Option<String>, Option<u64>) {
        if self.realized_vol.current_vol_at(now_ms).is_none() {
            return (None, None);
        }

        (
            self.realized_vol_source_venue
                .clone()
                .or_else(|| self.fast_spot.as_ref().map(|spot| spot.venue.clone()))
                .or_else(|| self.realized_vol.active_venue.clone()),
            self.realized_vol.last_ready_ts_ms,
        )
    }

    pub fn seed_ready_realized_vol(
        &mut self,
        source_venue: Option<String>,
        realized_vol: f64,
        ready_ts_ms: u64,
    ) {
        if !is_positive_finite(realized_vol) {
            return;
        }
        if self
            .realized_vol
            .last_ready_ts_ms
            .is_none_or(|current_ts_ms| current_ts_ms <= ready_ts_ms)
        {
            self.realized_vol.last_ready_vol = Some(realized_vol);
            self.realized_vol.last_ready_ts_ms = Some(ready_ts_ms);
            self.realized_vol_source_venue = source_venue;
        }
    }

    pub(crate) fn theta_scaled_min_edge_bps_for(
        &self,
        config: &TakerPricingConfig<'_>,
        seconds_to_market_end: Option<u64>,
    ) -> Option<f64> {
        seconds_to_market_end.and_then(|seconds_to_market_end| {
            compute_theta_scaler(&ThetaScalerInputs {
                seconds_to_market_end,
                cadence_seconds: config.cadence_seconds,
                theta_decay_factor: config.theta_decay_factor,
            })
            .map(|theta| config.edge_threshold_basis_points as f64 * theta)
        })
    }

    pub fn entry_pricing_inputs_at(
        &self,
        config: &TakerPricingConfig<'_>,
        request: TakerPricingRequest,
    ) -> Result<TakerPricingInputs, Vec<TakerPricingBlockReason>> {
        let mut blocked_by = Vec::new();

        let spot_price = self.spot_price().filter(|value| is_positive_finite(*value));
        if spot_price.is_none() {
            blocked_by.push(TakerPricingBlockReason::SpotPriceMissing);
        }

        let strike_price = request
            .strike_price
            .filter(|value| is_positive_finite(*value));
        if strike_price.is_none() {
            blocked_by.push(TakerPricingBlockReason::StrikePriceMissing);
        }

        if request.seconds_to_market_end.is_none() {
            blocked_by.push(TakerPricingBlockReason::SecondsToExpiryMissing);
        }

        let realized_vol = self
            .current_realized_vol_at(request.now_ms)
            .filter(|value| is_positive_finite(*value));
        if realized_vol.is_none() {
            blocked_by.push(TakerPricingBlockReason::RealizedVolNotReady);
        }

        let theta_scaled_min_edge_bps =
            self.theta_scaled_min_edge_bps_for(config, request.seconds_to_market_end);
        if theta_scaled_min_edge_bps.is_none() {
            blocked_by.push(TakerPricingBlockReason::ThetaScalerUnavailable);
        }

        if !blocked_by.is_empty() {
            return Err(blocked_by);
        }

        Ok(TakerPricingInputs {
            spot_price: spot_price.expect("validated above"),
            strike_price: strike_price.expect("validated above"),
            seconds_to_market_end: request.seconds_to_market_end.expect("validated above"),
            realized_vol: realized_vol.expect("validated above"),
            theta_scaled_min_edge_bps: theta_scaled_min_edge_bps.expect("validated above"),
        })
    }

    pub fn entry_pricing_at(
        &self,
        config: &TakerPricingConfig<'_>,
        request: TakerPricingRequest,
    ) -> Result<TakerPricingResult, Vec<TakerPricingBlockReason>> {
        let inputs = self.entry_pricing_inputs_at(config, request)?;
        self.pricing_result_from_inputs(config, request.now_ms, inputs)
    }

    fn pricing_result_from_inputs(
        &self,
        config: &TakerPricingConfig<'_>,
        now_ms: u64,
        inputs: TakerPricingInputs,
    ) -> Result<TakerPricingResult, Vec<TakerPricingBlockReason>> {
        let Some(fair_probability_up) = bolt_v3_market_families::fair_probability_up_for_family(
            config.rotating_market_family,
            &FairProbabilityInputs {
                spot_price: inputs.spot_price,
                strike_price: inputs.strike_price,
                seconds_to_market_end: inputs.seconds_to_market_end,
                realized_vol: inputs.realized_vol,
                pricing_kurtosis: config.pricing_kurtosis,
            },
        ) else {
            return Err(vec![TakerPricingBlockReason::FairProbabilityUnavailable]);
        };
        let (realized_vol_source_venue, realized_vol_source_ts_ms) =
            self.current_realized_vol_source_at(now_ms);

        Ok(TakerPricingResult {
            spot_price: inputs.spot_price,
            strike_price: inputs.strike_price,
            seconds_to_market_end: inputs.seconds_to_market_end,
            realized_vol: inputs.realized_vol,
            realized_vol_source_venue,
            realized_vol_source_ts_ms,
            theta_scaled_min_edge_bps: inputs.theta_scaled_min_edge_bps,
            fair_probability_up,
            fair_probability_down: UNIT_F64 - fair_probability_up,
        })
    }

    /// Arm the spike cooldown when a new signal-price observation jumps past
    /// the configured single-step return threshold.
    fn detect_signal_spike(
        &mut self,
        quote: &FastSpotObservation,
        spike_return_threshold: f64,
        spike_cooldown_secs: u64,
    ) {
        let Some(previous) = self.fast_spot.as_ref() else {
            return;
        };
        if !is_positive_finite(previous.price) || !is_positive_finite(quote.price) {
            return;
        }
        let relative_move = (quote.price / previous.price - UNIT_F64).abs();
        if relative_move >= spike_return_threshold {
            let new_deadline = quote
                .observed_ts_ms
                .saturating_add(spike_cooldown_secs.saturating_mul(MILLIS_PER_SECOND_U64));
            self.spike_until_ms = Some(match self.spike_until_ms {
                Some(existing) => existing.max(new_deadline),
                None => new_deadline,
            });
        }
    }

    fn record_signal_quote_timing(&mut self, venue: &str, observed_ts_ms: u64) -> u64 {
        let timing = self
            .venue_timing
            .entry(venue.to_string())
            .or_insert_with(VenueTimingState::empty);
        let current_interval_ms = timing
            .last_observed_ts_ms
            .map(|last_ts_ms| observed_ts_ms.saturating_sub(last_ts_ms));
        let jitter_ms = match (current_interval_ms, timing.last_interval_ms) {
            (Some(current_interval_ms), Some(last_interval_ms)) => {
                current_interval_ms.abs_diff(last_interval_ms)
            }
            _ => INITIAL_COUNTER_U64,
        };
        timing.last_observed_ts_ms = Some(observed_ts_ms);
        timing.last_interval_ms = current_interval_ms;
        jitter_ms
    }
}
