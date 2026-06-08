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

const ZERO_MILLIS_U64: u64 = u64::MIN;
const INITIAL_REJECTION_COUNT: u64 = u64::MIN;
const COUNTER_INCREMENT_U64: u64 = 1;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RealizedVolEngineConfig {
    pub surface_id: String,
    pub window_ms: u64,
    pub sampling_interval_ms: u64,
    pub min_ready_sources: usize,
    pub max_source_age_ms: u64,
    pub max_event_receive_lag_ms: u64,
    pub max_inter_sample_gap_ms: u64,
    pub min_coverage_ratio: f64,
    pub max_cross_source_dispersion: f64,
    pub seconds_per_annum: f64,
    pub aggregation: RealizedVolAggregation,
    pub sources: Vec<RealizedVolSourceConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum RealizedVolAggregation {
    UpperQuantile { quantile: f64 },
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
    pub annualized_realized_vol_decimal: Option<f64>,
    pub ready: bool,
    pub sources_used: Vec<String>,
    pub source_diagnostics: Vec<RealizedVolSourceDiagnostic>,
    pub unknown_source_rejections: BTreeMap<String, u64>,
    pub blocked_reasons: Vec<RealizedVolBlockReason>,
    pub aggregate_method: RealizedVolAggregation,
    pub seconds_per_annum: f64,
    pub config_fingerprint: String,
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
            annualized_realized_vol_decimal: None,
            ready: false,
            sources_used: Vec::new(),
            source_diagnostics: Vec::new(),
            unknown_source_rejections: BTreeMap::new(),
            blocked_reasons: vec![RealizedVolBlockReason::InvalidConfig],
            aggregate_method,
            seconds_per_annum,
            config_fingerprint: config_fingerprint.to_string(),
        }
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
    pub first_sample_ts_ms: Option<u64>,
    pub last_sample_ts_ms: Option<u64>,
    pub raw_sample_count: usize,
    pub grid_sample_count: usize,
    pub coverage_ratio: f64,
    pub max_inter_sample_gap_ms: Option<u64>,
    pub last_rejected_reason: Option<RealizedVolSourceRejectReason>,
    pub rejection_counters: BTreeMap<RealizedVolSourceRejectReason, u64>,
    pub block_reason: Option<RealizedVolBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealizedVolSourceStatus {
    Ready,
    Waiting,
    Rejected,
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
    ReceiveBeforeEvent,
    EventReceiveLagExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RealizedVolBlockReason {
    InvalidConfig,
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

    pub fn observe(&mut self, observation: RealizedVolObservation) -> bool {
        let Some(source) = self.sources.get_mut(&observation.source_id) else {
            increment_counter(&mut self.unknown_source_rejections, observation.source_id);
            return false;
        };
        let rejected = reject_observation(
            &source.config,
            &source.samples,
            &observation,
            self.config.max_event_receive_lag_ms,
        );
        if let Some(reason) = rejected {
            source.last_rejected_reason = Some(reason);
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
            let diagnostic = source_diagnostic(&self.config, state, as_of_ms);
            match (
                state.config.enabled,
                diagnostic.status,
                state.config.counts_toward_quorum,
                diagnostic.annualized_realized_vol_decimal,
                diagnostic.block_reason,
            ) {
                (true, RealizedVolSourceStatus::Ready, true, Some(value), _) => {
                    ready_values.push((
                        diagnostic.source_id.clone(),
                        diagnostic.source_class,
                        diagnostic.sample_kind,
                        value,
                    ));
                }
                (true, _, true, _, Some(reason)) => {
                    blockers.insert(reason);
                }
                _ => {}
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
        match aggregate {
            Some(value)
                if dispersion(&ready_values, value) > self.config.max_cross_source_dispersion =>
            {
                blockers.insert(RealizedVolBlockReason::CrossSourceDispersion);
            }
            _ => {}
        }
        let ready = blockers.is_empty() && aggregate.is_some();
        RealizedVolSnapshot {
            surface_id: self.config.surface_id.clone(),
            as_of_ms,
            annualized_realized_vol_decimal: if ready { aggregate } else { None },
            ready,
            sources_used: if ready {
                ready_values
                    .iter()
                    .map(|(source_id, _, _, _)| source_id.clone())
                    .collect()
            } else {
                Vec::new()
            },
            source_diagnostics: diagnostics,
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
    }
    let mut ids = BTreeSet::new();
    for source in &config.sources {
        if source.source_id.trim().is_empty() {
            return Err("source_id must be non-empty".to_string());
        }
        if !ids.insert(source.source_id.clone()) {
            return Err(format!("duplicate source_id `{}`", source.source_id));
        }
    }
    Ok(())
}

fn reject_observation(
    config: &RealizedVolSourceConfig,
    samples: &VecDeque<RealizedVolObservation>,
    observation: &RealizedVolObservation,
    max_lag_ms: u64,
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
    if observation.recv_ts_ms < observation.event_ts_ms {
        return Some(RealizedVolSourceRejectReason::ReceiveBeforeEvent);
    }
    if observation
        .recv_ts_ms
        .saturating_sub(observation.event_ts_ms)
        > max_lag_ms
    {
        return Some(RealizedVolSourceRejectReason::EventReceiveLagExceeded);
    }
    if samples
        .back()
        .is_some_and(|sample| observation.event_ts_ms < sample.event_ts_ms)
    {
        return Some(RealizedVolSourceRejectReason::EventTimeRegression);
    }
    if let Some(sample) = samples
        .back()
        .filter(|sample| observation.event_ts_ms == sample.event_ts_ms)
    {
        if observation.recv_ts_ms == sample.recv_ts_ms {
            return Some(RealizedVolSourceRejectReason::DuplicateTimestamp);
        }
        if observation.recv_ts_ms < sample.recv_ts_ms {
            return Some(RealizedVolSourceRejectReason::StaleSameEventUpdate);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
struct SourceComputation {
    rv: Option<f64>,
    block_reason: Option<RealizedVolBlockReason>,
}

fn source_diagnostic(
    config: &RealizedVolEngineConfig,
    state: &SourceState,
    as_of_ms: u64,
) -> RealizedVolSourceDiagnostic {
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
        .map(|pair| pair[1].0.saturating_sub(pair[0].0))
        .max();
    let computation = compute_rv(config, &grid, coverage_ratio, max_gap, as_of_ms);
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
    RealizedVolSourceDiagnostic {
        source_id: state.config.source_id.clone(),
        source_class: state.config.source_class,
        sample_kind: state.config.sample_kind,
        enabled: state.config.enabled,
        counts_toward_quorum: state.config.counts_toward_quorum,
        status: if computation.rv.is_some() {
            RealizedVolSourceStatus::Ready
        } else if state.last_rejected_reason.is_some() {
            RealizedVolSourceStatus::Rejected
        } else {
            RealizedVolSourceStatus::Waiting
        },
        annualized_realized_vol_decimal: computation.rv,
        first_sample_ts_ms: window_samples.first().map(|sample| sample.event_ts_ms),
        last_sample_ts_ms: samples.last().map(|sample| sample.event_ts_ms),
        raw_sample_count: window_samples.len(),
        grid_sample_count: grid.len(),
        coverage_ratio,
        max_inter_sample_gap_ms: max_gap,
        last_rejected_reason: state.last_rejected_reason,
        rejection_counters: state.rejection_counters.clone(),
        block_reason: rejection_block_reason.or(computation.block_reason),
    }
}

fn grid_prices(
    config: &RealizedVolEngineConfig,
    samples: &[&RealizedVolObservation],
    window_start_ms: u64,
    as_of_ms: u64,
) -> Vec<(u64, f64)> {
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
                out.push((ts, sample.price));
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
    grid: &[(u64, f64)],
    coverage_ratio: f64,
    max_gap: Option<u64>,
    as_of_ms: u64,
) -> SourceComputation {
    let Some((last_grid_ts_ms, _)) = grid.last() else {
        return SourceComputation {
            rv: None,
            block_reason: Some(RealizedVolBlockReason::NotWarm),
        };
    };
    if as_of_ms.saturating_sub(*last_grid_ts_ms) > config.max_source_age_ms {
        return SourceComputation {
            rv: None,
            block_reason: Some(RealizedVolBlockReason::SourceStale),
        };
    }
    if grid.len() < 2 {
        return SourceComputation {
            rv: None,
            block_reason: Some(RealizedVolBlockReason::NotWarm),
        };
    }
    if coverage_ratio < config.min_coverage_ratio {
        return SourceComputation {
            rv: None,
            block_reason: Some(RealizedVolBlockReason::CoverageBelowMinimum),
        };
    }
    if max_gap.is_some_and(|gap| gap > config.max_inter_sample_gap_ms) {
        return SourceComputation {
            rv: None,
            block_reason: Some(RealizedVolBlockReason::InterSampleGapExceeded),
        };
    }
    if !is_positive_finite(config.seconds_per_annum) {
        return SourceComputation {
            rv: None,
            block_reason: Some(RealizedVolBlockReason::AnnualizationBasisInvalid),
        };
    }
    let mut sum = ZERO_F64;
    let mut elapsed_ms = 0;
    for pair in grid.windows(2) {
        let dt = pair[1].0.saturating_sub(pair[0].0);
        let log_return = (pair[1].1 / pair[0].1).ln();
        if dt == ZERO_MILLIS_U64 || !log_return.is_finite() {
            return SourceComputation {
                rv: None,
                block_reason: Some(RealizedVolBlockReason::NotWarm),
            };
        }
        sum += log_return.powi(POWER_OF_TWO);
        elapsed_ms += dt;
    }
    let elapsed_seconds = elapsed_ms as f64 / MILLIS_PER_SECOND_F64;
    let variance = (sum / elapsed_seconds) * config.seconds_per_annum;
    let rv = variance.sqrt();
    if rv.is_finite() && rv >= ZERO_F64 {
        SourceComputation {
            rv: Some(rv),
            block_reason: None,
        }
    } else {
        SourceComputation {
            rv: None,
            block_reason: Some(RealizedVolBlockReason::AnnualizationBasisInvalid),
        }
    }
}

fn ready_values_have_mismatched_classes(
    values: &[(String, RealizedVolSourceClass, RealizedVolSampleKind, f64)],
) -> bool {
    values
        .first()
        .is_some_and(|first| values.iter().any(|value| value.1 != first.1))
}

fn ready_values_have_mismatched_sample_kinds(
    values: &[(String, RealizedVolSourceClass, RealizedVolSampleKind, f64)],
) -> bool {
    values
        .first()
        .is_some_and(|first| values.iter().any(|value| value.2 != first.2))
}

fn upper_quantile(
    values: &[(String, RealizedVolSourceClass, RealizedVolSampleKind, f64)],
    aggregation: RealizedVolAggregation,
) -> Option<f64> {
    let mut sorted = values.iter().map(|value| value.3).collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    match aggregation {
        RealizedVolAggregation::UpperQuantile { quantile } => {
            let index = ((quantile * sorted.len() as f64).ceil() as usize)
                .saturating_sub(1)
                .min(sorted.len().saturating_sub(1));
            sorted.get(index).copied()
        }
    }
}

fn dispersion(
    values: &[(String, RealizedVolSourceClass, RealizedVolSampleKind, f64)],
    aggregate: f64,
) -> f64 {
    if values.len() < 2 {
        return ZERO_F64;
    }
    let mut sorted = values.iter().map(|value| value.3).collect::<Vec<_>>();
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
        "max_event_receive_lag_ms={}",
        config.max_event_receive_lag_ms
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
    }
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
            max_event_receive_lag_ms: 250,
            max_inter_sample_gap_ms: 2_000,
            min_coverage_ratio: 0.75,
            max_cross_source_dispersion: 0.50,
            seconds_per_annum: 31_536_000.0,
            aggregation: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
            sources: vec![RealizedVolSourceConfig {
                source_id: SOURCE_ID.to_string(),
                data_client_id: "<DATA_CLIENT_ID>".to_string(),
                instrument_id: "<INSTRUMENT_ID>.<DATA_CLIENT_ID>".to_string(),
                source_class: RealizedVolSourceClass::SpotQuote,
                sample_kind: RealizedVolSampleKind::Midpoint,
                enabled: true,
                counts_toward_quorum: true,
                canonical_quote_asset: "<QUOTE_ASSET>".to_string(),
            }],
        }
    }

    fn observation(price: f64, ts_ms: u64) -> RealizedVolObservation {
        RealizedVolObservation {
            source_id: SOURCE_ID.to_string(),
            source_class: RealizedVolSourceClass::SpotQuote,
            sample_kind: RealizedVolSampleKind::Midpoint,
            price,
            event_ts_ms: ts_ms,
            recv_ts_ms: ts_ms,
        }
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
}
