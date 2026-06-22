//! Shared taker pricing state extracted from `binary_oracle_edge_taker`.
//!
//! This module mirrors the current surfaced-RV taker entry path: reference and
//! lead-venue observations establish shared spot context, then the shared
//! fair-value layer assembles realized-vol, strike, expiry, and family fair
//! probability before taker-only theta/edge policy is applied.
//! It deliberately does not introduce IV, maker spread logic, or submit policy.

use std::collections::BTreeMap;

use crate::{
    bolt_v3_fair_value_pricing::{
        FairValuePricingBlockReason, FairValuePricingConfig, FairValuePricingInputs,
        FairValuePricingRequest, FairValuePricingResult, FairValuePricingState,
    },
    bolt_v3_numeric::{
        MILLIS_PER_SECOND_U64, UNIT_F64, ZERO_F64, clamp_probability, is_positive_finite,
        sanitize_probability,
    },
    bolt_v3_realized_volatility::RealizedVolSnapshot,
    bolt_v3_taker_updown_signal::{
        ThetaScalerInputs, compute_theta_scaler, price_agreement_corr, price_gap_probability,
    },
};

pub use crate::bolt_v3_fair_value_pricing::FastSpotObservation;

const INITIAL_COUNTER_U64: u64 = u64::MIN;

#[derive(Debug, Clone, PartialEq)]
pub struct TakerPricingConfig<'a> {
    pub realized_volatility_surface_id: String,
    pub lead_agreement_min_corr: f64,
    pub lead_jitter_max_ms: u64,
    pub spike_guard_return_threshold: f64,
    pub spike_guard_cooldown_secs: u64,
    pub cadence_seconds: u64,
    pub theta_decay_factor: f64,
    pub edge_threshold_basis_points: i64,
    pub pricing_kurtosis: f64,
    pub rotating_market_family: &'a str,
    pub max_reference_current_price_age_ms: Option<u64>,
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
    pub realized_vol_surface_id: Option<String>,
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
    ReferenceCurrentPriceStale,
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
    fair_value: FairValuePricingState,
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
            fair_value: FairValuePricingState::from_realized_volatility_surface_id(
                config.realized_volatility_surface_id.clone(),
            ),
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

    pub fn observe_reference_current_price(&mut self, quote: &FastSpotObservation) {
        if self.fair_value.observe_reference_current_price(quote)
            && !self.lead_quality_policy_applied
        {
            self.fair_value.observe_pricing_spot(quote);
        }
    }

    pub(crate) fn clear_reference_current_price_state(&mut self) {
        let reference_owned_fast_spot =
            self.fair_value.selected_pricing_spot().is_some_and(|spot| {
                self.fair_value
                    .last_reference_source_id()
                    .is_some_and(|source_id| source_id == spot.venue.as_str())
            });
        self.fair_value.clear_reference_current_price();
        if reference_owned_fast_spot {
            self.fair_value.clear_pricing_spot();
            self.last_lead_gap_probability = None;
            self.last_jitter_penalty_probability = None;
            self.last_lead_agreement_corr = None;
            self.last_fast_venue_age_ms = None;
            self.last_fast_venue_jitter_ms = None;
            self.fast_venue_incoherent = false;
            self.lead_quality_policy_applied = false;
        }
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
            .fair_value
            .last_reference_fair_value()
            .filter(|value| is_positive_finite(*value))
        else {
            self.fair_value.clear_pricing_spot();
            self.last_lead_gap_probability = None;
            self.last_jitter_penalty_probability = None;
            self.last_lead_agreement_corr = None;
            self.last_fast_venue_age_ms = Some(INITIAL_COUNTER_U64);
            self.last_fast_venue_jitter_ms = Some(jitter_ms);
            self.fast_venue_incoherent = true;
            return;
        };
        let agreement_corr = price_agreement_corr(quote.price, reference_fair_value)
            .expect("validated signal/reference current prices should yield agreement");
        let lead_gap_probability = price_gap_probability(quote.price, reference_fair_value)
            .expect("validated signal/reference current prices should yield a gap");
        let eligible = agreement_corr >= config.lead_agreement_min_corr
            && jitter_ms <= config.lead_jitter_max_ms
            && sanitize_probability(lead_gap_probability).is_some();

        if eligible {
            self.fair_value.observe_pricing_spot(quote);
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
            self.fair_value.clear_pricing_spot();
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

    pub fn observe_realized_vol_snapshot(&mut self, snapshot: RealizedVolSnapshot) {
        self.fair_value.observe_realized_vol_snapshot(snapshot);
    }

    pub(crate) fn last_reference_current_price(&self) -> Option<f64> {
        self.fair_value.last_reference_fair_value()
    }

    #[cfg(test)]
    pub(crate) fn last_reference_current_price_source_id(&self) -> Option<&str> {
        self.fair_value.last_reference_source_id()
    }

    pub(crate) fn last_reference_current_price_ts_ms(&self) -> Option<u64> {
        self.fair_value.last_reference_observed_ts_ms()
    }

    pub(crate) fn selected_pricing_spot(&self) -> Option<&FastSpotObservation> {
        self.fair_value.selected_pricing_spot()
    }

    #[cfg(test)]
    pub(crate) fn set_selected_pricing_spot(&mut self, spot: Option<FastSpotObservation>) {
        self.fair_value.set_selected_pricing_spot(spot);
    }

    pub(crate) fn spot_price(&self) -> Option<f64> {
        self.fair_value.spot_price()
    }

    #[cfg(test)]
    pub(crate) fn set_last_reference_fair_value(&mut self, fair_value: Option<f64>) {
        self.fair_value.set_last_reference_fair_value(fair_value);
    }

    #[cfg(test)]
    pub(crate) fn set_last_reference_observation(
        &mut self,
        observed_ts_ms: Option<u64>,
        fair_value: Option<f64>,
    ) {
        self.fair_value
            .set_last_reference_observation(observed_ts_ms, fair_value);
    }

    #[cfg(test)]
    pub(crate) fn set_realized_volatility_surface_id(&mut self, surface_id: String) {
        self.fair_value
            .set_realized_volatility_surface_id(surface_id);
    }

    fn reference_current_price_stale_at(
        &self,
        observed_ts_ms: u64,
        config: &TakerPricingConfig<'_>,
        now_ms: u64,
    ) -> bool {
        config
            .max_reference_current_price_age_ms
            .is_some_and(|max_age_ms| {
                observed_ts_ms > now_ms || now_ms - observed_ts_ms > max_age_ms
            })
    }

    pub fn current_realized_vol_at(&self, now_ms: u64) -> Option<f64> {
        self.fair_value.current_realized_vol_at(now_ms)
    }

    pub fn current_realized_vol_source_at(&self, now_ms: u64) -> (Option<String>, Option<u64>) {
        self.fair_value.current_realized_vol_source_at(now_ms)
    }

    /// Raw (readiness-unfiltered) latest snapshot for a surface, for evidence/audit. Use the
    /// readiness-gating path for entry decisions, which also enforces `as_of_ms <= now_ms`.
    pub(crate) fn latest_realized_vol_snapshot_for_surface(
        &self,
        surface_id: &str,
    ) -> Option<&RealizedVolSnapshot> {
        self.fair_value
            .latest_realized_vol_snapshot_for_surface(surface_id)
    }

    /// Classify the realized-vol staleness gate for a surface at `now_ms`, using the same
    /// shared single-source classifier as the pricing gate (#885 RCA evidence).
    pub(crate) fn classify_realized_vol_gate(
        &self,
        surface_id: &str,
        now_ms: u64,
    ) -> crate::bolt_v3_decision_evidence::BoltV3RvGateResult {
        crate::bolt_v3_fair_value_pricing::classify_rv_gate(
            self.fair_value
                .latest_realized_vol_snapshot_for_surface(surface_id),
            now_ms,
        )
    }

    #[cfg(test)]
    pub(crate) fn clear_latest_realized_vol_snapshot(&mut self) {
        self.fair_value.clear_latest_realized_vol_snapshot();
    }

    #[cfg(test)]
    pub fn seed_ready_realized_vol(
        &mut self,
        source_venue: Option<String>,
        realized_vol: f64,
        ready_ts_ms: u64,
    ) {
        self.fair_value
            .seed_ready_realized_vol(source_venue, realized_vol, ready_ts_ms);
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

        let stale_pricing_spot = self.lead_quality_policy_applied
            && self.selected_pricing_spot().is_some_and(|spot| {
                self.reference_current_price_stale_at(spot.observed_ts_ms, config, request.now_ms)
            });

        let reference_current_price_stale =
            config.max_reference_current_price_age_ms.is_some_and(|_| {
                self.last_reference_current_price_ts_ms()
                    .is_none_or(|ts_ms| {
                        self.reference_current_price_stale_at(ts_ms, config, request.now_ms)
                    })
            });

        let mut fair_value_tail_blockers = Vec::new();

        if stale_pricing_spot {
            blocked_by.push(TakerPricingBlockReason::SpotPriceMissing);
        }

        let fair_value_inputs = match self.fair_value_inputs_at(config, request) {
            Ok(inputs) if !stale_pricing_spot => Some(inputs),
            Ok(_) => None,
            Err(reasons) => {
                for reason in reasons.into_iter().map(TakerPricingBlockReason::from) {
                    if matches!(reason, TakerPricingBlockReason::SpotPriceMissing) {
                        if !stale_pricing_spot {
                            blocked_by.push(reason);
                        }
                    } else {
                        fair_value_tail_blockers.push(reason);
                    }
                }
                None
            }
        };

        if reference_current_price_stale {
            blocked_by.push(TakerPricingBlockReason::ReferenceCurrentPriceStale);
        }
        blocked_by.extend(fair_value_tail_blockers);

        let theta_scaled_min_edge_bps =
            self.theta_scaled_min_edge_bps_for(config, request.seconds_to_market_end);
        if theta_scaled_min_edge_bps.is_none() {
            blocked_by.push(TakerPricingBlockReason::ThetaScalerUnavailable);
        }

        if !blocked_by.is_empty() {
            return Err(blocked_by);
        }

        let fair_value_inputs = fair_value_inputs.expect("validated above");
        Ok(TakerPricingInputs {
            spot_price: fair_value_inputs.spot_price,
            strike_price: fair_value_inputs.strike_price,
            seconds_to_market_end: fair_value_inputs.seconds_to_market_end,
            realized_vol: fair_value_inputs.realized_vol,
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
        let fair_value = self
            .fair_value_pricing_from_inputs(
                config,
                now_ms,
                FairValuePricingInputs {
                    spot_price: inputs.spot_price,
                    strike_price: inputs.strike_price,
                    seconds_to_market_end: inputs.seconds_to_market_end,
                    realized_vol: inputs.realized_vol,
                },
            )
            .map_err(|blocked_by| {
                blocked_by
                    .into_iter()
                    .map(TakerPricingBlockReason::from)
                    .collect::<Vec<_>>()
            })?;

        Ok(TakerPricingResult {
            spot_price: inputs.spot_price,
            strike_price: inputs.strike_price,
            seconds_to_market_end: inputs.seconds_to_market_end,
            realized_vol: inputs.realized_vol,
            realized_vol_surface_id: fair_value.realized_vol_surface_id,
            realized_vol_source_venue: fair_value.realized_vol_source_venue,
            realized_vol_source_ts_ms: fair_value.realized_vol_source_ts_ms,
            theta_scaled_min_edge_bps: inputs.theta_scaled_min_edge_bps,
            fair_probability_up: fair_value.fair_probability_up,
            fair_probability_down: fair_value.fair_probability_down,
        })
    }

    fn fair_value_inputs_at(
        &self,
        config: &TakerPricingConfig<'_>,
        request: TakerPricingRequest,
    ) -> Result<FairValuePricingInputs, Vec<FairValuePricingBlockReason>> {
        self.fair_value.fair_value_inputs_at(
            &fair_value_config(config),
            FairValuePricingRequest {
                now_ms: request.now_ms,
                strike_price: request.strike_price,
                seconds_to_market_end: request.seconds_to_market_end,
            },
        )
    }

    pub fn fair_value_pricing_at(
        &self,
        config: &TakerPricingConfig<'_>,
        request: TakerPricingRequest,
    ) -> Result<FairValuePricingResult, Vec<FairValuePricingBlockReason>> {
        self.fair_value.fair_value_pricing_at(
            &fair_value_config(config),
            FairValuePricingRequest {
                now_ms: request.now_ms,
                strike_price: request.strike_price,
                seconds_to_market_end: request.seconds_to_market_end,
            },
        )
    }

    fn fair_value_pricing_from_inputs(
        &self,
        config: &TakerPricingConfig<'_>,
        now_ms: u64,
        inputs: FairValuePricingInputs,
    ) -> Result<FairValuePricingResult, Vec<FairValuePricingBlockReason>> {
        self.fair_value
            .fair_value_pricing_from_inputs(&fair_value_config(config), now_ms, inputs)
    }

    /// Arm the spike cooldown when a new signal-price observation jumps past
    /// the configured single-step return threshold.
    fn detect_signal_spike(
        &mut self,
        quote: &FastSpotObservation,
        spike_return_threshold: f64,
        spike_cooldown_secs: u64,
    ) {
        let Some(previous) = self.fair_value.selected_pricing_spot() else {
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

fn fair_value_config<'config, 'family>(
    config: &'config TakerPricingConfig<'family>,
) -> FairValuePricingConfig<'config>
where
    'family: 'config,
{
    FairValuePricingConfig {
        realized_volatility_surface_id: config.realized_volatility_surface_id.as_str(),
        pricing_kurtosis: config.pricing_kurtosis,
        market_family: config.rotating_market_family,
    }
}

impl From<FairValuePricingBlockReason> for TakerPricingBlockReason {
    fn from(reason: FairValuePricingBlockReason) -> Self {
        match reason {
            FairValuePricingBlockReason::SpotPriceMissing => Self::SpotPriceMissing,
            FairValuePricingBlockReason::StrikePriceMissing => Self::StrikePriceMissing,
            FairValuePricingBlockReason::SecondsToExpiryMissing => Self::SecondsToExpiryMissing,
            FairValuePricingBlockReason::RealizedVolNotReady => Self::RealizedVolNotReady,
            FairValuePricingBlockReason::FairProbabilityUnavailable => {
                Self::FairProbabilityUnavailable
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_LEAD_AGREEMENT_MIN_CORR: f64 = 0.80;
    const TEST_LEAD_JITTER_MAX_MS: u64 = 1_000;
    const TEST_SPIKE_GUARD_RETURN_THRESHOLD: f64 = 0.10;
    const TEST_SPIKE_GUARD_COOLDOWN_SECS: u64 = 2;
    const TEST_CADENCE_SECONDS: u64 = 300;
    const TEST_THETA_DECAY_FACTOR: f64 = 1.5;
    const TEST_EDGE_THRESHOLD_BASIS_POINTS: i64 = 10;
    const TEST_PRICING_KURTOSIS: f64 = 0.0;
    const TEST_MIN_OBSERVATIONS_READY_AFTER_TWO_SAMPLES: u64 = 1;
    const TEST_GAP_RESET_SECS: u64 = 30;
    const TEST_BRIDGE_VALID_SECS: u64 = 10;
    const TEST_REFERENCE_CURRENT_PRICE_STEP: f64 = 100.0;
    const TEST_REFERENCE_TS_STEP_MS: u64 = 100;
    const TEST_NEWER_REFERENCE_CURRENT_PRICE: f64 = 100.0;
    const TEST_STALE_REFERENCE_CURRENT_PRICE: f64 =
        TEST_NEWER_REFERENCE_CURRENT_PRICE + TEST_REFERENCE_CURRENT_PRICE_STEP;
    const TEST_REPLACEMENT_REFERENCE_CURRENT_PRICE: f64 =
        TEST_STALE_REFERENCE_CURRENT_PRICE + TEST_REFERENCE_CURRENT_PRICE_STEP;
    const TEST_NEWER_REFERENCE_TS_MS: u64 = 1_000;
    const TEST_STALE_REFERENCE_TS_MS: u64 = TEST_NEWER_REFERENCE_TS_MS - TEST_REFERENCE_TS_STEP_MS;
    const TEST_REPLACEMENT_REFERENCE_TS_MS: u64 =
        TEST_NEWER_REFERENCE_TS_MS + TEST_REFERENCE_TS_STEP_MS;
    const TEST_SIGNAL_AFTER_REFERENCE_TS_MS: u64 = TEST_REPLACEMENT_REFERENCE_TS_MS;
    const TEST_SIGNAL_AFTER_REPLACEMENT_REFERENCE_TS_MS: u64 =
        TEST_REPLACEMENT_REFERENCE_TS_MS + TEST_REFERENCE_TS_STEP_MS;

    fn config(
        _min_observations: u64,
        _gap_reset_secs: u64,
        _bridge_valid_secs: u64,
    ) -> TakerPricingConfig<'static> {
        TakerPricingConfig {
            realized_volatility_surface_id: "<surface_id>".to_string(),
            lead_agreement_min_corr: TEST_LEAD_AGREEMENT_MIN_CORR,
            lead_jitter_max_ms: TEST_LEAD_JITTER_MAX_MS,
            spike_guard_return_threshold: TEST_SPIKE_GUARD_RETURN_THRESHOLD,
            spike_guard_cooldown_secs: TEST_SPIKE_GUARD_COOLDOWN_SECS,
            cadence_seconds: TEST_CADENCE_SECONDS,
            theta_decay_factor: TEST_THETA_DECAY_FACTOR,
            edge_threshold_basis_points: TEST_EDGE_THRESHOLD_BASIS_POINTS,
            pricing_kurtosis: TEST_PRICING_KURTOSIS,
            rotating_market_family: crate::bolt_v3_market_families::updown::KEY,
            max_reference_current_price_age_ms: Some(2_000),
        }
    }

    fn reference_current_price_source() -> &'static str {
        std::any::type_name::<FastSpotObservation>()
    }

    fn signal_venue() -> &'static str {
        std::any::type_name::<TakerPricingState>()
    }

    fn quote(venue: &str, price: f64, observed_ts_ms: u64) -> FastSpotObservation {
        FastSpotObservation {
            venue: venue.to_string(),
            price,
            observed_ts_ms,
        }
    }

    fn observe_reference_and_signal(
        pricing: &mut TakerPricingState,
        config: &TakerPricingConfig<'_>,
        venue: &str,
        price: f64,
        observed_ts_ms: u64,
    ) {
        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            price,
            observed_ts_ms,
        ));
        pricing.observe_signal_quote(&quote(venue, price, observed_ts_ms), config);
    }

    #[test]
    fn signal_without_reference_fails_closed_and_marks_fast_venue_incoherent() {
        let config = config(1, 30, 10);
        let mut pricing = TakerPricingState::from_config(&config);

        pricing.observe_signal_quote(&quote("bybit", 3_100.0, 1_000), &config);

        assert_eq!(pricing.selected_pricing_spot(), None);
        assert!(pricing.fast_venue_incoherent);
        assert!(pricing.lead_quality_policy_applied);
        assert_eq!(pricing.last_lead_gap_probability, None);
        assert_eq!(pricing.last_jitter_penalty_probability, None);
        assert_eq!(pricing.last_lead_agreement_corr, None);
        assert_eq!(pricing.last_fast_venue_age_ms, Some(INITIAL_COUNTER_U64));
        assert_eq!(pricing.last_fast_venue_jitter_ms, Some(INITIAL_COUNTER_U64));
    }

    #[test]
    fn reference_tick_does_not_clear_fast_venue_incoherence() {
        let mut config = config(1, 30, 10);
        config.lead_agreement_min_corr = 0.99;
        let mut pricing = TakerPricingState::from_config(&config);

        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            100.0,
            1_000,
        ));
        pricing.observe_signal_quote(&quote(signal_venue(), 120.0, 1_050), &config);
        assert!(pricing.fast_venue_incoherent);
        assert_eq!(pricing.selected_pricing_spot(), None);

        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            101.0,
            1_100,
        ));

        assert!(pricing.fast_venue_incoherent);
        assert_eq!(pricing.selected_pricing_spot(), None);
        assert_eq!(pricing.last_reference_current_price(), Some(101.0));
        assert_eq!(pricing.last_reference_current_price_ts_ms(), Some(1_100));
    }

    #[test]
    fn reference_tick_does_not_overwrite_coherent_signal_fast_spot() {
        let config = config(1, 30, 10);
        let mut pricing = TakerPricingState::from_config(&config);

        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            100.0,
            1_000,
        ));
        assert_eq!(
            pricing.selected_pricing_spot().map(|spot| spot.price),
            Some(100.0)
        );

        pricing.observe_signal_quote(&quote(signal_venue(), 100.1, 1_050), &config);
        assert!(!pricing.fast_venue_incoherent);
        assert!(pricing.lead_quality_policy_applied);
        assert_eq!(
            pricing.selected_pricing_spot().map(|spot| spot.price),
            Some(100.1)
        );

        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            99.9,
            1_100,
        ));

        assert_eq!(pricing.last_reference_current_price(), Some(99.9));
        assert_eq!(pricing.last_reference_current_price_ts_ms(), Some(1_100));
        assert_eq!(
            pricing.selected_pricing_spot().map(|spot| spot.price),
            Some(100.1)
        );
        assert_eq!(
            pricing
                .selected_pricing_spot()
                .map(|spot| spot.venue.as_str()),
            Some(signal_venue())
        );
    }

    #[test]
    fn stale_reference_current_price_blocks_entry_inputs_at_decision_time() {
        let config = config(1, 30, 10);
        let mut pricing = TakerPricingState::from_config(&config);
        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            100.0,
            1_000,
        ));
        pricing.seed_ready_realized_vol(Some("rv".to_string()), 1.5, 1_000);

        let blocked_by = pricing
            .entry_pricing_inputs_at(
                &config,
                TakerPricingRequest {
                    now_ms: 3_001,
                    strike_price: Some(100.0),
                    seconds_to_market_end: Some(300),
                },
            )
            .expect_err("stale reference current price must block entry pricing inputs");

        assert_eq!(
            blocked_by,
            vec![TakerPricingBlockReason::ReferenceCurrentPriceStale]
        );
    }

    #[test]
    fn stale_signal_fast_spot_blocks_entry_inputs_at_decision_time() {
        let config = config(1, 30, 10);
        let mut pricing = TakerPricingState::from_config(&config);

        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            100.0,
            1_000,
        ));
        pricing.observe_signal_quote(&quote(signal_venue(), 100.1, 1_000), &config);
        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            100.2,
            3_000,
        ));
        pricing.seed_ready_realized_vol(Some("rv".to_string()), 1.5, 3_000);

        let blocked_by = pricing
            .entry_pricing_inputs_at(
                &config,
                TakerPricingRequest {
                    now_ms: 3_001,
                    strike_price: Some(100.0),
                    seconds_to_market_end: Some(300),
                },
            )
            .expect_err("stale signal fast spot must block entry pricing inputs");

        assert_eq!(blocked_by, vec![TakerPricingBlockReason::SpotPriceMissing]);
    }

    #[test]
    fn out_of_order_reference_current_price_does_not_overwrite_newer_value() {
        let config = config(
            TEST_MIN_OBSERVATIONS_READY_AFTER_TWO_SAMPLES,
            TEST_GAP_RESET_SECS,
            TEST_BRIDGE_VALID_SECS,
        );
        let mut pricing = TakerPricingState::from_config(&config);

        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            TEST_NEWER_REFERENCE_CURRENT_PRICE,
            TEST_NEWER_REFERENCE_TS_MS,
        ));
        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            TEST_STALE_REFERENCE_CURRENT_PRICE,
            TEST_STALE_REFERENCE_TS_MS,
        ));
        pricing.observe_signal_quote(
            &quote(
                signal_venue(),
                TEST_NEWER_REFERENCE_CURRENT_PRICE,
                TEST_SIGNAL_AFTER_REFERENCE_TS_MS,
            ),
            &config,
        );

        assert_eq!(
            pricing.last_reference_current_price(),
            Some(TEST_NEWER_REFERENCE_CURRENT_PRICE)
        );
        assert_eq!(
            pricing.last_reference_current_price_ts_ms(),
            Some(TEST_NEWER_REFERENCE_TS_MS)
        );
        assert_eq!(
            pricing.selected_pricing_spot().map(|spot| spot.price),
            Some(TEST_NEWER_REFERENCE_CURRENT_PRICE)
        );
        assert!(!pricing.fast_venue_incoherent);
    }

    #[test]
    fn different_reference_current_price_source_can_replace_newer_timestamp() {
        let config = config(
            TEST_MIN_OBSERVATIONS_READY_AFTER_TWO_SAMPLES,
            TEST_GAP_RESET_SECS,
            TEST_BRIDGE_VALID_SECS,
        );
        let mut pricing = TakerPricingState::from_config(&config);

        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            TEST_NEWER_REFERENCE_CURRENT_PRICE,
            TEST_NEWER_REFERENCE_TS_MS,
        ));
        pricing.observe_reference_current_price(&quote(
            "backup_reference_current_price",
            TEST_STALE_REFERENCE_CURRENT_PRICE,
            TEST_STALE_REFERENCE_TS_MS,
        ));
        pricing.observe_reference_current_price(&quote(
            "backup_reference_current_price",
            TEST_REPLACEMENT_REFERENCE_CURRENT_PRICE,
            TEST_STALE_REFERENCE_TS_MS - 1,
        ));

        assert_eq!(
            pricing.last_reference_current_price(),
            Some(TEST_STALE_REFERENCE_CURRENT_PRICE)
        );
        assert_eq!(
            pricing.last_reference_current_price_source_id(),
            Some("backup_reference_current_price")
        );
        assert_eq!(
            pricing.last_reference_current_price_ts_ms(),
            Some(TEST_STALE_REFERENCE_TS_MS)
        );
    }

    #[test]
    fn newer_reference_current_price_overwrites_previous_value() {
        let config = config(
            TEST_MIN_OBSERVATIONS_READY_AFTER_TWO_SAMPLES,
            TEST_GAP_RESET_SECS,
            TEST_BRIDGE_VALID_SECS,
        );
        let mut pricing = TakerPricingState::from_config(&config);

        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            TEST_NEWER_REFERENCE_CURRENT_PRICE,
            TEST_NEWER_REFERENCE_TS_MS,
        ));
        pricing.observe_reference_current_price(&quote(
            reference_current_price_source(),
            TEST_REPLACEMENT_REFERENCE_CURRENT_PRICE,
            TEST_REPLACEMENT_REFERENCE_TS_MS,
        ));
        pricing.observe_signal_quote(
            &quote(
                signal_venue(),
                TEST_REPLACEMENT_REFERENCE_CURRENT_PRICE,
                TEST_SIGNAL_AFTER_REPLACEMENT_REFERENCE_TS_MS,
            ),
            &config,
        );

        assert_eq!(
            pricing.last_reference_current_price(),
            Some(TEST_REPLACEMENT_REFERENCE_CURRENT_PRICE)
        );
        assert_eq!(
            pricing.last_reference_current_price_ts_ms(),
            Some(TEST_REPLACEMENT_REFERENCE_TS_MS)
        );
        assert_eq!(
            pricing.selected_pricing_spot().map(|spot| spot.price),
            Some(TEST_REPLACEMENT_REFERENCE_CURRENT_PRICE)
        );
        assert!(!pricing.fast_venue_incoherent);
    }

    #[test]
    fn spike_cooldown_arms_on_large_signal_move_and_only_extends() {
        let config = config(1, 30, 10);
        let mut pricing = TakerPricingState::from_config(&config);

        observe_reference_and_signal(&mut pricing, &config, "bybit", 100.0, 1_000);
        assert_eq!(pricing.spike_until_ms, None);

        observe_reference_and_signal(&mut pricing, &config, "bybit", 120.0, 1_500);
        assert_eq!(pricing.spike_until_ms, Some(3_500));

        observe_reference_and_signal(&mut pricing, &config, "bybit", 125.0, 2_000);
        assert_eq!(pricing.spike_until_ms, Some(3_500));

        observe_reference_and_signal(&mut pricing, &config, "bybit", 90.0, 2_200);
        assert_eq!(pricing.spike_until_ms, Some(4_200));

        let shorter_cooldown = TakerPricingConfig {
            spike_guard_cooldown_secs: 1,
            ..config
        };
        observe_reference_and_signal(&mut pricing, &shorter_cooldown, "bybit", 120.0, 2_500);
        assert_eq!(pricing.spike_until_ms, Some(4_200));
    }

    #[test]
    fn zero_jitter_threshold_uses_zero_penalty_without_dividing() {
        let mut config = config(1, 30, 10);
        config.lead_jitter_max_ms = 0;
        let mut pricing = TakerPricingState::from_config(&config);

        observe_reference_and_signal(&mut pricing, &config, "bybit", 100.0, 1_000);
        observe_reference_and_signal(&mut pricing, &config, "bybit", 100.0, 2_000);

        assert!(!pricing.fast_venue_incoherent);
        assert_eq!(pricing.last_jitter_penalty_probability, Some(ZERO_F64));

        observe_reference_and_signal(&mut pricing, &config, "bybit", 100.0, 2_500);

        assert!(pricing.fast_venue_incoherent);
        assert_eq!(pricing.last_jitter_penalty_probability, Some(ZERO_F64));
    }
}
