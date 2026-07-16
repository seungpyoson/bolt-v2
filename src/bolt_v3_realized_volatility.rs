//! RV-specific realized-volatility engine.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Write,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::bolt_v3_numeric::{
    HALF_F64, MILLIS_PER_SECOND_F64, POWER_OF_TWO, UNIT_F64, ZERO_F64, is_positive_finite,
};
use crate::bolt_v3_timestamp_domain::{LocalReceiveMs, VenueEventMs};

const ZERO_MILLIS_U64: u64 = u64::MIN;
const ZERO_COUNT_USIZE: usize = usize::MIN;
const INITIAL_REJECTION_COUNT: u64 = u64::MIN;
const COUNTER_INCREMENT_U64: u64 = 1;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RealizedVolEngineConfig {
    pub surface_id: String,
    pub window_ms: u64,
    pub sampling_interval_ms: u64,
    pub min_ready_sources: usize,
    pub max_source_age_ms: u64,
    pub max_inter_sample_gap_ms: u64,
    pub min_coverage_ratio: f64,
    pub max_cross_source_dispersion: f64,
    pub seconds_per_annum: f64,
    pub aggregation: RealizedVolAggregation,
    pub estimator: RealizedVolEstimatorConfig,
    pub sources: Vec<RealizedVolSourceConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum RealizedVolAggregation {
    UpperQuantile {
        quantile: f64,
    },
    Median,
    TrimmedMean {
        trim_fraction: f64,
    },
    MedianWithUpperQuantileGuard {
        upper_quantile: f64,
        guard_weight: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RealizedVolEstimatorConfig {
    pub horizons: Vec<RealizedVolHorizonConfig>,
    pub horizon_policy: RealizedVolHorizonPolicy,
    pub noise: RealizedVolNoiseConfig,
    pub jump: RealizedVolJumpConfig,
    pub forecast: RealizedVolForecastConfig,
    pub pricing_component: RealizedVolPricingComponent,
}

impl RealizedVolEstimatorConfig {
    pub fn measured() -> Self {
        Self {
            horizons: Vec::new(),
            horizon_policy: RealizedVolHorizonPolicy::Measured,
            noise: RealizedVolNoiseConfig::none(),
            jump: RealizedVolJumpConfig::none(),
            forecast: RealizedVolForecastConfig::none(),
            pricing_component: RealizedVolPricingComponent::Measured,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RealizedVolHorizonConfig {
    pub horizon_id: String,
    pub window_ms: u64,
    pub sampling_interval_ms: u64,
    pub required: bool,
    pub weight: f64,
    pub role: Option<RealizedVolHorizonRole>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RealizedVolHorizonRole {
    Short,
    Medium,
    Long,
    Primary,
    Floor,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum RealizedVolHorizonPolicy {
    Measured,
    WeightedBlend,
    MaxFloor {
        primary_horizon_id: String,
        floor_horizon_id: String,
        floor_multiplier: f64,
    },
    ShortWithLongFloor {
        short_horizon_id: String,
        long_horizon_id: String,
        floor_multiplier: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RealizedVolNoiseConfig {
    pub method: RealizedVolNoiseMethod,
}

impl RealizedVolNoiseConfig {
    pub fn none() -> Self {
        Self {
            method: RealizedVolNoiseMethod::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum RealizedVolNoiseMethod {
    None,
    CoarserGrid {
        coarse_sampling_interval_ms: u64,
        policy: RealizedVolCoarserGridPolicy,
    },
    Subsampled {
        subsamples: usize,
        min_ready_subsamples: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RealizedVolCoarserGridPolicy {
    CoarseOnly,
    MinBaseCoarse,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RealizedVolJumpConfig {
    pub policy: RealizedVolJumpPolicy,
}

impl RealizedVolJumpConfig {
    pub fn none() -> Self {
        Self {
            policy: RealizedVolJumpPolicy::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RealizedVolJumpPolicy {
    None,
    Separate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RealizedVolForecastConfig {
    pub method: RealizedVolForecastMethod,
}

impl RealizedVolForecastConfig {
    pub fn none() -> Self {
        Self {
            method: RealizedVolForecastMethod::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum RealizedVolForecastMethod {
    None,
    Ewma {
        alpha: f64,
    },
    HarLite {
        intercept: f64,
        short_weight: f64,
        medium_weight: f64,
        long_weight: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RealizedVolPricingComponent {
    Measured,
    NoiseRobust,
    Continuous,
    Forecast,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RealizedVolHorizonEstimate {
    pub horizon_id: String,
    pub measured_annualized_vol_decimal: Option<f64>,
    pub noise_robust_annualized_vol_decimal: Option<f64>,
    pub continuous_annualized_vol_decimal: Option<f64>,
    pub jump_annualized_vol_decimal: Option<f64>,
    pub grid_sample_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RealizedVolSourceConfig {
    pub source_id: String,
    pub data_client_id: String,
    pub instrument_id: String,
    pub source_class: RealizedVolSourceClass,
    pub sample_kind: RealizedVolSampleKind,
    pub enabled: bool,
    pub counts_toward_quorum: bool,
    pub canonical_base_asset: String,
    pub canonical_quote_asset: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RealizedVolSourceClass {
    SpotQuote,
    Trade,
    Mark,
    Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RealizedVolSampleKind {
    Midpoint,
    Trade,
    Mark,
    Index,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RealizedVolObservation {
    pub source_id: String,
    pub source_class: RealizedVolSourceClass,
    pub sample_kind: RealizedVolSampleKind,
    pub price: f64,
    pub event_ts_ms: u64,
    pub recv_ts_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RealizedVolSnapshot {
    pub surface_id: String,
    pub as_of_ms: u64,
    /// Latest receive timestamp among accepted observations which contribute to
    /// this surface. Pricing freshness is evaluated only against this watermark;
    /// `as_of_ms` remains the surface's venue-event-time coordinate for RV math
    /// and evidence.
    pub latest_accepted_receive_ms: Option<LocalReceiveMs>,
    pub annualized_realized_vol_decimal: Option<f64>,
    pub measured_annualized_realized_vol_decimal: Option<f64>,
    pub noise_robust_annualized_realized_vol_decimal: Option<f64>,
    pub continuous_annualized_realized_vol_decimal: Option<f64>,
    pub jump_annualized_realized_vol_decimal: Option<f64>,
    pub forecast_annualized_realized_vol_decimal: Option<f64>,
    pub pricing_component: RealizedVolPricingComponent,
    pub ready: bool,
    pub sources_used: Vec<String>,
    pub source_diagnostics: Vec<RealizedVolSourceDiagnostic>,
    pub horizon_estimates: Vec<RealizedVolHorizonEstimate>,
    pub unknown_source_rejections: BTreeMap<String, u64>,
    pub blocked_reasons: Vec<RealizedVolBlockReason>,
    pub aggregate_method: RealizedVolAggregation,
    pub seconds_per_annum: f64,
    pub config_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ValidRealizedVol(f64);

impl ValidRealizedVol {
    pub fn new(value: f64) -> Option<Self> {
        if value.is_finite() && value >= ZERO_F64 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ReadyRealizedVol(ValidRealizedVol);

impl ReadyRealizedVol {
    pub fn get(self) -> f64 {
        self.0.get()
    }
}

impl RealizedVolSnapshot {
    pub fn invalid_config(
        surface_id: &str,
        as_of_ms: u64,
        aggregate_method: RealizedVolAggregation,
        seconds_per_annum: f64,
        config_fingerprint: &str,
    ) -> Self {
        Self {
            surface_id: surface_id.to_string(),
            as_of_ms,
            latest_accepted_receive_ms: None,
            annualized_realized_vol_decimal: None,
            measured_annualized_realized_vol_decimal: None,
            noise_robust_annualized_realized_vol_decimal: None,
            continuous_annualized_realized_vol_decimal: None,
            jump_annualized_realized_vol_decimal: None,
            forecast_annualized_realized_vol_decimal: None,
            pricing_component: RealizedVolPricingComponent::Measured,
            ready: false,
            sources_used: Vec::new(),
            source_diagnostics: Vec::new(),
            horizon_estimates: Vec::new(),
            unknown_source_rejections: BTreeMap::new(),
            blocked_reasons: vec![RealizedVolBlockReason::InvalidConfig],
            aggregate_method,
            seconds_per_annum,
            config_fingerprint: config_fingerprint.to_string(),
        }
    }

    pub fn ready_realized_vol(&self) -> Option<ReadyRealizedVol> {
        if !self.ready || !self.blocked_reasons.is_empty() {
            return None;
        }

        self.annualized_realized_vol_decimal
            .and_then(ValidRealizedVol::new)
            .map(ReadyRealizedVol)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RealizedVolSourceDiagnostic {
    pub source_id: String,
    pub source_class: RealizedVolSourceClass,
    pub sample_kind: RealizedVolSampleKind,
    pub enabled: bool,
    pub counts_toward_quorum: bool,
    pub status: RealizedVolSourceStatus,
    pub annualized_realized_vol_decimal: Option<f64>,
    pub measured_annualized_realized_vol_decimal: Option<f64>,
    pub noise_robust_annualized_realized_vol_decimal: Option<f64>,
    pub continuous_annualized_realized_vol_decimal: Option<f64>,
    pub jump_annualized_realized_vol_decimal: Option<f64>,
    pub first_sample_ts_ms: Option<u64>,
    pub last_sample_ts_ms: Option<u64>,
    pub raw_sample_count: usize,
    pub grid_sample_count: usize,
    pub coverage_ratio: f64,
    pub max_inter_sample_gap_ms: Option<u64>,
    pub last_rejected_reason: Option<RealizedVolSourceRejectReason>,
    pub last_rejected_event_ts_ms: Option<u64>,
    pub last_rejected_recv_ts_ms: Option<u64>,
    pub rejection_counters: BTreeMap<RealizedVolSourceRejectReason, u64>,
    pub block_reason: Option<RealizedVolBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealizedVolSourceStatus {
    Ready,
    Blocked,
    DiagnosticOnly,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RealizedVolSourceRejectReason {
    DisabledSource,
    InvalidPrice,
    SourceClassMismatch,
    SampleKindMismatch,
    EventTimeRegression,
    DuplicateTimestamp,
    StaleSameEventUpdate,
    // Retained for historical evidence deserialization. Live ingest no longer rejects
    // observations solely because receive time precedes or lags venue event time.
    ReceiveBeforeEvent,
    EventReceiveLagExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RealizedVolBlockReason {
    InvalidConfig,
    ProviderCapabilityUnavailable,
    QuorumNotReady,
    SourceStale,
    CoverageBelowMinimum,
    InterSampleGapExceeded,
    SourceClassMismatch,
    SampleKindMismatch,
    CrossSourceDispersion,
    AnnualizationBasisInvalid,
    NotWarm,
}

impl RealizedVolBlockReason {
    pub const ALL: &'static [Self] = &[
        Self::InvalidConfig,
        Self::ProviderCapabilityUnavailable,
        Self::QuorumNotReady,
        Self::SourceStale,
        Self::CoverageBelowMinimum,
        Self::InterSampleGapExceeded,
        Self::SourceClassMismatch,
        Self::SampleKindMismatch,
        Self::CrossSourceDispersion,
        Self::AnnualizationBasisInvalid,
        Self::NotWarm,
    ];
}

#[derive(Debug, Clone, PartialEq)]
pub struct RealizedVolEngine {
    config: RealizedVolEngineConfig,
    sources: BTreeMap<String, SourceState>,
    unknown_source_rejections: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq)]
struct SourceState {
    config: RealizedVolSourceConfig,
    samples: VecDeque<RealizedVolObservation>,
    last_rejected_reason: Option<RealizedVolSourceRejectReason>,
    last_rejected_event_ts_ms: Option<u64>,
    last_rejected_recv_ts_ms: Option<u64>,
    rejection_counters: BTreeMap<RealizedVolSourceRejectReason, u64>,
}

impl RealizedVolEngine {
    pub fn from_config(config: RealizedVolEngineConfig) -> Result<Self, String> {
        validate_config(&config)?;
        let sources = config
            .sources
            .iter()
            .cloned()
            .map(|source| {
                (
                    source.source_id.clone(),
                    SourceState {
                        config: source,
                        samples: VecDeque::new(),
                        last_rejected_reason: None,
                        last_rejected_event_ts_ms: None,
                        last_rejected_recv_ts_ms: None,
                        rejection_counters: BTreeMap::new(),
                    },
                )
            })
            .collect();
        Ok(Self {
            config,
            sources,
            unknown_source_rejections: BTreeMap::new(),
        })
    }

    pub fn config(&self) -> &RealizedVolEngineConfig {
        &self.config
    }

    pub fn latest_accepted_event_ts(&self) -> Option<VenueEventMs> {
        self.sources
            .values()
            .filter_map(|state| {
                state
                    .samples
                    .back()
                    .map(|sample| VenueEventMs::new(sample.event_ts_ms))
            })
            .max()
    }

    pub fn observe(&mut self, observation: RealizedVolObservation) -> bool {
        let Some(source) = self.sources.get_mut(&observation.source_id) else {
            increment_counter(&mut self.unknown_source_rejections, observation.source_id);
            return false;
        };
        let rejected = reject_observation(&source.config, &source.samples, &observation);
        if let Some(reason) = rejected {
            source.last_rejected_reason = Some(reason);
            source.last_rejected_event_ts_ms = Some(observation.event_ts_ms);
            source.last_rejected_recv_ts_ms = Some(observation.recv_ts_ms);
            increment_counter(&mut source.rejection_counters, reason);
            return false;
        }
        let event_ts_ms = observation.event_ts_ms;
        if source
            .samples
            .back()
            .is_some_and(|sample| sample.event_ts_ms == event_ts_ms)
        {
            let _ = source.samples.pop_back();
        }
        source.samples.push_back(observation);
        prune_source_samples(
            &mut source.samples,
            event_ts_ms.saturating_sub(
                self.config
                    .window_ms
                    .saturating_add(self.config.max_source_age_ms)
                    .saturating_add(self.config.sampling_interval_ms),
            ),
        );
        true
    }

    pub fn snapshot_at(&self, as_of_ms: u64) -> RealizedVolSnapshot {
        let mut diagnostics = Vec::new();
        let mut ready_values = Vec::new();
        let mut blockers = BTreeSet::new();
        if !is_positive_finite(self.config.seconds_per_annum) {
            blockers.insert(RealizedVolBlockReason::AnnualizationBasisInvalid);
        }
        for state in self.sources.values() {
            let (diagnostic, latest_used_receive_ms) =
                source_diagnostic(&self.config, state, as_of_ms);
            if let (true, RealizedVolSourceStatus::Ready, true, Some(value), _) = (
                state.config.enabled,
                diagnostic.status,
                state.config.counts_toward_quorum,
                diagnostic.annualized_realized_vol_decimal,
                diagnostic.block_reason,
            ) {
                ready_values.push(ReadySourceValue {
                    source_id: diagnostic.source_id.clone(),
                    source_class: diagnostic.source_class,
                    sample_kind: diagnostic.sample_kind,
                    final_value: value,
                    measured: diagnostic.measured_annualized_realized_vol_decimal,
                    noise_robust: diagnostic.noise_robust_annualized_realized_vol_decimal,
                    continuous: diagnostic.continuous_annualized_realized_vol_decimal,
                    jump: diagnostic.jump_annualized_realized_vol_decimal,
                    latest_accepted_receive_ms: latest_used_receive_ms
                        .expect("a ready source must contain an accepted sample"),
                });
            }
            diagnostics.push(diagnostic);
        }

        if ready_values.len() < self.config.min_ready_sources {
            blockers.insert(RealizedVolBlockReason::QuorumNotReady);
        }
        if ready_values_have_mismatched_classes(&ready_values) {
            blockers.insert(RealizedVolBlockReason::SourceClassMismatch);
        }
        if ready_values_have_mismatched_sample_kinds(&ready_values) {
            blockers.insert(RealizedVolBlockReason::SampleKindMismatch);
        }
        let aggregate = upper_quantile(&ready_values, self.config.aggregation);
        let measured_aggregate =
            aggregate_component(&ready_values, self.config.aggregation, |value| {
                value.measured
            });
        let noise_robust_aggregate =
            aggregate_component(&ready_values, self.config.aggregation, |value| {
                value.noise_robust
            });
        let continuous_aggregate =
            aggregate_component(&ready_values, self.config.aggregation, |value| {
                value.continuous
            });
        let jump_aggregate =
            aggregate_component(&ready_values, self.config.aggregation, |value| value.jump);
        match aggregate {
            Some(value)
                if dispersion(&ready_values, value) > self.config.max_cross_source_dispersion =>
            {
                blockers.insert(RealizedVolBlockReason::CrossSourceDispersion);
            }
            _ => {}
        }
        let ready = blockers.is_empty() && aggregate.is_some();
        let latest_accepted_receive_ms = ready_values
            .iter()
            .map(|value| value.latest_accepted_receive_ms)
            .max();
        RealizedVolSnapshot {
            surface_id: self.config.surface_id.clone(),
            as_of_ms,
            latest_accepted_receive_ms,
            annualized_realized_vol_decimal: if ready { aggregate } else { None },
            measured_annualized_realized_vol_decimal: if ready { measured_aggregate } else { None },
            noise_robust_annualized_realized_vol_decimal: if ready {
                noise_robust_aggregate
            } else {
                None
            },
            continuous_annualized_realized_vol_decimal: if ready {
                continuous_aggregate
            } else {
                None
            },
            jump_annualized_realized_vol_decimal: if ready { jump_aggregate } else { None },
            forecast_annualized_realized_vol_decimal: None,
            pricing_component: self.config.estimator.pricing_component,
            ready,
            sources_used: if ready {
                ready_values
                    .iter()
                    .map(|value| value.source_id.clone())
                    .collect()
            } else {
                Vec::new()
            },
            source_diagnostics: diagnostics,
            horizon_estimates: Vec::new(),
            unknown_source_rejections: self.unknown_source_rejections.clone(),
            blocked_reasons: blockers.into_iter().collect(),
            aggregate_method: self.config.aggregation,
            seconds_per_annum: self.config.seconds_per_annum,
            config_fingerprint: config_fingerprint(&self.config),
        }
    }
}

fn validate_config(config: &RealizedVolEngineConfig) -> Result<(), String> {
    if config.surface_id.trim().is_empty() {
        return Err("surface_id must be non-empty".to_string());
    }
    if config.sources.is_empty() {
        return Err("realized_volatility source list must be non-empty".to_string());
    }
    if config.window_ms == 0 || config.sampling_interval_ms == 0 || config.min_ready_sources == 0 {
        return Err("realized_volatility policy integers must be positive".to_string());
    }
    if config.sampling_interval_ms > config.window_ms {
        return Err("window_ms must be greater than or equal to sampling_interval_ms".to_string());
    }
    if !is_positive_finite(config.seconds_per_annum) {
        return Err("seconds_per_annum must be positive finite".to_string());
    }
    if !config.min_coverage_ratio.is_finite()
        || config.min_coverage_ratio <= ZERO_F64
        || config.min_coverage_ratio > UNIT_F64
    {
        return Err("min_coverage_ratio must be in (0, 1]".to_string());
    }
    if !config.max_cross_source_dispersion.is_finite()
        || config.max_cross_source_dispersion < ZERO_F64
    {
        return Err("max_cross_source_dispersion must be finite and non-negative".to_string());
    }
    match config.aggregation {
        RealizedVolAggregation::UpperQuantile { quantile }
            if !(HALF_F64..=UNIT_F64).contains(&quantile) =>
        {
            return Err("upper_quantile must be in [0.5, 1.0]".to_string());
        }
        RealizedVolAggregation::UpperQuantile { .. } => {}
        RealizedVolAggregation::Median => {}
        RealizedVolAggregation::TrimmedMean { trim_fraction }
            if !trim_fraction.is_finite() || !(ZERO_F64..HALF_F64).contains(&trim_fraction) =>
        {
            return Err("trim_fraction must be finite and in [0, 0.5)".to_string());
        }
        RealizedVolAggregation::TrimmedMean { .. } => {}
        RealizedVolAggregation::MedianWithUpperQuantileGuard {
            upper_quantile,
            guard_weight,
        } if !(HALF_F64..=UNIT_F64).contains(&upper_quantile)
            || !guard_weight.is_finite()
            || !(ZERO_F64..=UNIT_F64).contains(&guard_weight) =>
        {
            return Err(
                "median_with_upper_quantile_guard requires upper_quantile in [0.5, 1.0] and guard_weight in [0, 1]"
                    .to_string(),
            );
        }
        RealizedVolAggregation::MedianWithUpperQuantileGuard { .. } => {}
    }
    match config.estimator.noise.method {
        RealizedVolNoiseMethod::None => {}
        RealizedVolNoiseMethod::CoarserGrid {
            coarse_sampling_interval_ms,
            ..
        } if coarse_sampling_interval_ms <= config.sampling_interval_ms => {
            return Err(
                "coarser_grid coarse_sampling_interval_ms must be greater than sampling_interval_ms"
                    .to_string(),
            );
        }
        RealizedVolNoiseMethod::CoarserGrid { .. } => {}
        RealizedVolNoiseMethod::Subsampled {
            subsamples,
            min_ready_subsamples,
        } if subsamples == 0 || min_ready_subsamples == 0 => {
            return Err(
                "subsampled RV requires positive subsamples and min_ready_subsamples".to_string(),
            );
        }
        RealizedVolNoiseMethod::Subsampled {
            subsamples,
            min_ready_subsamples,
        } if min_ready_subsamples > subsamples => {
            return Err(
                "subsampled RV min_ready_subsamples must be less than or equal to subsamples"
                    .to_string(),
            );
        }
        RealizedVolNoiseMethod::Subsampled { .. } => {}
    }
    if matches!(
        config.estimator.pricing_component,
        RealizedVolPricingComponent::NoiseRobust
    ) && matches!(config.estimator.noise.method, RealizedVolNoiseMethod::None)
    {
        return Err(
            "pricing_component noise_robust requires a noise_robust_method other than none"
                .to_string(),
        );
    }
    if matches!(
        config.estimator.pricing_component,
        RealizedVolPricingComponent::Continuous
    ) && !matches!(
        config.estimator.jump.policy,
        RealizedVolJumpPolicy::Separate
    ) {
        return Err("pricing_component continuous requires jump_policy separate".to_string());
    }
    if matches!(
        config.estimator.pricing_component,
        RealizedVolPricingComponent::Forecast
    ) || !matches!(
        config.estimator.forecast.method,
        RealizedVolForecastMethod::None
    ) {
        return Err("forecast RV is not enabled in this implementation slice".to_string());
    }
    let mut ids = BTreeSet::new();
    let mut enabled_quorum_sources = 0usize;
    let mut quorum_contract: Option<(RealizedVolSourceClass, RealizedVolSampleKind)> = None;
    for source in &config.sources {
        if source.source_id.trim().is_empty() {
            return Err("source_id must be non-empty".to_string());
        }
        if !ids.insert(source.source_id.clone()) {
            return Err(format!("duplicate source_id `{}`", source.source_id));
        }
        if source.enabled && source.counts_toward_quorum {
            enabled_quorum_sources += 1;
            match quorum_contract {
                Some((source_class, sample_kind))
                    if source.source_class != source_class || source.sample_kind != sample_kind =>
                {
                    return Err(
                        "enabled quorum-counting sources must share source_class and sample_kind"
                            .to_string(),
                    );
                }
                Some(_) => {}
                None => quorum_contract = Some((source.source_class, source.sample_kind)),
            }
        }
    }
    if config.min_ready_sources > enabled_quorum_sources {
        return Err("min_ready_sources exceeds enabled quorum source count".to_string());
    }
    Ok(())
}

fn reject_observation(
    config: &RealizedVolSourceConfig,
    samples: &VecDeque<RealizedVolObservation>,
    observation: &RealizedVolObservation,
) -> Option<RealizedVolSourceRejectReason> {
    if !config.enabled {
        return Some(RealizedVolSourceRejectReason::DisabledSource);
    }
    if observation.source_class != config.source_class {
        return Some(RealizedVolSourceRejectReason::SourceClassMismatch);
    }
    if observation.sample_kind != config.sample_kind {
        return Some(RealizedVolSourceRejectReason::SampleKindMismatch);
    }
    if !is_positive_finite(observation.price) {
        return Some(RealizedVolSourceRejectReason::InvalidPrice);
    }
    let observed_event_ts = VenueEventMs::new(observation.event_ts_ms);
    if samples
        .back()
        .is_some_and(|sample| observed_event_ts < VenueEventMs::new(sample.event_ts_ms))
    {
        return Some(RealizedVolSourceRejectReason::EventTimeRegression);
    }
    if let Some(sample) = samples
        .back()
        .filter(|sample| observed_event_ts == VenueEventMs::new(sample.event_ts_ms))
    {
        let observed_receive_ts = LocalReceiveMs::new(observation.recv_ts_ms);
        let prior_receive_ts = LocalReceiveMs::new(sample.recv_ts_ms);
        if observed_receive_ts == prior_receive_ts {
            return Some(RealizedVolSourceRejectReason::DuplicateTimestamp);
        }
        if observed_receive_ts < prior_receive_ts {
            return Some(RealizedVolSourceRejectReason::StaleSameEventUpdate);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
struct SourceComputation {
    rv: Option<f64>,
    measured_rv: Option<f64>,
    noise_robust_rv: Option<f64>,
    continuous_rv: Option<f64>,
    jump_rv: Option<f64>,
    latest_used_receive_ms: Option<LocalReceiveMs>,
    block_reason: Option<RealizedVolBlockReason>,
}

#[derive(Debug, Clone, Copy)]
struct GridPoint<'a> {
    ts_ms: u64,
    sample: &'a RealizedVolObservation,
}

#[derive(Debug, Clone, PartialEq)]
struct ReadySourceValue {
    source_id: String,
    source_class: RealizedVolSourceClass,
    sample_kind: RealizedVolSampleKind,
    final_value: f64,
    measured: Option<f64>,
    noise_robust: Option<f64>,
    continuous: Option<f64>,
    jump: Option<f64>,
    latest_accepted_receive_ms: LocalReceiveMs,
}

fn source_diagnostic(
    config: &RealizedVolEngineConfig,
    state: &SourceState,
    as_of_ms: u64,
) -> (RealizedVolSourceDiagnostic, Option<LocalReceiveMs>) {
    let window_start_ms = as_of_ms.saturating_sub(config.window_ms);
    let samples = state
        .samples
        .iter()
        .filter(|sample| sample.event_ts_ms <= as_of_ms)
        .collect::<Vec<_>>();
    let grid = grid_prices(config, &samples, window_start_ms, as_of_ms);
    let expected_grid_count = (config.window_ms / config.sampling_interval_ms) as usize;
    let coverage_ratio = if expected_grid_count == 0 {
        ZERO_F64
    } else {
        grid.len() as f64 / expected_grid_count as f64
    };
    let max_gap = grid
        .windows(2)
        .map(|pair| pair[1].ts_ms.saturating_sub(pair[0].ts_ms))
        .max();
    let computation = compute_rv(config, &samples, &grid, coverage_ratio, max_gap, as_of_ms);
    let window_samples = samples
        .iter()
        .copied()
        .filter(|sample| sample.event_ts_ms >= window_start_ms)
        .collect::<Vec<_>>();
    let rejection_block_reason = match state.last_rejected_reason {
        Some(RealizedVolSourceRejectReason::SourceClassMismatch) => {
            Some(RealizedVolBlockReason::SourceClassMismatch)
        }
        Some(RealizedVolSourceRejectReason::SampleKindMismatch) => {
            Some(RealizedVolBlockReason::SampleKindMismatch)
        }
        _ => None,
    };
    let status = source_status(&state.config, &state.samples, &computation);
    let block_reason = if status == RealizedVolSourceStatus::Ready {
        None
    } else {
        rejection_block_reason.or(computation.block_reason)
    };
    let latest_used_receive_ms = computation.latest_used_receive_ms;
    let diagnostic = RealizedVolSourceDiagnostic {
        source_id: state.config.source_id.clone(),
        source_class: state.config.source_class,
        sample_kind: state.config.sample_kind,
        enabled: state.config.enabled,
        counts_toward_quorum: state.config.counts_toward_quorum,
        status,
        annualized_realized_vol_decimal: computation.rv,
        measured_annualized_realized_vol_decimal: computation.measured_rv,
        noise_robust_annualized_realized_vol_decimal: computation.noise_robust_rv,
        continuous_annualized_realized_vol_decimal: computation.continuous_rv,
        jump_annualized_realized_vol_decimal: computation.jump_rv,
        first_sample_ts_ms: window_samples.first().map(|sample| sample.event_ts_ms),
        last_sample_ts_ms: samples.last().map(|sample| sample.event_ts_ms),
        raw_sample_count: window_samples.len(),
        grid_sample_count: grid.len(),
        coverage_ratio,
        max_inter_sample_gap_ms: max_gap,
        last_rejected_reason: state.last_rejected_reason,
        last_rejected_event_ts_ms: state.last_rejected_event_ts_ms,
        last_rejected_recv_ts_ms: state.last_rejected_recv_ts_ms,
        rejection_counters: state.rejection_counters.clone(),
        block_reason,
    };
    (diagnostic, latest_used_receive_ms)
}

fn source_status(
    config: &RealizedVolSourceConfig,
    samples: &VecDeque<RealizedVolObservation>,
    computation: &SourceComputation,
) -> RealizedVolSourceStatus {
    if !config.enabled || !config.counts_toward_quorum {
        return RealizedVolSourceStatus::DiagnosticOnly;
    }
    if computation.rv.is_some() {
        return RealizedVolSourceStatus::Ready;
    }
    if samples.is_empty() {
        return RealizedVolSourceStatus::Waiting;
    }
    RealizedVolSourceStatus::Blocked
}

fn grid_prices<'a>(
    config: &RealizedVolEngineConfig,
    samples: &[&'a RealizedVolObservation],
    window_start_ms: u64,
    as_of_ms: u64,
) -> Vec<GridPoint<'a>> {
    let mut out = Vec::new();
    let mut latest = None;
    let mut index = 0;
    let mut ts = window_start_ms.saturating_add(config.sampling_interval_ms);
    while ts <= as_of_ms {
        while index < samples.len() && samples[index].event_ts_ms <= ts {
            latest = Some(samples[index]);
            index += 1;
        }
        match latest {
            Some(sample) if ts.saturating_sub(sample.event_ts_ms) <= config.max_source_age_ms => {
                out.push(GridPoint { ts_ms: ts, sample });
            }
            _ => {}
        }
        let Some(next_ts) = ts.checked_add(config.sampling_interval_ms) else {
            break;
        };
        if next_ts <= ts {
            break;
        }
        ts = next_ts;
    }
    out
}

fn compute_rv(
    config: &RealizedVolEngineConfig,
    samples: &[&RealizedVolObservation],
    grid: &[GridPoint<'_>],
    coverage_ratio: f64,
    max_gap: Option<u64>,
    as_of_ms: u64,
) -> SourceComputation {
    let Some(last_grid_point) = grid.last() else {
        return blocked_computation(RealizedVolBlockReason::NotWarm);
    };
    if as_of_ms.saturating_sub(last_grid_point.ts_ms) > config.max_source_age_ms {
        return blocked_computation(RealizedVolBlockReason::SourceStale);
    }
    if grid.len() < 2 {
        return blocked_computation(RealizedVolBlockReason::NotWarm);
    }
    if coverage_ratio < config.min_coverage_ratio {
        return blocked_computation(RealizedVolBlockReason::CoverageBelowMinimum);
    }
    if max_gap.is_some_and(|gap| gap > config.max_inter_sample_gap_ms) {
        return blocked_computation(RealizedVolBlockReason::InterSampleGapExceeded);
    }
    if !is_positive_finite(config.seconds_per_annum) {
        return blocked_computation(RealizedVolBlockReason::AnnualizationBasisInvalid);
    }
    let Some(measured) = variance_from_grid(config, grid) else {
        return blocked_computation(RealizedVolBlockReason::NotWarm);
    };
    let Some((noise_robust_variance, noise_latest_receive_ms)) =
        noise_robust_variance(config, samples, grid, measured)
    else {
        return blocked_computation(RealizedVolBlockReason::NotWarm);
    };
    let (continuous_variance, jump_variance) = match config.estimator.jump.policy {
        RealizedVolJumpPolicy::None => (measured, ZERO_F64),
        RealizedVolJumpPolicy::Separate => {
            let Some(bipower_variance) = bipower_variation(config, grid) else {
                return blocked_computation(RealizedVolBlockReason::NotWarm);
            };
            let continuous = measured.min(bipower_variance);
            let jump = (measured - continuous).max(ZERO_F64);
            (continuous, jump)
        }
    };
    let Some(measured_rv) = valid_vol_from_variance(measured) else {
        return blocked_computation(RealizedVolBlockReason::AnnualizationBasisInvalid);
    };
    let Some(noise_robust_rv) = valid_vol_from_variance(noise_robust_variance) else {
        return blocked_computation(RealizedVolBlockReason::AnnualizationBasisInvalid);
    };
    let Some(continuous_rv) = valid_vol_from_variance(continuous_variance) else {
        return blocked_computation(RealizedVolBlockReason::AnnualizationBasisInvalid);
    };
    let Some(jump_rv) = valid_vol_from_variance(jump_variance) else {
        return blocked_computation(RealizedVolBlockReason::AnnualizationBasisInvalid);
    };
    let final_rv = match config.estimator.pricing_component {
        RealizedVolPricingComponent::Measured => measured_rv,
        RealizedVolPricingComponent::NoiseRobust => noise_robust_rv,
        RealizedVolPricingComponent::Continuous => continuous_rv,
        // Unreachable for engines built through from_config: forecast pricing is future scope and
        // validation rejects it. Keep a deterministic value here so private computation stays total.
        RealizedVolPricingComponent::Forecast => measured_rv,
    };
    SourceComputation {
        rv: Some(final_rv),
        measured_rv: Some(measured_rv),
        noise_robust_rv: Some(noise_robust_rv),
        continuous_rv: Some(continuous_rv),
        jump_rv: Some(jump_rv),
        latest_used_receive_ms: grid
            .iter()
            .map(|point| LocalReceiveMs::new(point.sample.recv_ts_ms))
            .chain(std::iter::once(noise_latest_receive_ms))
            .max(),
        block_reason: None,
    }
}

fn blocked_computation(reason: RealizedVolBlockReason) -> SourceComputation {
    SourceComputation {
        rv: None,
        measured_rv: None,
        noise_robust_rv: None,
        continuous_rv: None,
        jump_rv: None,
        latest_used_receive_ms: None,
        block_reason: Some(reason),
    }
}

fn valid_vol_from_variance(variance: f64) -> Option<f64> {
    ValidRealizedVol::new(variance.sqrt()).map(ValidRealizedVol::get)
}

fn variance_from_grid(config: &RealizedVolEngineConfig, grid: &[GridPoint<'_>]) -> Option<f64> {
    let mut sum = ZERO_F64;
    let mut elapsed_ms = 0;
    for pair in grid.windows(2) {
        let dt = pair[1].ts_ms.saturating_sub(pair[0].ts_ms);
        let log_return = (pair[1].sample.price / pair[0].sample.price).ln();
        if dt == ZERO_MILLIS_U64 || !log_return.is_finite() {
            return None;
        }
        sum += log_return.powi(POWER_OF_TWO);
        elapsed_ms += dt;
    }
    let elapsed_seconds = elapsed_ms as f64 / MILLIS_PER_SECOND_F64;
    Some((sum / elapsed_seconds) * config.seconds_per_annum)
}

fn noise_robust_variance(
    config: &RealizedVolEngineConfig,
    samples: &[&RealizedVolObservation],
    grid: &[GridPoint<'_>],
    measured_variance: f64,
) -> Option<(f64, LocalReceiveMs)> {
    let base_latest_receive_ms = grid
        .iter()
        .map(|point| LocalReceiveMs::new(point.sample.recv_ts_ms))
        .max()?;
    match &config.estimator.noise.method {
        RealizedVolNoiseMethod::None => Some((measured_variance, base_latest_receive_ms)),
        RealizedVolNoiseMethod::CoarserGrid {
            coarse_sampling_interval_ms,
            policy,
        } => {
            if *coarse_sampling_interval_ms == ZERO_MILLIS_U64 {
                return None;
            }
            let (Some(first_grid_point), Some(last_grid_point)) = (grid.first(), grid.last())
            else {
                return None;
            };
            let coarse_grid = grid_prices_with_interval(
                *coarse_sampling_interval_ms,
                config.max_source_age_ms,
                samples,
                first_grid_point
                    .ts_ms
                    .saturating_sub(*coarse_sampling_interval_ms),
                last_grid_point.ts_ms,
            );
            let coarse = variance_from_grid(config, &coarse_grid)?;
            let latest_receive_ms = coarse_grid
                .iter()
                .map(|point| LocalReceiveMs::new(point.sample.recv_ts_ms))
                .chain(std::iter::once(base_latest_receive_ms))
                .max()?;
            let variance = match policy {
                RealizedVolCoarserGridPolicy::CoarseOnly => coarse,
                RealizedVolCoarserGridPolicy::MinBaseCoarse => measured_variance.min(coarse),
            };
            Some((variance, latest_receive_ms))
        }
        RealizedVolNoiseMethod::Subsampled {
            subsamples,
            min_ready_subsamples,
        } => {
            if *subsamples == ZERO_COUNT_USIZE || *min_ready_subsamples == ZERO_COUNT_USIZE {
                return None;
            }
            let (Some(first_grid_point), Some(last_grid_point)) = (grid.first(), grid.last())
            else {
                return None;
            };
            let window_start_ms = first_grid_point
                .ts_ms
                .saturating_sub(config.sampling_interval_ms);
            let mut variances = Vec::new();
            let mut latest_receive_ms = base_latest_receive_ms;
            for offset in ZERO_COUNT_USIZE..*subsamples {
                let offset_ms = ((config.sampling_interval_ms as u128 * offset as u128)
                    / *subsamples as u128) as u64;
                let lane = grid_prices_with_interval(
                    config.sampling_interval_ms,
                    config.max_source_age_ms,
                    samples,
                    window_start_ms.saturating_add(offset_ms),
                    last_grid_point.ts_ms,
                );
                if lane.len() < POWER_OF_TWO as usize {
                    continue;
                }
                if let Some(variance) = variance_from_grid(config, &lane) {
                    variances.push(variance);
                    if let Some(lane_latest_receive_ms) = lane
                        .iter()
                        .map(|point| LocalReceiveMs::new(point.sample.recv_ts_ms))
                        .max()
                    {
                        latest_receive_ms = latest_receive_ms.max(lane_latest_receive_ms);
                    }
                }
            }
            if variances.len() < *min_ready_subsamples {
                return None;
            }
            Some((
                variances.iter().sum::<f64>() / variances.len() as f64,
                latest_receive_ms,
            ))
        }
    }
}

fn grid_prices_with_interval<'a>(
    sampling_interval_ms: u64,
    max_source_age_ms: u64,
    samples: &[&'a RealizedVolObservation],
    window_start_ms: u64,
    as_of_ms: u64,
) -> Vec<GridPoint<'a>> {
    let mut out = Vec::new();
    let mut latest = None;
    let mut index = 0;
    let mut ts = window_start_ms.saturating_add(sampling_interval_ms);
    while ts <= as_of_ms {
        while index < samples.len() && samples[index].event_ts_ms <= ts {
            latest = Some(samples[index]);
            index += 1;
        }
        if let Some(sample) = latest
            && ts.saturating_sub(sample.event_ts_ms) <= max_source_age_ms
        {
            out.push(GridPoint { ts_ms: ts, sample });
        }
        let Some(next_ts) = ts.checked_add(sampling_interval_ms) else {
            break;
        };
        if next_ts <= ts {
            break;
        }
        ts = next_ts;
    }
    out
}

fn bipower_variation(config: &RealizedVolEngineConfig, grid: &[GridPoint<'_>]) -> Option<f64> {
    let mut returns = Vec::new();
    let mut elapsed_ms = 0;
    for pair in grid.windows(2) {
        let dt = pair[1].ts_ms.saturating_sub(pair[0].ts_ms);
        let log_return = (pair[1].sample.price / pair[0].sample.price).ln();
        if dt == ZERO_MILLIS_U64 || !log_return.is_finite() {
            return None;
        }
        elapsed_ms += dt;
        returns.push(log_return);
    }
    if returns.len() < POWER_OF_TWO as usize {
        return None;
    }
    let adjacent_abs_products = returns
        .windows(POWER_OF_TWO as usize)
        .map(|pair| {
            let [left, right] = pair else {
                return ZERO_F64;
            };
            left.abs() * right.abs()
        })
        .sum::<f64>();
    let n = returns.len() as f64;
    let mu_one_squared = crate::bolt_v3_numeric::TWO_F64 / std::f64::consts::PI;
    let finite_sample = n / (n - UNIT_F64);
    let elapsed_seconds = elapsed_ms as f64 / MILLIS_PER_SECOND_F64;
    Some(
        (adjacent_abs_products / mu_one_squared) * finite_sample / elapsed_seconds
            * config.seconds_per_annum,
    )
}

fn ready_values_have_mismatched_classes(values: &[ReadySourceValue]) -> bool {
    values.first().is_some_and(|first| {
        values
            .iter()
            .any(|value| value.source_class != first.source_class)
    })
}

fn ready_values_have_mismatched_sample_kinds(values: &[ReadySourceValue]) -> bool {
    values.first().is_some_and(|first| {
        values
            .iter()
            .any(|value| value.sample_kind != first.sample_kind)
    })
}

fn upper_quantile(values: &[ReadySourceValue], aggregation: RealizedVolAggregation) -> Option<f64> {
    let mut sorted = values
        .iter()
        .map(|value| value.final_value)
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    match aggregation {
        RealizedVolAggregation::UpperQuantile { quantile } => {
            let index = ((quantile * sorted.len() as f64).ceil() as usize)
                .saturating_sub(1)
                .min(sorted.len().saturating_sub(1));
            sorted.get(index).copied()
        }
        RealizedVolAggregation::Median => median_sorted(&sorted),
        RealizedVolAggregation::TrimmedMean { trim_fraction } => {
            if sorted.is_empty() {
                return None;
            }
            let trim_count = (trim_fraction * sorted.len() as f64).floor() as usize;
            let start = trim_count.min(sorted.len());
            let end = sorted.len().saturating_sub(trim_count);
            if start >= end {
                return None;
            }
            let trimmed = &sorted[start..end];
            Some(trimmed.iter().sum::<f64>() / trimmed.len() as f64)
        }
        RealizedVolAggregation::MedianWithUpperQuantileGuard {
            upper_quantile,
            guard_weight,
        } => {
            let median = median_sorted(&sorted)?;
            let guard_index = ((upper_quantile * sorted.len() as f64).ceil() as usize)
                .saturating_sub(1)
                .min(sorted.len().saturating_sub(1));
            let guard = *sorted.get(guard_index)?;
            Some(median.mul_add(UNIT_F64 - guard_weight, guard * guard_weight))
        }
    }
}

fn median_sorted(sorted: &[f64]) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let divisor = POWER_OF_TWO as usize;
    let mid = sorted.len() / divisor;
    if sorted.len() % divisor == ZERO_COUNT_USIZE {
        let left = sorted[..mid].last().copied()?;
        let right = sorted.get(mid).copied()?;
        Some((left + right) / POWER_OF_TWO as f64)
    } else {
        sorted.get(mid).copied()
    }
}

fn aggregate_component(
    values: &[ReadySourceValue],
    aggregation: RealizedVolAggregation,
    component: impl Fn(&ReadySourceValue) -> Option<f64>,
) -> Option<f64> {
    let component_values = values
        .iter()
        .filter_map(|value| {
            component(value).map(|component| ReadySourceValue {
                source_id: value.source_id.clone(),
                source_class: value.source_class,
                sample_kind: value.sample_kind,
                final_value: component,
                measured: None,
                noise_robust: None,
                continuous: None,
                jump: None,
                latest_accepted_receive_ms: value.latest_accepted_receive_ms,
            })
        })
        .collect::<Vec<_>>();
    if component_values.len() == values.len() {
        upper_quantile(&component_values, aggregation)
    } else {
        None
    }
}

fn dispersion(values: &[ReadySourceValue], aggregate: f64) -> f64 {
    if values.len() < 2 {
        return ZERO_F64;
    }
    let mut sorted = values
        .iter()
        .map(|value| value.final_value)
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let range = sorted[sorted.len() - 1] - sorted[0];
    if range <= ZERO_F64 {
        return ZERO_F64;
    }
    if !aggregate.is_finite() || aggregate <= ZERO_F64 {
        return f64::INFINITY;
    }
    range / aggregate
}

fn prune_source_samples(samples: &mut VecDeque<RealizedVolObservation>, min_event_ts_ms: u64) {
    while samples
        .front()
        .is_some_and(|sample| sample.event_ts_ms < min_event_ts_ms)
    {
        let _ = samples.pop_front();
    }
}

fn increment_counter<K: Ord>(counters: &mut BTreeMap<K, u64>, key: K) {
    let count = counters.entry(key).or_insert(INITIAL_REJECTION_COUNT);
    *count = count.saturating_add(COUNTER_INCREMENT_U64);
}

fn config_fingerprint(config: &RealizedVolEngineConfig) -> String {
    let mut sources = config.sources.clone();
    sources.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    let mut canonical = String::new();
    writeln!(&mut canonical, "surface_id={}", config.surface_id)
        .expect("canonical fingerprint write should not fail");
    writeln!(&mut canonical, "window_ms={}", config.window_ms)
        .expect("canonical fingerprint write should not fail");
    writeln!(
        &mut canonical,
        "sampling_interval_ms={}",
        config.sampling_interval_ms
    )
    .expect("canonical fingerprint write should not fail");
    writeln!(
        &mut canonical,
        "min_ready_sources={}",
        config.min_ready_sources
    )
    .expect("canonical fingerprint write should not fail");
    writeln!(
        &mut canonical,
        "max_source_age_ms={}",
        config.max_source_age_ms
    )
    .expect("canonical fingerprint write should not fail");
    writeln!(
        &mut canonical,
        "max_inter_sample_gap_ms={}",
        config.max_inter_sample_gap_ms
    )
    .expect("canonical fingerprint write should not fail");
    writeln!(
        &mut canonical,
        "min_coverage_ratio={}",
        canonical_f64(config.min_coverage_ratio)
    )
    .expect("canonical fingerprint write should not fail");
    writeln!(
        &mut canonical,
        "max_cross_source_dispersion={}",
        canonical_f64(config.max_cross_source_dispersion)
    )
    .expect("canonical fingerprint write should not fail");
    writeln!(
        &mut canonical,
        "seconds_per_annum={}",
        canonical_f64(config.seconds_per_annum)
    )
    .expect("canonical fingerprint write should not fail");
    match config.aggregation {
        RealizedVolAggregation::UpperQuantile { quantile } => {
            writeln!(&mut canonical, "aggregation=upper_quantile")
                .expect("canonical fingerprint write should not fail");
            writeln!(
                &mut canonical,
                "aggregation.quantile={}",
                canonical_f64(quantile)
            )
            .expect("canonical fingerprint write should not fail");
        }
        RealizedVolAggregation::Median => {
            writeln!(&mut canonical, "aggregation=median")
                .expect("canonical fingerprint write should not fail");
        }
        RealizedVolAggregation::TrimmedMean { trim_fraction } => {
            writeln!(&mut canonical, "aggregation=trimmed_mean")
                .expect("canonical fingerprint write should not fail");
            writeln!(
                &mut canonical,
                "aggregation.trim_fraction={}",
                canonical_f64(trim_fraction)
            )
            .expect("canonical fingerprint write should not fail");
        }
        RealizedVolAggregation::MedianWithUpperQuantileGuard {
            upper_quantile,
            guard_weight,
        } => {
            writeln!(
                &mut canonical,
                "aggregation=median_with_upper_quantile_guard"
            )
            .expect("canonical fingerprint write should not fail");
            writeln!(
                &mut canonical,
                "aggregation.upper_quantile={}",
                canonical_f64(upper_quantile)
            )
            .expect("canonical fingerprint write should not fail");
            writeln!(
                &mut canonical,
                "aggregation.guard_weight={}",
                canonical_f64(guard_weight)
            )
            .expect("canonical fingerprint write should not fail");
        }
    }
    write_estimator_fingerprint(&mut canonical, &config.estimator);
    for source in sources {
        writeln!(&mut canonical, "source.id={}", source.source_id)
            .expect("canonical fingerprint write should not fail");
        writeln!(
            &mut canonical,
            "source.data_client_id={}",
            source.data_client_id
        )
        .expect("canonical fingerprint write should not fail");
        writeln!(
            &mut canonical,
            "source.instrument_id={}",
            source.instrument_id
        )
        .expect("canonical fingerprint write should not fail");
        writeln!(
            &mut canonical,
            "source.source_class={}",
            source_class_fingerprint_label(source.source_class)
        )
        .expect("canonical fingerprint write should not fail");
        writeln!(
            &mut canonical,
            "source.sample_kind={}",
            sample_kind_fingerprint_label(source.sample_kind)
        )
        .expect("canonical fingerprint write should not fail");
        writeln!(&mut canonical, "source.enabled={}", source.enabled)
            .expect("canonical fingerprint write should not fail");
        writeln!(
            &mut canonical,
            "source.counts_toward_quorum={}",
            source.counts_toward_quorum
        )
        .expect("canonical fingerprint write should not fail");
        writeln!(
            &mut canonical,
            "source.canonical_base_asset={}",
            source.canonical_base_asset
        )
        .expect("canonical fingerprint write should not fail");
        writeln!(
            &mut canonical,
            "source.canonical_quote_asset={}",
            source.canonical_quote_asset
        )
        .expect("canonical fingerprint write should not fail");
    }
    let digest = Sha256::digest(canonical.as_bytes());
    format!("sha256:{}", hex::encode(digest))
}

fn canonical_f64(value: f64) -> String {
    format!("{:016x}", value.to_bits())
}

fn write_estimator_fingerprint(canonical: &mut String, estimator: &RealizedVolEstimatorConfig) {
    writeln!(
        canonical,
        "estimator.pricing_component={}",
        pricing_component_fingerprint_label(estimator.pricing_component)
    )
    .expect("canonical fingerprint write should not fail");
    match &estimator.horizon_policy {
        RealizedVolHorizonPolicy::Measured => {
            writeln!(canonical, "estimator.horizon_policy=measured")
                .expect("canonical fingerprint write should not fail");
        }
        RealizedVolHorizonPolicy::WeightedBlend => {
            writeln!(canonical, "estimator.horizon_policy=weighted_blend")
                .expect("canonical fingerprint write should not fail");
        }
        RealizedVolHorizonPolicy::MaxFloor {
            primary_horizon_id,
            floor_horizon_id,
            floor_multiplier,
        } => {
            writeln!(canonical, "estimator.horizon_policy=max_floor")
                .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.horizon_policy.primary_horizon_id={primary_horizon_id}"
            )
            .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.horizon_policy.floor_horizon_id={floor_horizon_id}"
            )
            .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.horizon_policy.floor_multiplier={}",
                canonical_f64(*floor_multiplier)
            )
            .expect("canonical fingerprint write should not fail");
        }
        RealizedVolHorizonPolicy::ShortWithLongFloor {
            short_horizon_id,
            long_horizon_id,
            floor_multiplier,
        } => {
            writeln!(canonical, "estimator.horizon_policy=short_with_long_floor")
                .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.horizon_policy.short_horizon_id={short_horizon_id}"
            )
            .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.horizon_policy.long_horizon_id={long_horizon_id}"
            )
            .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.horizon_policy.floor_multiplier={}",
                canonical_f64(*floor_multiplier)
            )
            .expect("canonical fingerprint write should not fail");
        }
    }
    let mut horizons = estimator.horizons.clone();
    horizons.sort_by(|left, right| left.horizon_id.cmp(&right.horizon_id));
    for horizon in horizons {
        writeln!(canonical, "estimator.horizon.id={}", horizon.horizon_id)
            .expect("canonical fingerprint write should not fail");
        writeln!(
            canonical,
            "estimator.horizon.window_ms={}",
            horizon.window_ms
        )
        .expect("canonical fingerprint write should not fail");
        writeln!(
            canonical,
            "estimator.horizon.sampling_interval_ms={}",
            horizon.sampling_interval_ms
        )
        .expect("canonical fingerprint write should not fail");
        writeln!(canonical, "estimator.horizon.required={}", horizon.required)
            .expect("canonical fingerprint write should not fail");
        writeln!(
            canonical,
            "estimator.horizon.weight={}",
            canonical_f64(horizon.weight)
        )
        .expect("canonical fingerprint write should not fail");
        writeln!(
            canonical,
            "estimator.horizon.role={}",
            horizon
                .role
                .map(horizon_role_fingerprint_label)
                .unwrap_or("none")
        )
        .expect("canonical fingerprint write should not fail");
    }
    match estimator.noise.method {
        RealizedVolNoiseMethod::None => {
            writeln!(canonical, "estimator.noise.method=none")
                .expect("canonical fingerprint write should not fail");
        }
        RealizedVolNoiseMethod::CoarserGrid {
            coarse_sampling_interval_ms,
            policy,
        } => {
            writeln!(canonical, "estimator.noise.method=coarser_grid")
                .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.noise.coarse_sampling_interval_ms={coarse_sampling_interval_ms}"
            )
            .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.noise.coarser_grid_policy={}",
                coarser_grid_policy_fingerprint_label(policy)
            )
            .expect("canonical fingerprint write should not fail");
        }
        RealizedVolNoiseMethod::Subsampled {
            subsamples,
            min_ready_subsamples,
        } => {
            writeln!(canonical, "estimator.noise.method=subsampled")
                .expect("canonical fingerprint write should not fail");
            writeln!(canonical, "estimator.noise.subsamples={subsamples}")
                .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.noise.min_ready_subsamples={min_ready_subsamples}"
            )
            .expect("canonical fingerprint write should not fail");
        }
    }
    writeln!(
        canonical,
        "estimator.jump.policy={}",
        jump_policy_fingerprint_label(estimator.jump.policy)
    )
    .expect("canonical fingerprint write should not fail");
    match &estimator.forecast.method {
        RealizedVolForecastMethod::None => {
            writeln!(canonical, "estimator.forecast.method=none")
                .expect("canonical fingerprint write should not fail");
        }
        RealizedVolForecastMethod::Ewma { alpha } => {
            writeln!(canonical, "estimator.forecast.method=ewma")
                .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.forecast.alpha={}",
                canonical_f64(*alpha)
            )
            .expect("canonical fingerprint write should not fail");
        }
        RealizedVolForecastMethod::HarLite {
            intercept,
            short_weight,
            medium_weight,
            long_weight,
        } => {
            writeln!(canonical, "estimator.forecast.method=har_lite")
                .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.forecast.intercept={}",
                canonical_f64(*intercept)
            )
            .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.forecast.short_weight={}",
                canonical_f64(*short_weight)
            )
            .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.forecast.medium_weight={}",
                canonical_f64(*medium_weight)
            )
            .expect("canonical fingerprint write should not fail");
            writeln!(
                canonical,
                "estimator.forecast.long_weight={}",
                canonical_f64(*long_weight)
            )
            .expect("canonical fingerprint write should not fail");
        }
    }
}

fn source_class_fingerprint_label(source_class: RealizedVolSourceClass) -> &'static str {
    match source_class {
        RealizedVolSourceClass::SpotQuote => "spot_quote",
        RealizedVolSourceClass::Trade => "trade",
        RealizedVolSourceClass::Mark => "mark",
        RealizedVolSourceClass::Index => "index",
    }
}

fn sample_kind_fingerprint_label(sample_kind: RealizedVolSampleKind) -> &'static str {
    match sample_kind {
        RealizedVolSampleKind::Midpoint => "midpoint",
        RealizedVolSampleKind::Trade => "trade",
        RealizedVolSampleKind::Mark => "mark",
        RealizedVolSampleKind::Index => "index",
    }
}

fn horizon_role_fingerprint_label(role: RealizedVolHorizonRole) -> &'static str {
    match role {
        RealizedVolHorizonRole::Short => "short",
        RealizedVolHorizonRole::Medium => "medium",
        RealizedVolHorizonRole::Long => "long",
        RealizedVolHorizonRole::Primary => "primary",
        RealizedVolHorizonRole::Floor => "floor",
    }
}

fn coarser_grid_policy_fingerprint_label(policy: RealizedVolCoarserGridPolicy) -> &'static str {
    match policy {
        RealizedVolCoarserGridPolicy::CoarseOnly => "coarse_only",
        RealizedVolCoarserGridPolicy::MinBaseCoarse => "min_base_coarse",
    }
}

fn jump_policy_fingerprint_label(policy: RealizedVolJumpPolicy) -> &'static str {
    match policy {
        RealizedVolJumpPolicy::None => "none",
        RealizedVolJumpPolicy::Separate => "separate",
    }
}

fn pricing_component_fingerprint_label(component: RealizedVolPricingComponent) -> &'static str {
    match component {
        RealizedVolPricingComponent::Measured => "measured",
        RealizedVolPricingComponent::NoiseRobust => "noise_robust",
        RealizedVolPricingComponent::Continuous => "continuous",
        RealizedVolPricingComponent::Forecast => "forecast",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SURFACE_ID: &str = "<surface_id>";
    const SOURCE_ID: &str = "<SOURCE_ID_A>";

    fn config() -> RealizedVolEngineConfig {
        RealizedVolEngineConfig {
            surface_id: SURFACE_ID.to_string(),
            window_ms: 4_000,
            sampling_interval_ms: 1_000,
            min_ready_sources: 1,
            max_source_age_ms: 500,
            max_inter_sample_gap_ms: 2_000,
            min_coverage_ratio: 0.75,
            max_cross_source_dispersion: 0.50,
            seconds_per_annum: 31_536_000.0,
            aggregation: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
            estimator: RealizedVolEstimatorConfig::measured(),
            sources: vec![RealizedVolSourceConfig {
                source_id: SOURCE_ID.to_string(),
                data_client_id: "<DATA_CLIENT_ID>".to_string(),
                instrument_id: "<INSTRUMENT_ID>.<DATA_CLIENT_ID>".to_string(),
                source_class: RealizedVolSourceClass::SpotQuote,
                sample_kind: RealizedVolSampleKind::Midpoint,
                enabled: true,
                counts_toward_quorum: true,
                canonical_base_asset: "<BASE_ASSET>".to_string(),
                canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
            }],
        }
    }

    fn observation(price: f64, ts_ms: u64) -> RealizedVolObservation {
        observation_with_receive(price, ts_ms, ts_ms)
    }

    fn observation_with_receive(
        price: f64,
        event_ts_ms: u64,
        recv_ts_ms: u64,
    ) -> RealizedVolObservation {
        RealizedVolObservation {
            source_id: SOURCE_ID.to_string(),
            source_class: RealizedVolSourceClass::SpotQuote,
            sample_kind: RealizedVolSampleKind::Midpoint,
            price,
            event_ts_ms,
            recv_ts_ms,
        }
    }

    #[test]
    fn valid_realized_vol_constructor_rejects_negative_and_non_finite_values() {
        assert_eq!(
            ValidRealizedVol::new(ZERO_F64).map(ValidRealizedVol::get),
            Some(ZERO_F64)
        );
        assert_eq!(
            ValidRealizedVol::new(HALF_F64).map(ValidRealizedVol::get),
            Some(HALF_F64)
        );
        assert_eq!(ValidRealizedVol::new(-UNIT_F64), None);
        assert_eq!(ValidRealizedVol::new(f64::NAN), None);
        assert_eq!(ValidRealizedVol::new(f64::INFINITY), None);
        assert_eq!(ValidRealizedVol::new(f64::NEG_INFINITY), None);
    }

    #[test]
    fn source_samples_are_pruned_to_bounded_retention_horizon() {
        let mut engine = RealizedVolEngine::from_config(config()).unwrap();

        for index in 1..=32 {
            assert!(engine.observe(observation(100.0 + index as f64, index * 1_000)));
        }

        let source = engine.sources.get(SOURCE_ID).unwrap();
        let retention_horizon_ms = engine
            .config
            .window_ms
            .saturating_add(engine.config.max_source_age_ms)
            .saturating_add(engine.config.sampling_interval_ms);
        let expected_bound =
            (retention_horizon_ms / engine.config.sampling_interval_ms) as usize + 1;
        assert!(source.samples.len() <= expected_bound);
    }

    #[test]
    fn same_event_millisecond_uses_receive_order_for_duplicate_or_replacement() {
        let mut engine = RealizedVolEngine::from_config(config()).unwrap();
        for (price, event_ts_ms) in [
            (100.0, 1_000),
            (101.0, 2_000),
            (102.0, 3_000),
            (103.0, 4_000),
        ] {
            assert!(engine.observe(observation(price, event_ts_ms)));
        }

        let initial_snapshot = engine.snapshot_at(4_000);
        let initial_rv = initial_snapshot
            .annualized_realized_vol_decimal
            .expect("four exact-grid samples must produce realized volatility");

        assert!(!engine.observe(observation_with_receive(130.0, 4_000, 4_000)));
        let retained_after_duplicate = engine
            .sources
            .get(SOURCE_ID)
            .and_then(|source| source.samples.back())
            .expect("accepted source sample must remain retained");
        assert_eq!(retained_after_duplicate.price, 103.0);
        assert_eq!(retained_after_duplicate.recv_ts_ms, 4_000);
        let duplicate_snapshot = engine.snapshot_at(4_000);
        assert_eq!(
            duplicate_snapshot.annualized_realized_vol_decimal,
            Some(initial_rv)
        );
        assert_eq!(
            duplicate_snapshot.source_diagnostics[0].last_rejected_reason,
            Some(RealizedVolSourceRejectReason::DuplicateTimestamp)
        );

        assert!(engine.observe(observation_with_receive(130.0, 4_000, 4_001)));
        let retained_after_later_receive = engine
            .sources
            .get(SOURCE_ID)
            .and_then(|source| source.samples.back())
            .expect("later-received same-event sample must be retained");
        assert_eq!(retained_after_later_receive.price, 130.0);
        assert_eq!(retained_after_later_receive.recv_ts_ms, 4_001);
        let replaced_rv = engine
            .snapshot_at(4_000)
            .annualized_realized_vol_decimal
            .expect("replacement at the final grid point must preserve readiness");
        assert!(replaced_rv > initial_rv);
    }
}
