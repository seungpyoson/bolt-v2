//! Shared taker pricing state extracted from `binary_oracle_edge_taker`.
//!
//! This module mirrors the current surfaced-RV taker pricing path: reference and
//! lead-venue observations establish spot context, then a shared realized-vol
//! snapshot plus current strike/expiry/config are assembled into the existing
//! market-family fair probability.
//! It deliberately does not introduce IV, maker spread logic, or submit policy.

use std::collections::BTreeMap;

#[cfg(test)]
use crate::bolt_v3_realized_volatility::RealizedVolAggregation;
use crate::{
    bolt_v3_market_families::{self, FairProbabilityInputs},
    bolt_v3_numeric::{
        MILLIS_PER_SECOND_U64, UNIT_F64, ZERO_F64, clamp_probability, is_positive_finite,
        sanitize_probability,
    },
    bolt_v3_realized_volatility::RealizedVolSnapshot,
    bolt_v3_taker_signal::{
        ThetaScalerInputs, compute_theta_scaler, price_agreement_corr, price_gap_probability,
    },
};

const INITIAL_COUNTER_U64: u64 = u64::MIN;

#[derive(Debug, Clone, PartialEq)]
pub struct FastSpotObservation {
    pub venue: String,
    pub price: f64,
    pub observed_ts_ms: u64,
}

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
    pub(crate) last_reference_current_price: Option<f64>,
    pub(crate) last_reference_current_price_ts_ms: Option<u64>,
    pub(crate) fast_spot: Option<FastSpotObservation>,
    pub(crate) realized_volatility_surface_id: String,
    pub(crate) latest_realized_vol_snapshot: Option<RealizedVolSnapshot>,
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
            last_reference_current_price: None,
            last_reference_current_price_ts_ms: None,
            fast_spot: None,
            realized_volatility_surface_id: config.realized_volatility_surface_id.clone(),
            latest_realized_vol_snapshot: None,
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
        if !is_positive_finite(quote.price) {
            return;
        }
        if self
            .last_reference_current_price_ts_ms
            .is_some_and(|last_ts_ms| quote.observed_ts_ms <= last_ts_ms)
        {
            return;
        }

        self.last_reference_current_price_ts_ms = Some(quote.observed_ts_ms);
        self.last_reference_current_price = Some(quote.price);
        self.fast_spot = Some(quote.clone());
    }

    pub(crate) fn clear_reference_current_price_state(&mut self) {
        self.last_reference_current_price = None;
        self.last_reference_current_price_ts_ms = None;
        self.fast_spot = None;
        self.last_lead_gap_probability = None;
        self.last_jitter_penalty_probability = None;
        self.last_lead_agreement_corr = None;
        self.last_fast_venue_age_ms = None;
        self.last_fast_venue_jitter_ms = None;
        self.fast_venue_incoherent = false;
        self.lead_quality_policy_applied = false;
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
        let Some(reference_current_price) = self
            .last_reference_current_price
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
        let agreement_corr = price_agreement_corr(quote.price, reference_current_price)
            .expect("validated signal/reference current prices should yield agreement");
        let lead_gap_probability = price_gap_probability(quote.price, reference_current_price)
            .expect("validated signal/reference current prices should yield a gap");
        let eligible = agreement_corr >= config.lead_agreement_min_corr
            && jitter_ms <= config.lead_jitter_max_ms
            && sanitize_probability(lead_gap_probability).is_some();

        if eligible {
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

    pub fn observe_realized_vol_snapshot(&mut self, snapshot: RealizedVolSnapshot) {
        if self
            .latest_realized_vol_snapshot
            .as_ref()
            .is_none_or(|current| current.as_of_ms <= snapshot.as_of_ms)
        {
            self.latest_realized_vol_snapshot = Some(snapshot);
        }
    }

    pub(crate) fn spot_price(&self) -> Option<f64> {
        self.fast_spot.as_ref().map(|spot| spot.price)
    }

    pub fn current_realized_vol_at(&self, now_ms: u64) -> Option<f64> {
        self.current_surfaced_realized_vol_at(&self.realized_volatility_surface_id, now_ms)
    }

    pub fn current_realized_vol_source_at(&self, now_ms: u64) -> (Option<String>, Option<u64>) {
        self.current_surfaced_realized_vol_snapshot_at(&self.realized_volatility_surface_id, now_ms)
            .map_or((None, None), |snapshot| (None, Some(snapshot.as_of_ms)))
    }

    fn current_realized_vol_for_config_at(
        &self,
        config: &TakerPricingConfig<'_>,
        now_ms: u64,
    ) -> Option<f64> {
        self.current_surfaced_realized_vol_at(&config.realized_volatility_surface_id, now_ms)
    }

    fn current_realized_vol_evidence_for_config_at(
        &self,
        config: &TakerPricingConfig<'_>,
        now_ms: u64,
    ) -> (Option<String>, Option<String>, Option<u64>) {
        let surface_id = config.realized_volatility_surface_id.as_str();
        self.current_surfaced_realized_vol_snapshot_at(surface_id, now_ms)
            .map_or((None, None, None), |snapshot| {
                (Some(surface_id.to_string()), None, Some(snapshot.as_of_ms))
            })
    }

    fn current_surfaced_realized_vol_at(&self, surface_id: &str, now_ms: u64) -> Option<f64> {
        self.current_surfaced_realized_vol_snapshot_at(surface_id, now_ms)
            .and_then(|snapshot| snapshot.ready_realized_vol())
            .map(|realized_vol| realized_vol.get())
    }

    fn current_surfaced_realized_vol_snapshot_at(
        &self,
        surface_id: &str,
        now_ms: u64,
    ) -> Option<&RealizedVolSnapshot> {
        let snapshot = self.latest_realized_vol_snapshot.as_ref()?;
        if snapshot.surface_id != surface_id
            || snapshot.as_of_ms > now_ms
            || snapshot.ready_realized_vol().is_none()
        {
            return None;
        }

        Some(snapshot)
    }

    #[cfg(test)]
    pub fn seed_ready_realized_vol(
        &mut self,
        source_venue: Option<String>,
        realized_vol: f64,
        ready_ts_ms: u64,
    ) {
        if crate::bolt_v3_realized_volatility::ValidRealizedVol::new(realized_vol).is_none() {
            return;
        }
        self.observe_realized_vol_snapshot(RealizedVolSnapshot {
            surface_id: self.realized_volatility_surface_id.clone(),
            as_of_ms: ready_ts_ms,
            annualized_realized_vol_decimal: Some(realized_vol),
            measured_annualized_realized_vol_decimal: Some(realized_vol),
            noise_robust_annualized_realized_vol_decimal: Some(realized_vol),
            continuous_annualized_realized_vol_decimal: Some(realized_vol),
            jump_annualized_realized_vol_decimal: Some(0.0),
            forecast_annualized_realized_vol_decimal: None,
            pricing_component:
                crate::bolt_v3_realized_volatility::RealizedVolPricingComponent::Measured,
            ready: true,
            sources_used: source_venue.into_iter().collect(),
            source_diagnostics: Vec::new(),
            horizon_estimates: Vec::new(),
            unknown_source_rejections: BTreeMap::new(),
            blocked_reasons: Vec::new(),
            aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
            seconds_per_annum: 31_536_000.0,
            config_fingerprint: String::new(),
        });
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

        let realized_vol = self.current_realized_vol_for_config_at(config, request.now_ms);
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
        let (realized_vol_surface_id, realized_vol_source_venue, realized_vol_source_ts_ms) =
            self.current_realized_vol_evidence_for_config_at(config, now_ms);

        Ok(TakerPricingResult {
            spot_price: inputs.spot_price,
            strike_price: inputs.strike_price,
            seconds_to_market_end: inputs.seconds_to_market_end,
            realized_vol: inputs.realized_vol,
            realized_vol_surface_id,
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
            rotating_market_family: bolt_v3_market_families::updown::KEY,
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
        pricing.last_reference_current_price = Some(price);
        pricing.last_reference_current_price_ts_ms = Some(observed_ts_ms);
        pricing.observe_signal_quote(&quote(venue, price, observed_ts_ms), config);
    }

    #[test]
    fn signal_without_reference_fails_closed_and_marks_fast_venue_incoherent() {
        let config = config(1, 30, 10);
        let mut pricing = TakerPricingState::from_config(&config);

        pricing.observe_signal_quote(&quote("bybit", 3_100.0, 1_000), &config);

        assert_eq!(pricing.fast_spot, None);
        assert!(pricing.fast_venue_incoherent);
        assert!(pricing.lead_quality_policy_applied);
        assert_eq!(pricing.last_lead_gap_probability, None);
        assert_eq!(pricing.last_jitter_penalty_probability, None);
        assert_eq!(pricing.last_lead_agreement_corr, None);
        assert_eq!(pricing.last_fast_venue_age_ms, Some(INITIAL_COUNTER_U64));
        assert_eq!(pricing.last_fast_venue_jitter_ms, Some(INITIAL_COUNTER_U64));
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
            pricing.last_reference_current_price,
            Some(TEST_NEWER_REFERENCE_CURRENT_PRICE)
        );
        assert_eq!(
            pricing.last_reference_current_price_ts_ms,
            Some(TEST_NEWER_REFERENCE_TS_MS)
        );
        assert_eq!(
            pricing.fast_spot.as_ref().map(|spot| spot.price),
            Some(TEST_NEWER_REFERENCE_CURRENT_PRICE)
        );
        assert!(!pricing.fast_venue_incoherent);
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
            pricing.last_reference_current_price,
            Some(TEST_REPLACEMENT_REFERENCE_CURRENT_PRICE)
        );
        assert_eq!(
            pricing.last_reference_current_price_ts_ms,
            Some(TEST_REPLACEMENT_REFERENCE_TS_MS)
        );
        assert_eq!(
            pricing.fast_spot.as_ref().map(|spot| spot.price),
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
