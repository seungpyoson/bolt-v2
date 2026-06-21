//! Shared fair-value pricing inputs for binary oracle strategies.
//!
//! This module owns the reusable current-value inputs needed by both maker and
//! taker strategy layers: selected spot, realized-vol snapshot freshness,
//! strike, expiry, market-family pricing parameters, and family fair
//! probability. Taker-only theta/edge blockers and maker-only spread,
//! inventory, skew, and submit mechanics stay outside this module.

use crate::{
    bolt_v3_decision_evidence::BoltV3RvGateResult,
    bolt_v3_market_families::{self, FairProbabilityInputs},
    bolt_v3_numeric::{UNIT_F64, is_positive_finite},
    bolt_v3_realized_volatility::RealizedVolSnapshot,
};

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct FastSpotObservation {
    pub venue: String,
    pub price: f64,
    pub observed_ts_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FairValuePricingConfig<'a> {
    pub realized_volatility_surface_id: &'a str,
    pub pricing_kurtosis: f64,
    pub market_family: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FairValuePricingRequest {
    pub now_ms: u64,
    pub strike_price: Option<f64>,
    pub seconds_to_market_end: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FairValuePricingInputs {
    pub spot_price: f64,
    pub strike_price: f64,
    pub seconds_to_market_end: u64,
    pub realized_vol: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FairValuePricingResult {
    pub spot_price: f64,
    pub strike_price: f64,
    pub seconds_to_market_end: u64,
    pub realized_vol: f64,
    pub realized_vol_surface_id: Option<String>,
    pub realized_vol_source_venue: Option<String>,
    pub realized_vol_source_ts_ms: Option<u64>,
    pub fair_probability_up: f64,
    pub fair_probability_down: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FairValuePricingBlockReason {
    SpotPriceMissing,
    StrikePriceMissing,
    SecondsToExpiryMissing,
    RealizedVolNotReady,
    FairProbabilityUnavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FairValuePricingState {
    last_reference_fair_value: Option<f64>,
    last_reference_source_id: Option<String>,
    last_reference_observed_ts_ms: Option<u64>,
    selected_pricing_spot: Option<FastSpotObservation>,
    realized_volatility_surface_id: String,
    // Latest RV snapshot per `surface_id`, as prescribed by issue #770. The shared runtime
    // routes ticks by instrument, so a strategy may observe snapshots for surfaces it does
    // not price; those foreign entries are retained under their own keys but are never read
    // by this strategy for pricing.
    latest_realized_vol_snapshots: BTreeMap<String, RealizedVolSnapshot>,
}

impl FairValuePricingState {
    pub fn from_realized_volatility_surface_id(realized_volatility_surface_id: String) -> Self {
        Self {
            last_reference_fair_value: None,
            last_reference_source_id: None,
            last_reference_observed_ts_ms: None,
            selected_pricing_spot: None,
            realized_volatility_surface_id,
            latest_realized_vol_snapshots: BTreeMap::new(),
        }
    }

    pub fn observe_reference_current_price(&mut self, quote: &FastSpotObservation) -> bool {
        if !is_positive_finite(quote.price) {
            return false;
        }
        let same_reference_source = self
            .last_reference_source_id
            .as_deref()
            .is_some_and(|source_id| source_id == quote.venue);
        if same_reference_source
            && self
                .last_reference_observed_ts_ms
                .is_some_and(|last_ts_ms| quote.observed_ts_ms <= last_ts_ms)
        {
            return false;
        }

        self.last_reference_source_id = Some(quote.venue.clone());
        self.last_reference_observed_ts_ms = Some(quote.observed_ts_ms);
        self.last_reference_fair_value = Some(quote.price);
        true
    }

    pub fn clear_reference_current_price(&mut self) {
        self.last_reference_fair_value = None;
        self.last_reference_source_id = None;
        self.last_reference_observed_ts_ms = None;
    }

    pub fn observe_pricing_spot(&mut self, quote: &FastSpotObservation) {
        if is_positive_finite(quote.price) {
            self.selected_pricing_spot = Some(quote.clone());
        }
    }

    pub fn clear_pricing_spot(&mut self) {
        self.selected_pricing_spot = None;
    }

    pub fn last_reference_fair_value(&self) -> Option<f64> {
        self.last_reference_fair_value
    }

    pub fn last_reference_observed_ts_ms(&self) -> Option<u64> {
        self.last_reference_observed_ts_ms
    }

    pub fn last_reference_source_id(&self) -> Option<&str> {
        self.last_reference_source_id.as_deref()
    }

    #[cfg(test)]
    pub fn set_last_reference_observation(
        &mut self,
        observed_ts_ms: Option<u64>,
        fair_value: Option<f64>,
    ) {
        self.last_reference_observed_ts_ms = observed_ts_ms;
        self.last_reference_fair_value = fair_value;
    }

    pub fn selected_pricing_spot(&self) -> Option<&FastSpotObservation> {
        self.selected_pricing_spot.as_ref()
    }

    #[cfg(test)]
    pub fn set_selected_pricing_spot(&mut self, spot: Option<FastSpotObservation>) {
        self.selected_pricing_spot = spot;
    }

    pub fn spot_price(&self) -> Option<f64> {
        self.selected_pricing_spot.as_ref().map(|spot| spot.price)
    }

    #[cfg(test)]
    pub fn set_last_reference_fair_value(&mut self, fair_value: Option<f64>) {
        self.last_reference_fair_value = fair_value;
    }

    #[cfg(test)]
    pub fn set_realized_volatility_surface_id(&mut self, surface_id: String) {
        self.realized_volatility_surface_id = surface_id;
    }

    pub fn observe_realized_vol_snapshot(&mut self, snapshot: RealizedVolSnapshot) {
        let surface_id = snapshot.surface_id.as_str();
        let current = self.latest_realized_vol_snapshots.get(surface_id);
        if current.is_none_or(|current| current.as_of_ms <= snapshot.as_of_ms) {
            self.latest_realized_vol_snapshots
                .insert(snapshot.surface_id.clone(), snapshot);
        }
    }

    /// Raw (readiness-unfiltered) latest snapshot for a surface, for evidence/audit.
    pub fn latest_realized_vol_snapshot_for_surface(
        &self,
        surface_id: &str,
    ) -> Option<&RealizedVolSnapshot> {
        self.latest_realized_vol_snapshots.get(surface_id)
    }

    #[cfg(test)]
    pub fn clear_latest_realized_vol_snapshot(&mut self) {
        self.latest_realized_vol_snapshots
            .remove(&self.realized_volatility_surface_id);
    }

    pub fn current_realized_vol_at(&self, now_ms: u64) -> Option<f64> {
        self.current_surfaced_realized_vol_at(&self.realized_volatility_surface_id, now_ms)
    }

    pub fn current_realized_vol_source_at(&self, now_ms: u64) -> (Option<String>, Option<u64>) {
        self.current_surfaced_realized_vol_snapshot_at(&self.realized_volatility_surface_id, now_ms)
            .map_or((None, None), realized_vol_source_evidence)
    }

    pub fn fair_value_inputs_at(
        &self,
        config: &FairValuePricingConfig<'_>,
        request: FairValuePricingRequest,
    ) -> Result<FairValuePricingInputs, Vec<FairValuePricingBlockReason>> {
        self.debug_assert_config_surface_matches_state(config);
        let mut blocked_by = Vec::new();

        let spot_price = self.spot_price().filter(|value| is_positive_finite(*value));
        if spot_price.is_none() {
            blocked_by.push(FairValuePricingBlockReason::SpotPriceMissing);
        }

        let strike_price = request
            .strike_price
            .filter(|value| is_positive_finite(*value));
        if strike_price.is_none() {
            blocked_by.push(FairValuePricingBlockReason::StrikePriceMissing);
        }

        if request.seconds_to_market_end.is_none() {
            blocked_by.push(FairValuePricingBlockReason::SecondsToExpiryMissing);
        }

        let realized_vol = self.current_realized_vol_for_config_at(config, request.now_ms);
        if realized_vol.is_none() {
            blocked_by.push(FairValuePricingBlockReason::RealizedVolNotReady);
        }

        if !blocked_by.is_empty() {
            return Err(blocked_by);
        }

        Ok(FairValuePricingInputs {
            spot_price: spot_price.expect("validated above"),
            strike_price: strike_price.expect("validated above"),
            seconds_to_market_end: request.seconds_to_market_end.expect("validated above"),
            realized_vol: realized_vol.expect("validated above"),
        })
    }

    pub fn fair_value_pricing_at(
        &self,
        config: &FairValuePricingConfig<'_>,
        request: FairValuePricingRequest,
    ) -> Result<FairValuePricingResult, Vec<FairValuePricingBlockReason>> {
        let inputs = self.fair_value_inputs_at(config, request)?;
        self.fair_value_pricing_from_inputs(config, request.now_ms, inputs)
    }

    pub fn fair_value_pricing_from_inputs(
        &self,
        config: &FairValuePricingConfig<'_>,
        now_ms: u64,
        inputs: FairValuePricingInputs,
    ) -> Result<FairValuePricingResult, Vec<FairValuePricingBlockReason>> {
        self.debug_assert_config_surface_matches_state(config);
        let Some(fair_probability_up) = bolt_v3_market_families::fair_probability_up_for_family(
            config.market_family,
            &FairProbabilityInputs {
                spot_price: inputs.spot_price,
                strike_price: inputs.strike_price,
                seconds_to_market_end: inputs.seconds_to_market_end,
                realized_vol: inputs.realized_vol,
                pricing_kurtosis: config.pricing_kurtosis,
            },
        ) else {
            return Err(vec![
                FairValuePricingBlockReason::FairProbabilityUnavailable,
            ]);
        };
        let (realized_vol_surface_id, realized_vol_source_venue, realized_vol_source_ts_ms) =
            self.current_realized_vol_evidence_for_config_at(config, now_ms);

        Ok(FairValuePricingResult {
            spot_price: inputs.spot_price,
            strike_price: inputs.strike_price,
            seconds_to_market_end: inputs.seconds_to_market_end,
            realized_vol: inputs.realized_vol,
            realized_vol_surface_id,
            realized_vol_source_venue,
            realized_vol_source_ts_ms,
            fair_probability_up,
            fair_probability_down: UNIT_F64 - fair_probability_up,
        })
    }

    fn current_realized_vol_for_config_at(
        &self,
        config: &FairValuePricingConfig<'_>,
        now_ms: u64,
    ) -> Option<f64> {
        self.current_surfaced_realized_vol_at(config.realized_volatility_surface_id, now_ms)
    }

    fn current_realized_vol_evidence_for_config_at(
        &self,
        config: &FairValuePricingConfig<'_>,
        now_ms: u64,
    ) -> (Option<String>, Option<String>, Option<u64>) {
        let surface_id = config.realized_volatility_surface_id;
        self.current_surfaced_realized_vol_snapshot_at(surface_id, now_ms)
            .map_or((None, None, None), |snapshot| {
                let (source_venue, source_ts_ms) = realized_vol_source_evidence(snapshot);
                (Some(surface_id.to_string()), source_venue, source_ts_ms)
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
        let snapshot = self.latest_realized_vol_snapshots.get(surface_id);
        // Single source of truth for the realized-vol staleness gate: the same
        // classification drives both the pricing gate (here) and the RCA exit
        // evidence (#885). The gate admits a snapshot only when `Accepted`.
        match classify_rv_gate(snapshot, now_ms) {
            BoltV3RvGateResult::Accepted => snapshot,
            BoltV3RvGateResult::MissingSnapshot
            | BoltV3RvGateResult::RejectedFutureDated
            | BoltV3RvGateResult::RejectedNotReady => None,
        }
    }

    fn debug_assert_config_surface_matches_state(&self, config: &FairValuePricingConfig<'_>) {
        debug_assert_eq!(
            self.realized_volatility_surface_id.as_str(),
            config.realized_volatility_surface_id,
            "fair-value config surface must match the surface-scoped pricing state"
        );
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
            unknown_source_rejections: std::collections::BTreeMap::new(),
            blocked_reasons: Vec::new(),
            aggregate_method:
                crate::bolt_v3_realized_volatility::RealizedVolAggregation::UpperQuantile {
                    quantile: 1.0,
                },
            seconds_per_annum: 31_536_000.0,
            config_fingerprint: String::new(),
        });
    }
}

fn realized_vol_source_evidence(snapshot: &RealizedVolSnapshot) -> (Option<String>, Option<u64>) {
    (
        snapshot.sources_used.first().cloned(),
        Some(snapshot.as_of_ms),
    )
}

/// Classify the realized-vol staleness gate for a surface at `now_ms`.
///
/// Single source of truth for the gate decision: `current_surfaced_realized_vol_snapshot_at`
/// admits a snapshot only when this returns [`BoltV3RvGateResult::Accepted`], and the #885
/// exit-evaluation evidence records the same classification so a rejected snapshot is
/// explainable from disk. The production gate collapses future-dated and not-ready into a
/// single `None`; this split preserves which condition fired for RCA.
pub fn classify_rv_gate(snapshot: Option<&RealizedVolSnapshot>, now_ms: u64) -> BoltV3RvGateResult {
    let Some(snapshot) = snapshot else {
        return BoltV3RvGateResult::MissingSnapshot;
    };
    if snapshot.as_of_ms > now_ms {
        return BoltV3RvGateResult::RejectedFutureDated;
    }
    if snapshot.ready_realized_vol().is_none() {
        return BoltV3RvGateResult::RejectedNotReady;
    }
    BoltV3RvGateResult::Accepted
}
