# Realized Volatility Surfaces Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Start prompt:** Before implementing, read `implementation-prompt.md` in this
> directory. It records the latest external review outcomes and design
> constraints discovered after this plan was first drafted.

**Goal:** Implement a TOML-owned, RV-specific, multi-source realized-volatility engine that publishes audit-grade `RealizedVolSnapshot` values consumed by taker pricing and future strategies.

**Architecture:** Add a shared RV-only module below strategy code. Root config owns `realized_volatility_surfaces`; strategies reference a `realized_volatility_surface_id`. Strategy code forwards observations only; RV readiness, source validation, per-source fixed-grid RV, conservative aggregation, and blockers live in the shared RV module.

**Tech Stack:** Rust, TOML config via `serde`, existing Bolt-v3 validation, existing `cargo test` / `cargo clippy` verification.

---

## File Structure

- Create `src/bolt_v3_realized_volatility.rs`: RV config view, observation types, engine, snapshot, diagnostics, blockers, and tests for pure engine behavior.
- Modify `src/lib.rs`: export `bolt_v3_realized_volatility`.
- Modify `src/bolt_v3_config.rs`: parse root `[realized_volatility_surfaces]` and strategy `realized_volatility_surface_id`.
- Modify `src/bolt_v3_validate.rs`: validate realized-volatility surface references, clients, instruments, source ids, and policy values.
- Modify `src/bolt_v3_taker_pricing.rs`: consume `RealizedVolSnapshot` in surfaced RV mode.
- Modify `src/strategies/binary_oracle_edge_taker/config.rs`: reject legacy RV fields in surfaced RV mode.
- Modify `src/strategies/binary_oracle_edge_taker/mod.rs`: forward configured RV observations and consume pricing snapshots without implementing RV policy.
- Modify `src/bolt_v3_decision_evidence.rs`: add RV snapshot evidence fields.
- Add `tests/bolt_v3_realized_volatility.rs`: engine integration tests.
- Modify `tests/bolt_v3_strategy_registration.rs`: root/strategy mapping and validation tests.
- Modify `tests/bolt_v3_decision_evidence.rs`: evidence serialization tests.
- Add `tests/bolt_v3_realized_volatility_source_fence.rs`: strategy boundary source-fence test.

## Task 1: RV Engine Contract

**Files:**
- Create: `src/bolt_v3_realized_volatility.rs`
- Modify: `src/lib.rs`
- Test: `tests/bolt_v3_realized_volatility.rs`

- [ ] **Step 1: Write failing tests for source-id keyed RV snapshots**

Create `tests/bolt_v3_realized_volatility.rs` with:

```rust
use bolt_v2::bolt_v3_realized_volatility::{
    RealizedVolAggregation, RealizedVolBlockReason, RealizedVolEngine,
    RealizedVolEngineConfig, RealizedVolObservation, RealizedVolSampleKind,
    RealizedVolSnapshot,
    RealizedVolSourceClass, RealizedVolSourceConfig, RealizedVolSourceRejectReason,
    RealizedVolSourceStatus,
};

const SURFACE_ID: &str = "<surface_id>";
const SOURCE_A: &str = "<SOURCE_ID_A>";
const SOURCE_B: &str = "<SOURCE_ID_B>";

fn source(source_id: &str) -> RealizedVolSourceConfig {
    RealizedVolSourceConfig {
        source_id: source_id.to_string(),
        source_class: RealizedVolSourceClass::SpotQuote,
        sample_kind: RealizedVolSampleKind::Midpoint,
        enabled: true,
        counts_toward_quorum: true,
    }
}

fn config(source_ids: &[&str]) -> RealizedVolEngineConfig {
    RealizedVolEngineConfig {
        surface_id: SURFACE_ID.to_string(),
        window_ms: 4_000,
        sampling_interval_ms: 1_000,
        min_ready_sources: source_ids.len(),
        max_source_age_ms: 500,
        max_event_receive_lag_ms: 250,
        max_inter_sample_gap_ms: 2_000,
        min_coverage_ratio: 0.75,
        max_cross_source_dispersion: 0.50,
        seconds_per_annum: 31_536_000.0,
        aggregation: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        sources: source_ids.iter().map(|source_id| source(source_id)).collect(),
    }
}

fn observation(source_id: &str, price: f64, ts_ms: u64) -> RealizedVolObservation {
    RealizedVolObservation {
        source_id: source_id.to_string(),
        source_class: RealizedVolSourceClass::SpotQuote,
        sample_kind: RealizedVolSampleKind::Midpoint,
        price,
        event_ts_ms: ts_ms,
        recv_ts_ms: ts_ms,
    }
}

fn observe_path(engine: &mut RealizedVolEngine, source_id: &str, prices: &[f64]) {
    for (index, price) in prices.iter().enumerate() {
        let ts_ms = (index as u64 + 1) * 1_000;
        assert!(engine.observe(observation(source_id, *price, ts_ms)));
    }
}

#[test]
fn source_id_quorum_snapshot_records_audit_fields() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A, SOURCE_B])).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 101.0, 102.0, 103.0]);
    observe_path(&mut engine, SOURCE_B, &[200.0, 202.0, 204.0, 206.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(snapshot.surface_id, SURFACE_ID);
    assert_eq!(snapshot.sources_used, vec![SOURCE_A.to_string(), SOURCE_B.to_string()]);
    assert_eq!(snapshot.aggregate_method, RealizedVolAggregation::UpperQuantile { quantile: 1.0 });
    assert_eq!(snapshot.seconds_per_annum, 31_536_000.0);
    assert!(snapshot.annualized_realized_vol_decimal.unwrap() > 0.0);
    assert!(snapshot.blocked_reasons.is_empty());
    assert_eq!(snapshot.source_diagnostics.len(), 2);
    assert!(snapshot.source_diagnostics.iter().all(|d| d.status == RealizedVolSourceStatus::Ready));
}

#[test]
fn cross_source_dispersion_blocks_instead_of_publishing_low_rv() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A, SOURCE_B])).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.1, 100.2, 100.3]);
    observe_path(&mut engine, SOURCE_B, &[100.0, 105.0, 95.0, 110.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(!snapshot.ready);
    assert_eq!(snapshot.annualized_realized_vol_decimal, None);
    assert!(snapshot.blocked_reasons.contains(&RealizedVolBlockReason::CrossSourceDispersion));
}

#[test]
fn fixed_grid_coverage_is_required_before_source_is_ready() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    assert!(engine.observe(observation(SOURCE_A, 100.0, 1_000)));
    assert!(engine.observe(observation(SOURCE_A, 104.0, 4_000)));

    let snapshot = engine.snapshot_at(4_000);

    assert!(!snapshot.ready);
    assert!(snapshot.blocked_reasons.contains(&RealizedVolBlockReason::CoverageBelowMinimum));
    assert!(snapshot.blocked_reasons.contains(&RealizedVolBlockReason::QuorumNotReady));
    assert_eq!(snapshot.source_diagnostics[0].status, RealizedVolSourceStatus::Waiting);
    assert!(snapshot.source_diagnostics[0].coverage_ratio < 0.75);
}

#[test]
fn same_event_update_requires_strictly_larger_receive_time() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    assert!(engine.observe(RealizedVolObservation {
        recv_ts_ms: 1_200,
        ..observation(SOURCE_A, 100.0, 1_000)
    }));

    assert!(!engine.observe(RealizedVolObservation {
        recv_ts_ms: 1_100,
        price: 101.0,
        ..observation(SOURCE_A, 101.0, 1_000)
    }));

    let snapshot = engine.snapshot_at(1_000);
    let diagnostic = &snapshot.source_diagnostics[0];
    assert_eq!(diagnostic.last_rejected_reason, Some(RealizedVolSourceRejectReason::StaleSameEventUpdate));
}

#[test]
fn unknown_source_rejections_are_audited_without_mutating_configured_sources() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    assert!(!engine.observe(observation("<unknown_source_id>", 100.0, 1_000)));

    let snapshot = engine.snapshot_at(1_000);

    assert_eq!(snapshot.unknown_source_rejections.get("<unknown_source_id>").copied(), Some(1));
    assert_eq!(snapshot.source_diagnostics.len(), 1);
    assert_eq!(snapshot.source_diagnostics[0].source_id, SOURCE_A);
}

#[test]
fn observation_validation_rejects_timestamp_and_lag_violations() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    assert!(engine.observe(observation(SOURCE_A, 100.0, 1_000)));

    let cases = [
        (
            RealizedVolObservation { event_ts_ms: 900, recv_ts_ms: 900, ..observation(SOURCE_A, 101.0, 900) },
            RealizedVolSourceRejectReason::EventTimeRegression,
        ),
        (
            observation(SOURCE_A, 100.0, 1_000),
            RealizedVolSourceRejectReason::DuplicateTimestamp,
        ),
        (
            RealizedVolObservation { event_ts_ms: 2_000, recv_ts_ms: 1_999, ..observation(SOURCE_A, 101.0, 2_000) },
            RealizedVolSourceRejectReason::ReceiveBeforeEvent,
        ),
        (
            RealizedVolObservation { event_ts_ms: 2_000, recv_ts_ms: 2_500, ..observation(SOURCE_A, 101.0, 2_000) },
            RealizedVolSourceRejectReason::EventReceiveLagExceeded,
        ),
    ];

    for (observation, reason) in cases {
        assert!(!engine.observe(observation));
        let snapshot = engine.snapshot_at(2_000);
        assert_eq!(snapshot.source_diagnostics[0].last_rejected_reason, Some(reason));
    }
}

#[test]
fn flat_valid_source_publishes_zero_realized_volatility() {
    let mut engine = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.0, 100.0, 100.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(snapshot.ready);
    assert_eq!(snapshot.annualized_realized_vol_decimal, Some(0.0));
}

#[test]
fn zero_aggregate_with_divergent_ready_sources_blocks_dispersion() {
    let mut cfg = config(&[SOURCE_A, SOURCE_B]);
    cfg.aggregation = RealizedVolAggregation::UpperQuantile { quantile: 0.5 };
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    observe_path(&mut engine, SOURCE_A, &[100.0, 100.0, 100.0, 100.0]);
    observe_path(&mut engine, SOURCE_B, &[100.0, 110.0, 90.0, 120.0]);

    let snapshot = engine.snapshot_at(4_000);

    assert!(!snapshot.ready);
    assert_eq!(snapshot.annualized_realized_vol_decimal, None);
    assert!(snapshot.blocked_reasons.contains(&RealizedVolBlockReason::CrossSourceDispersion));
}

#[test]
fn fresh_pre_window_observation_can_seed_first_grid_cell() {
    let mut cfg = config(&[SOURCE_A]);
    cfg.max_source_age_ms = 1_500;
    let mut engine = RealizedVolEngine::from_config(cfg).unwrap();
    assert!(engine.observe(observation(SOURCE_A, 100.0, 750)));
    assert!(engine.observe(observation(SOURCE_A, 101.0, 3_000)));
    assert!(engine.observe(observation(SOURCE_A, 102.0, 4_000)));
    assert!(engine.observe(observation(SOURCE_A, 103.0, 5_000)));

    let snapshot = engine.snapshot_at(5_000);

    assert!(snapshot.ready);
    assert_eq!(snapshot.source_diagnostics[0].grid_sample_count, 4);
}

#[test]
fn config_fingerprint_changes_when_policy_changes() {
    let baseline = RealizedVolEngine::from_config(config(&[SOURCE_A])).unwrap();
    let mut changed_config = config(&[SOURCE_A]);
    changed_config.max_source_age_ms += 1;
    let changed = RealizedVolEngine::from_config(changed_config).unwrap();

    assert_ne!(baseline.snapshot_at(4_000).config_fingerprint, changed.snapshot_at(4_000).config_fingerprint);
}

#[test]
fn invalid_config_snapshot_uses_explicit_invalid_config_blocker() {
    let snapshot = RealizedVolSnapshot::invalid_config(
        "<surface_id>",
        4_000,
        RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        31_536_000.0,
        "<config_fingerprint>",
    );

    assert!(!snapshot.ready);
    assert_eq!(snapshot.annualized_realized_vol_decimal, None);
    assert_eq!(snapshot.blocked_reasons, vec![RealizedVolBlockReason::InvalidConfig]);
}

#[test]
fn realized_volatility_block_reason_contract_is_exhaustive() {
    assert_eq!(
        RealizedVolBlockReason::ALL,
        &[
            RealizedVolBlockReason::InvalidConfig,
            RealizedVolBlockReason::QuorumNotReady,
            RealizedVolBlockReason::SourceStale,
            RealizedVolBlockReason::CoverageBelowMinimum,
            RealizedVolBlockReason::InterSampleGapExceeded,
            RealizedVolBlockReason::SourceClassMismatch,
            RealizedVolBlockReason::SampleKindMismatch,
            RealizedVolBlockReason::CrossSourceDispersion,
            RealizedVolBlockReason::AnnualizationBasisInvalid,
            RealizedVolBlockReason::NotWarm,
        ]
    );
}
```

Also add an internal `#[cfg(test)]` unit test in
`src/bolt_v3_realized_volatility.rs` that feeds more observations than the
retention horizon can need and asserts each `SourceState::samples.len()` remains
bounded by the configured window, freshness horizon, and sampling interval.

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test --test bolt_v3_realized_volatility -- --nocapture
```

Expected: FAIL with unresolved import `bolt_v3_realized_volatility`.

- [ ] **Step 3: Implement the RV module**

Create `src/bolt_v3_realized_volatility.rs` with:

```rust
//! RV-specific realized-volatility engine.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::bolt_v3_numeric::{is_positive_finite, MILLIS_PER_SECOND_F64, POWER_OF_TWO, ZERO_F64};

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
    pub source_class: RealizedVolSourceClass,
    pub sample_kind: RealizedVolSampleKind,
    pub enabled: bool,
    pub counts_toward_quorum: bool,
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
        let sources = config.sources.iter().cloned().map(|source| {
            (
                source.source_id.clone(),
                SourceState {
                    config: source,
                    samples: VecDeque::new(),
                    last_rejected_reason: None,
                    rejection_counters: BTreeMap::new(),
                },
            )
        }).collect();
        Ok(Self { config, sources, unknown_source_rejections: BTreeMap::new() })
    }

    pub fn observe(&mut self, observation: RealizedVolObservation) -> bool {
        let Some(source) = self.sources.get_mut(&observation.source_id) else {
            *self.unknown_source_rejections.entry(observation.source_id).or_default() += 1;
            return false;
        };
        let rejected = reject_observation(&source.config, &source.samples, &observation, self.config.max_event_receive_lag_ms);
        if let Some(reason) = rejected {
            source.last_rejected_reason = Some(reason);
            *source.rejection_counters.entry(reason).or_default() += 1;
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
        for state in self.sources.values().filter(|state| state.config.enabled) {
            let diagnostic = source_diagnostic(&self.config, state, as_of_ms);
            if diagnostic.status == RealizedVolSourceStatus::Ready && state.config.counts_toward_quorum {
                if let Some(value) = diagnostic.annualized_realized_vol_decimal {
                    ready_values.push((diagnostic.source_id.clone(), diagnostic.source_class, diagnostic.sample_kind, value));
                }
            } else if state.config.counts_toward_quorum {
                if let Some(reason) = diagnostic.block_reason {
                    blockers.insert(reason);
                }
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
        if let Some(value) = aggregate {
            if dispersion(&ready_values, value) > self.config.max_cross_source_dispersion {
                blockers.insert(RealizedVolBlockReason::CrossSourceDispersion);
            }
        }
        let ready = blockers.is_empty() && aggregate.is_some();
        RealizedVolSnapshot {
            surface_id: self.config.surface_id.clone(),
            as_of_ms,
            annualized_realized_vol_decimal: if ready { aggregate } else { None },
            ready,
            sources_used: if ready { ready_values.iter().map(|(id, _, _, _)| id.clone()).collect() } else { Vec::new() },
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
    if !is_positive_finite(config.seconds_per_annum) {
        return Err("seconds_per_annum must be positive finite".to_string());
    }
    if config.min_coverage_ratio <= ZERO_F64 || config.min_coverage_ratio > 1.0 {
        return Err("min_coverage_ratio must be in (0, 1]".to_string());
    }
    if !config.max_cross_source_dispersion.is_finite() || config.max_cross_source_dispersion < ZERO_F64 {
        return Err("max_cross_source_dispersion must be finite and non-negative".to_string());
    }
    match config.aggregation {
        RealizedVolAggregation::UpperQuantile { quantile } if !(0.5..=1.0).contains(&quantile) => {
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
    if observation.recv_ts_ms.saturating_sub(observation.event_ts_ms) > max_lag_ms {
        return Some(RealizedVolSourceRejectReason::EventReceiveLagExceeded);
    }
    if samples.back().is_some_and(|sample| observation.event_ts_ms < sample.event_ts_ms) {
        return Some(RealizedVolSourceRejectReason::EventTimeRegression);
    }
    if let Some(sample) = samples.back().filter(|sample| observation.event_ts_ms == sample.event_ts_ms) {
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

fn source_diagnostic(config: &RealizedVolEngineConfig, state: &SourceState, as_of_ms: u64) -> RealizedVolSourceDiagnostic {
    let window_start_ms = as_of_ms.saturating_sub(config.window_ms);
    let samples = state.samples.iter().filter(|sample| sample.event_ts_ms <= as_of_ms).collect::<Vec<_>>();
    let grid = grid_prices(config, &samples, window_start_ms, as_of_ms);
    let expected_grid_count = (config.window_ms / config.sampling_interval_ms) as usize;
    let coverage_ratio = if expected_grid_count == 0 { ZERO_F64 } else { grid.len() as f64 / expected_grid_count as f64 };
    let max_gap = grid.windows(2).map(|pair| pair[1].0.saturating_sub(pair[0].0)).max();
    let computation = compute_rv(config, &grid, coverage_ratio, max_gap, as_of_ms);
    let window_samples = samples
        .iter()
        .copied()
        .filter(|sample| sample.event_ts_ms >= window_start_ms)
        .collect::<Vec<_>>();
    let rejection_block_reason = match state.last_rejected_reason {
        Some(RealizedVolSourceRejectReason::SourceClassMismatch) => Some(RealizedVolBlockReason::SourceClassMismatch),
        Some(RealizedVolSourceRejectReason::SampleKindMismatch) => Some(RealizedVolBlockReason::SampleKindMismatch),
        _ => None,
    };
    RealizedVolSourceDiagnostic {
        source_id: state.config.source_id.clone(),
        source_class: state.config.source_class,
        sample_kind: state.config.sample_kind,
        status: if computation.rv.is_some() { RealizedVolSourceStatus::Ready } else if state.last_rejected_reason.is_some() { RealizedVolSourceStatus::Rejected } else { RealizedVolSourceStatus::Waiting },
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

fn grid_prices(config: &RealizedVolEngineConfig, samples: &[&RealizedVolObservation], window_start_ms: u64, as_of_ms: u64) -> Vec<(u64, f64)> {
    let mut out = Vec::new();
    let mut latest = None;
    let mut index = 0;
    let mut ts = window_start_ms + config.sampling_interval_ms;
    while ts <= as_of_ms {
        while index < samples.len() && samples[index].event_ts_ms <= ts {
            latest = Some(samples[index]);
            index += 1;
        }
        if let Some(sample) = latest {
            if ts.saturating_sub(sample.event_ts_ms) <= config.max_source_age_ms {
                out.push((ts, sample.price));
            }
        }
        ts = ts.saturating_add(config.sampling_interval_ms);
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
        return SourceComputation { rv: None, block_reason: Some(RealizedVolBlockReason::NotWarm) };
    };
    if as_of_ms.saturating_sub(*last_grid_ts_ms) > config.max_source_age_ms {
        return SourceComputation { rv: None, block_reason: Some(RealizedVolBlockReason::SourceStale) };
    }
    if grid.len() < 2 {
        return SourceComputation { rv: None, block_reason: Some(RealizedVolBlockReason::NotWarm) };
    }
    if coverage_ratio < config.min_coverage_ratio {
        return SourceComputation { rv: None, block_reason: Some(RealizedVolBlockReason::CoverageBelowMinimum) };
    }
    if max_gap.is_some_and(|gap| gap > config.max_inter_sample_gap_ms) {
        return SourceComputation { rv: None, block_reason: Some(RealizedVolBlockReason::InterSampleGapExceeded) };
    }
    if !is_positive_finite(config.seconds_per_annum) {
        return SourceComputation { rv: None, block_reason: Some(RealizedVolBlockReason::AnnualizationBasisInvalid) };
    }
    let mut sum = ZERO_F64;
    let mut elapsed_ms = 0;
    for pair in grid.windows(2) {
        let dt = pair[1].0.saturating_sub(pair[0].0);
        let log_return = (pair[1].1 / pair[0].1).ln();
        if dt == 0 || !log_return.is_finite() {
            return SourceComputation { rv: None, block_reason: Some(RealizedVolBlockReason::NotWarm) };
        }
        sum += log_return.powi(POWER_OF_TWO);
        elapsed_ms += dt;
    }
    let elapsed_seconds = elapsed_ms as f64 / MILLIS_PER_SECOND_F64;
    let variance = (sum / elapsed_seconds) * config.seconds_per_annum;
    let rv = variance.sqrt();
    if rv.is_finite() && rv >= ZERO_F64 {
        SourceComputation { rv: Some(rv), block_reason: None }
    } else {
        SourceComputation { rv: None, block_reason: Some(RealizedVolBlockReason::AnnualizationBasisInvalid) }
    }
}

fn ready_values_have_mismatched_classes(values: &[(String, RealizedVolSourceClass, RealizedVolSampleKind, f64)]) -> bool {
    values.first().is_some_and(|first| values.iter().any(|value| value.1 != first.1))
}

fn ready_values_have_mismatched_sample_kinds(values: &[(String, RealizedVolSourceClass, RealizedVolSampleKind, f64)]) -> bool {
    values.first().is_some_and(|first| values.iter().any(|value| value.2 != first.2))
}

fn upper_quantile(values: &[(String, RealizedVolSourceClass, RealizedVolSampleKind, f64)], aggregation: RealizedVolAggregation) -> Option<f64> {
    let mut sorted = values.iter().map(|value| value.3).collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    match aggregation {
        RealizedVolAggregation::UpperQuantile { quantile } => {
            let index = ((quantile * sorted.len() as f64).ceil() as usize).saturating_sub(1).min(sorted.len().saturating_sub(1));
            sorted.get(index).copied()
        }
    }
}

fn dispersion(values: &[(String, RealizedVolSourceClass, RealizedVolSampleKind, f64)], aggregate: f64) -> f64 {
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
    while samples.front().is_some_and(|sample| sample.event_ts_ms < min_event_ts_ms) {
        let _ = samples.pop_front();
    }
}

fn config_fingerprint(config: &RealizedVolEngineConfig) -> String {
    let mut canonical_config = config.clone();
    canonical_config
        .sources
        .sort_by(|left, right| left.source_id.cmp(&right.source_id));
    let canonical = toml::to_string(&canonical_config).expect("realized-volatility config should serialize");
    let digest = Sha256::digest(canonical.as_bytes());
    format!("sha256:{}", hex::encode(digest))
}
```

Modify `src/lib.rs`:

```rust
pub mod bolt_v3_realized_volatility;
```

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test --test bolt_v3_realized_volatility -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/bolt_v3_realized_volatility.rs src/lib.rs tests/bolt_v3_realized_volatility.rs
git commit -m "feat: add realized volatility engine contract"
```

## Task 2: Config Surface

**Files:**
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_validate.rs`
- Test: `src/bolt_v3_config.rs`
- Test: `tests/bolt_v3_strategy_registration.rs`

- [ ] **Step 1: Write failing config parse test**

In `src/bolt_v3_config.rs` test module, add:

```rust
#[test]
fn parses_realized_volatility_surfaces_from_root_config() {
    let raw = r#"
schema_version = 2
trader_id = "TRADER-001"
strategy_files = []

[runtime]
mode = "backtest"

[nautilus]
load_state = false
save_state = false
timeout_connection_secs = 1
timeout_reconciliation_secs = 1
timeout_portfolio_secs = 1
timeout_disconnection_secs = 1
delay_post_stop_secs = 1
timeout_shutdown_secs = 1

[nautilus.data_engine]
time_bars_build_with_no_updates = false
time_bars_timestamp_on_close = false
time_bars_skip_first_non_full_bar = false
time_bars_interval_type = "left_open"
time_bars_build_delay = 0
time_bars_origins = {}
validate_data_sequence = true
buffer_deltas = false
emit_quotes_from_book = false
emit_quotes_from_book_depths = false
external_clients = []
debug = false
graceful_shutdown_on_error = true
qsize = 1000

[nautilus.exec_engine]
load_cache = false
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_secs = 1
external_clients = []
debug = false
reconciliation = false
reconciliation_startup_delay_secs = 1
reconciliation_lookback_mins = 1
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = false
inflight_check_interval_ms = 1
inflight_check_threshold_ms = 1
inflight_check_retries = 1
open_check_interval_secs = 1
open_check_lookback_mins = 1
open_check_threshold_ms = 1
open_check_missing_retries = 1
open_check_open_only = true
max_single_order_queries_per_cycle = 1
single_order_query_delay_ms = 1
position_check_interval_secs = 1
position_check_lookback_mins = 1
position_check_threshold_ms = 1
position_check_retries = 1
purge_closed_orders_interval_mins = 1
purge_closed_orders_buffer_mins = 1
purge_closed_positions_interval_mins = 1
purge_closed_positions_buffer_mins = 1
purge_account_events_interval_mins = 1
purge_account_events_lookback_mins = 1
purge_from_database = false
own_books_audit_interval_secs = 1
graceful_shutdown_on_error = true
qsize = 1000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "1.00"

[risk.nautilus]
max_order_submit_rate = "1"
max_order_modify_rate = "1"
max_notional_per_order = {}

[logging]
log_level = "INFO"

[persistence]
decision_evidence_path = "/tmp/decision-evidence.jsonl"
decision_evidence_max_bytes = 1048576

[aws]
region = "us-east-1"

[clients.source_client]
venue = "<DATA_CLIENT_VENUE>"

[clients.source_client.data]
kind = "test_double"

[realized_volatility_surfaces."<surface_id>"]
canonical_base_asset = "<BASE_ASSET>"
canonical_quote_asset = "<QUOTE_ASSET>"

[realized_volatility_surfaces."<surface_id>".policy]
window_ms = 4000
sampling_interval_ms = 1000
min_ready_sources = 1
max_source_age_ms = 500
max_event_receive_lag_ms = 250
max_inter_sample_gap_ms = 2000
min_coverage_ratio = 0.75
max_cross_source_dispersion = 0.50
seconds_per_annum = 31536000.0
aggregation = "upper_quantile"
upper_quantile = 1.0

[[realized_volatility_surfaces."<surface_id>".sources]]
source_id = "<SOURCE_ID_A>"
data_client_id = "source_client"
instrument_id = "<INSTRUMENT_ID_A>"
source_class = "spot_quote"
sample_kind = "midpoint"
enabled = true
counts_toward_quorum = true
canonical_base_asset = "<BASE_ASSET>"
canonical_quote_asset = "<QUOTE_ASSET>"
"#;

    let config: BoltV3RootConfig = toml::from_str(raw).expect("root config should parse");
    let surface = config.realized_volatility_surfaces.get("<surface_id>").unwrap();
    assert_eq!(surface.policy.aggregation, RealizedVolatilityAggregationBlock::UpperQuantile);
    assert_eq!(surface.sources[0].source_id, "<SOURCE_ID_A>");
}
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test --lib parses_realized_volatility_surfaces_from_root_config -- --nocapture
```

Expected: FAIL because `realized_volatility_surfaces` config types do not exist.

- [ ] **Step 3: Add config structs**

Add to `src/bolt_v3_config.rs`:

```rust
pub realized_volatility_surfaces: BTreeMap<String, RealizedVolatilitySurfaceBlock>,
```

Define:

```rust
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RealizedVolatilitySurfaceBlock {
    pub canonical_base_asset: String,
    pub canonical_quote_asset: String,
    pub policy: RealizedVolatilityPolicyBlock,
    pub sources: Vec<RealizedVolatilitySourceBlock>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RealizedVolatilityPolicyBlock {
    pub window_ms: u64,
    pub sampling_interval_ms: u64,
    pub min_ready_sources: usize,
    pub max_source_age_ms: u64,
    pub max_event_receive_lag_ms: u64,
    pub max_inter_sample_gap_ms: u64,
    pub min_coverage_ratio: f64,
    pub max_cross_source_dispersion: f64,
    pub seconds_per_annum: f64,
    pub aggregation: RealizedVolatilityAggregationBlock,
    pub upper_quantile: f64,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealizedVolatilityAggregationBlock {
    UpperQuantile,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RealizedVolatilitySourceBlock {
    pub source_id: String,
    pub data_client_id: ClientId,
    pub instrument_id: InstrumentId,
    pub source_class: RealizedVolatilitySourceClassBlock,
    pub sample_kind: RealizedVolatilitySampleKindBlock,
    pub enabled: bool,
    pub counts_toward_quorum: bool,
    pub canonical_base_asset: String,
    pub canonical_quote_asset: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealizedVolatilitySourceClassBlock {
    SpotQuote,
    Trade,
    Mark,
    Index,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealizedVolatilitySampleKindBlock {
    Midpoint,
    Trade,
    Mark,
    Index,
}
```

- [ ] **Step 4: Run config parse test to verify GREEN**

Run:

```bash
cargo test --lib parses_realized_volatility_surfaces_from_root_config -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Add validation tests for policy and source identity**

Add tests in `tests/bolt_v3_strategy_registration.rs` that load a root fixture with:

- duplicate `source_id`
- unknown `data_client_id`
- empty source list
- `min_ready_sources` greater than enabled quorum sources
- strategy `realized_volatility_surface_id` referencing a missing surface

Each test must assert the validation error message names `realized_volatility_surfaces`.

- [ ] **Step 6: Implement validation**

In `src/bolt_v3_validate.rs`, add a validation pass that:

- checks every source data client exists and has a `[data]` block
- checks `source_id` values are unique within a surface
- checks each source `canonical_base_asset` matches the surface
  `canonical_base_asset`
- checks each source `canonical_quote_asset` matches the surface
  `canonical_quote_asset`
- checks source list is non-empty
- checks positive integer policy values
- checks finite decimal policy values
- checks `upper_quantile` is in `[0.5, 1.0]`
- checks `min_ready_sources <= enabled quorum source count`
- checks every surfaced strategy references a configured surface

- [ ] **Step 6a: Implement one config-to-engine adapter**

Add one adapter that converts a validated `RealizedVolatilitySurfaceBlock` into
`RealizedVolEngineConfig`. This is the only mapping point from TOML policy to
engine policy:

```rust
fn realized_volatility_engine_config(
    surface_id: &str,
    surface: &RealizedVolatilitySurfaceBlock,
) -> Result<RealizedVolEngineConfig, ConfigValidationError> {
    let aggregation = match surface.policy.aggregation {
        RealizedVolatilityAggregationBlock::UpperQuantile => {
            RealizedVolAggregation::UpperQuantile {
                quantile: surface.policy.upper_quantile,
            }
        }
    };
    // Copy validated TOML fields into RealizedVolEngineConfig without defaults.
}
```

Add a test that changes `upper_quantile` in TOML and asserts the resulting
`RealizedVolEngineConfig::aggregation` carries the changed quantile.

- [ ] **Step 7: Run validation tests**

Run:

```bash
cargo test --test bolt_v3_strategy_registration realized_volatility -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

Run:

```bash
git add src/bolt_v3_config.rs src/bolt_v3_validate.rs tests/bolt_v3_strategy_registration.rs
git commit -m "feat: add realized volatility surface config"
```

## Task 3: Reject Dual RV Paths

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/config.rs`
- Modify: `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/config.rs`
- Test: `tests/bolt_v3_strategy_registration.rs`

- [ ] **Step 1: Write failing parse/validation tests**

Add to `src/strategies/binary_oracle_edge_taker/tests/config.rs`:

```rust
#[test]
fn surfaced_realized_volatility_mode_rejects_legacy_runtime_vol_fields() {
    let mut raw = valid_raw_config();
    let table = raw.as_table_mut().expect("raw config should be a table");
    table.insert(
        "realized_volatility_surface_id".to_string(),
        Value::String("<surface_id>".to_string()),
    );

    let mut errors = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

    assert!(errors.iter().any(|error| {
        error.field == "strategies[0].config.vol_window_secs"
            && error.code == "legacy_realized_volatility_path"
    }));
}
```

Add to `tests/bolt_v3_strategy_registration.rs`:

```rust
#[test]
fn runtime_mapping_emits_surface_id_and_signal_data_for_surfaced_mode() {
    let mut loaded = fixture_loaded_config();
    let strategy = loaded.strategies.first_mut().expect("fixture strategy");
    strategy.config.parameters
        .as_table_mut()
        .unwrap()
        .insert("realized_volatility_surface_id".to_string(), toml::Value::String("<surface_id>".to_string()));

    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("surface id should map into runtime config");
    let table = raw.as_table().unwrap();

    assert_eq!(table.get("realized_volatility_surface_id").and_then(toml::Value::as_str), Some("<surface_id>"));
    assert!(!table.contains_key("vol_window_secs"));
    assert_eq!(table.get("signal_venue").and_then(toml::Value::as_str), Some("<SIGNAL_SOURCE_ID>"));
    assert_eq!(table.get("signal_instrument_id").and_then(toml::Value::as_str), Some("<INSTRUMENT_ID>"));
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test --lib surfaced_realized_volatility_mode_rejects_legacy_runtime_vol_fields -- --nocapture
cargo test --test bolt_v3_strategy_registration runtime_mapping_emits_surface_id_and_signal_data_for_surfaced_mode -- --nocapture
```

Expected: FAIL because surfaced mode is not parsed or mapped.

- [ ] **Step 3: Implement surfaced-mode validation and mapping**

Add `realized_volatility_surface_id: String` to the taker runtime config struct.
For taker strategies:

- allow the field through `validate_table`
- reject legacy `vol_window_secs`, `vol_gap_reset_secs`, `vol_min_observations`, `vol_bridge_valid_secs`
- keep `signal_venue` and `signal_instrument_id` mapped for fast-spot pricing
- map `realized_volatility_surface_id` from strategy root config into runtime config without legacy RV knobs

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test --lib surfaced_realized_volatility_mode_rejects_legacy_runtime_vol_fields -- --nocapture
cargo test --test bolt_v3_strategy_registration runtime_mapping_emits_surface_id_and_signal_data_for_surfaced_mode -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/strategies/binary_oracle_edge_taker/config.rs src/bolt_v3_archetypes/binary_oracle_edge_taker.rs src/strategies/binary_oracle_edge_taker/tests/config.rs tests/bolt_v3_strategy_registration.rs
git commit -m "feat: reject dual realized volatility paths"
```

## Task 4: Pricing Snapshot Consumption

**Files:**
- Modify: `src/bolt_v3_taker_pricing.rs`
- Test: `tests/bolt_v3_taker_pricing.rs`

- [ ] **Step 1: Write failing taker-pricing snapshot test**

Add to `tests/bolt_v3_taker_pricing.rs`:

```rust
use std::collections::BTreeMap;

use bolt_v2::bolt_v3_realized_volatility::{
    RealizedVolAggregation, RealizedVolBlockReason, RealizedVolSnapshot,
};

#[test]
fn taker_pricing_consumes_realized_vol_snapshot_without_internal_estimator_warmup() {
    let mut config = pricing_config();
    config.realized_volatility_surface_id = Some("<surface_id>".to_string());
    let mut pricing = TakerPricingState::from_config(&config);
    pricing.observe_reference_quote(&FastSpotObservation {
        venue: "<REFERENCE_SOURCE_ID>".to_string(),
        price: 3_100.0,
        observed_ts_ms: 1_000,
    });
    pricing.observe_signal_quote(
        &FastSpotObservation {
            venue: "<SIGNAL_SOURCE_ID>".to_string(),
            price: 3_100.0,
            observed_ts_ms: 1_000,
        },
        &config,
    );
    pricing.observe_realized_vol_snapshot(RealizedVolSnapshot {
        surface_id: "<surface_id>".to_string(),
        as_of_ms: 1_000,
        annualized_realized_vol_decimal: Some(2.5),
        ready: true,
        sources_used: vec!["<SOURCE_ID_A>".to_string()],
        source_diagnostics: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: Vec::new(),
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: "<config_fingerprint>".to_string(),
    });

    let result = pricing
        .entry_pricing_at(
            &config,
            TakerPricingRequest {
                now_ms: 1_000,
                strike_price: Some(3_100.0),
                seconds_to_market_end: Some(300),
            },
        )
        .expect("ready realized-volatility snapshot should satisfy taker pricing");

    assert_close(result.realized_vol, 2.5);
    assert_eq!(result.realized_vol_surface_id.as_deref(), Some("<surface_id>"));
    assert_eq!(result.realized_vol_source_venue, None);
    assert_eq!(result.realized_vol_source_ts_ms, Some(1_000));
}

#[test]
fn surfaced_realized_volatility_mode_blocks_instead_of_falling_back_to_legacy_estimator() {
    let mut config = pricing_config();
    config.realized_volatility_surface_id = Some("<surface_id>".to_string());
    let mut pricing = TakerPricingState::from_config(&config);
    warm_legacy_internal_realized_vol_estimator(&mut pricing, &config);
    pricing.observe_realized_vol_snapshot(RealizedVolSnapshot {
        surface_id: "<surface_id>".to_string(),
        as_of_ms: 1_000,
        annualized_realized_vol_decimal: None,
        ready: false,
        sources_used: Vec::new(),
        source_diagnostics: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: vec![RealizedVolBlockReason::QuorumNotReady],
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: "<config_fingerprint>".to_string(),
    });

    let err = pricing
        .entry_pricing_at(
            &config,
            TakerPricingRequest {
                now_ms: 1_000,
                strike_price: Some(3_100.0),
                seconds_to_market_end: Some(300),
            },
        )
        .expect_err("not-ready surfaced realized-volatility snapshot must fail closed");

    assert!(err.blocked_by.contains(&TakerPricingBlockReason::RealizedVolNotReady));
}
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test --test bolt_v3_taker_pricing taker_pricing_consumes_realized_vol_snapshot_without_internal_estimator_warmup -- --nocapture
cargo test --test bolt_v3_taker_pricing surfaced_realized_volatility_mode_blocks_instead_of_falling_back_to_legacy_estimator -- --nocapture
```

Expected: FAIL because `observe_realized_vol_snapshot` does not exist.

- [ ] **Step 3: Implement snapshot consumption**

In `src/bolt_v3_taker_pricing.rs`, import `RealizedVolSnapshot`, store the
latest snapshot, and add `realized_vol_surface_id` to the pricing result.
`current_realized_vol_at` must read only the latest matching ready snapshot. A
missing, stale, mismatched, or not-ready surfaced snapshot returns
`RealizedVolNotReady` and must not call a strategy-owned internal RV estimator.
`realized_vol_source_venue` is left `None` because surfaced RV provenance comes
from the snapshot source IDs.

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test --test bolt_v3_taker_pricing -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/bolt_v3_taker_pricing.rs tests/bolt_v3_taker_pricing.rs
git commit -m "feat: consume realized volatility snapshots in taker pricing"
```

## Task 5: Strategy Boundary And Source Fence

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Add: `tests/bolt_v3_realized_volatility_source_fence.rs`

- [ ] **Step 1: Write failing source-fence test**

Create `tests/bolt_v3_realized_volatility_source_fence.rs`:

```rust
use std::{fs, path::Path};

const FORBIDDEN_STRATEGY_RV_TERMS: &[&str] = &[
    "CrossSourceDispersion",
    "min_ready_sources",
    "max_cross_source_dispersion",
    "upper_quantile",
    "coverage_ratio",
];

#[test]
fn strategy_code_does_not_own_realized_volatility_engine_policy() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/strategies/binary_oracle_edge_taker/mod.rs");
    let source = fs::read_to_string(path).expect("strategy source should be readable");

    for forbidden in FORBIDDEN_STRATEGY_RV_TERMS {
        assert!(
            !source.contains(forbidden),
            "strategy code must not own realized-volatility policy term `{forbidden}`"
        );
    }
}
```

- [ ] **Step 2: Run test to verify RED or current PASS**

Run:

```bash
cargo test --test bolt_v3_realized_volatility_source_fence -- --nocapture
```

Expected: PASS before integration; it must stay PASS during integration.

- [ ] **Step 3: Wire strategy to forward observations**

In `src/strategies/binary_oracle_edge_taker/mod.rs`, add only forwarding code:

- build `RealizedVolObservation` from configured source ticks
- call `RealizedVolEngine::observe`
- call `snapshot_at(now_ms)`
- pass the snapshot into `TakerPricingState`

Do not implement source quorum, dispersion, coverage, aggregation, or RV blockers in strategy code.

- [ ] **Step 4: Run source-fence and pricing tests**

Run:

```bash
cargo test --test bolt_v3_realized_volatility_source_fence -- --nocapture
cargo test --lib pricing_state -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/strategies/binary_oracle_edge_taker/mod.rs tests/bolt_v3_realized_volatility_source_fence.rs
git commit -m "feat: forward realized volatility observations from strategy"
```

## Task 6: RV Evidence

**Files:**
- Modify: `src/bolt_v3_decision_evidence.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Test: `tests/bolt_v3_decision_evidence.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`

- [ ] **Step 1: Write failing evidence serialization test**

Add to `tests/bolt_v3_decision_evidence.rs`:

```rust
#[test]
fn strategy_input_evidence_records_realized_volatility_snapshot_provenance() {
    let line = fixture_strategy_input_snapshot_line_with_realized_volatility_snapshot();

    assert_eq!(line.realized_volatility_surface_id, "<surface_id>");
    assert_eq!(line.realized_volatility_annualized_decimal, "2.5");
    assert_eq!(line.realized_volatility_aggregation, "upper_quantile");
    assert_eq!(line.realized_volatility_sources_used, vec!["<SOURCE_ID_A>".to_string()]);
    assert!(line.realized_volatility_blockers.is_empty());
}
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test --test bolt_v3_decision_evidence strategy_input_evidence_records_realized_volatility_snapshot_provenance -- --nocapture
```

Expected: FAIL because evidence fields do not exist.

- [ ] **Step 3: Add evidence fields**

Add RV snapshot fields to strategy input and entry/exit evidence structures:

- `realized_volatility_surface_id`
- `realized_volatility_as_of_ms`
- `realized_volatility_annualized_decimal`
- `realized_volatility_seconds_per_annum`
- `realized_volatility_aggregation`
- `realized_volatility_sources_used`
- `realized_volatility_source_diagnostics`
- `realized_volatility_blockers`
- `realized_volatility_config_fingerprint`

- [ ] **Step 4: Run evidence tests**

Run:

```bash
cargo test --test bolt_v3_decision_evidence -- --nocapture
cargo test --lib source_evidence -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/bolt_v3_decision_evidence.rs src/strategies/binary_oracle_edge_taker/mod.rs tests/bolt_v3_decision_evidence.rs src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs
git commit -m "feat: record realized volatility snapshot evidence"
```

## Final Verification

- [ ] Run formatting:

```bash
cargo fmt --check
```

- [ ] Run clippy:

```bash
cargo clippy --locked --lib -- -D warnings
```

- [ ] Run focused tests:

```bash
cargo test --test bolt_v3_realized_volatility -- --nocapture
cargo test --test bolt_v3_taker_pricing -- --nocapture
cargo test --test bolt_v3_strategy_registration realized_volatility -- --nocapture
cargo test --test bolt_v3_decision_evidence realized_volatility -- --nocapture
cargo test --test bolt_v3_realized_volatility_source_fence -- --nocapture
```

- [ ] Run full library suite:

```bash
cargo test --lib -- --nocapture
```

- [ ] Check whitespace:

```bash
git diff --check
```

## Self-Review Notes

- The plan uses placeholder ids by design because concrete asset or venue
  examples are prohibited for this feature.
- The implementation is RV-specific and does not introduce generic volatility,
  IV, or broad market-state abstractions.
- The strategy boundary is enforced by an explicit source-fence test.
- The migration removes the taker strategy-owned internal RV estimator instead
  of allowing two live RV authorities.
