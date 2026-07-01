use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use serde::{Deserialize, Serialize};

use super::{
    authz::IvSelectorAuthorization,
    bounds::IvNumericBounds,
    derive::{
        IvDeriveError, IvDerivedInputPolicy, IvDerivedInputSet, IvDerivedOutput, IvHelperPolicy,
        derive_iv, resolve_derived_input_policy, select_helper_policy,
    },
    error::IvRejectReason,
    health::{IvSourceHealth, IvSourceHealthState},
    ingest::{IvIngestEvent, IvRawEvent},
    policy::{
        IvFallbackCandidate, IvFallbackPolicy, IvInterpolationPolicy, IvPolicyInput,
        IvPolicyOutput, IvProjectionPolicy, IvQuorumPolicy, IvStrikeSelection, interpolate_smile,
        project_scalar, resolve_fallback, resolve_quorum,
    },
    provenance::{IvPolicyDecision, IvProvenance, validate_iv_provenance},
    selector::IvSelector,
    store::{
        IvAggregateGreeks, IvEvidence, IvGreeksPoint, IvPoint, IvRetentionPolicy, IvSmile, IvStore,
        IvStoreError, IvSurface, evicted_surface_keys, surface_retention_key,
    },
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvProductKind},
};

const INITIAL_REJECT_COUNT: u64 = 0;
const REJECT_COUNT_INCREMENT: u64 = 1;
const EMPTY_RETENTION_START: usize = usize::MIN;
const RETENTION_MISS_MAX_INDEXED_PRODUCT_KINDS: &[IvProductKind] = &[
    IvProductKind::IvPoint,
    IvProductKind::IvGreeksPoint,
    IvProductKind::AggregateGreeks,
    IvProductKind::CustomIvEvidence,
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IvQuery {
    Product(Box<IvProductQuery>),
    RawPayload(IvRawPayloadQuery),
}

impl IvQuery {
    pub fn product(query: IvProductQuery) -> Self {
        Self::Product(Box::new(query))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvProductQuery {
    pub strategy_id: String,
    pub profile_id: String,
    pub product_kind: IvProductKind,
    pub selector: IvSelector,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvRawPayloadQuery {
    pub strategy_id: String,
    pub profile_id: String,
    pub raw_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IvQueryProduct {
    IvPoint(IvPoint),
    IvGreeksPoint(IvGreeksPoint),
    Smile(IvSmile),
    Surface(IvSurface),
    AggregateGreeks(IvAggregateGreeks),
    CustomIvEvidence(IvEvidence),
    ProjectedScalarIv(IvProjectedScalarIv),
    DerivedIv(Box<IvDerivedOutput>),
    DerivedInputDiagnostics(IvDerivedInputDiagnostics),
    SourceHealth(IvSourceHealth),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvDerivedInputDiagnostics {
    pub profile_id: String,
    pub inputs: Vec<IvDerivedInputSet>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvProjectedScalarIv {
    pub profile_id: String,
    pub source_id: String,
    pub selector_fingerprint: String,
    pub projection_policy_id: String,
    pub value: f64,
    pub as_of_ns: UnixNanos,
    pub provenance: IvProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IvQueryError {
    ProfileMismatch,
    ProductKindMismatch,
    ProductNotFound,
    RetentionMiss,
    ProjectionPolicyNotFound,
    ProjectionRejected,
    HelperPolicyNotFound,
    DerivedInputNotFound,
    DerivationRejected,
    RawPayloadRejected,
    StrategyNotAuthorized,
    UnsupportedProductKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvQueryState {
    store: IvStore,
    source_health: Vec<IvSourceHealth>,
    retention_misses: BTreeSet<IvRetainedProductKey>,
    query_rejections: Vec<IvPolicyDecision>,
    query_rejections_start: usize,
    projection_policies: Vec<IvProjectionPolicy>,
    interpolation_policies: Vec<IvInterpolationPolicy>,
    fallback_policies: Vec<IvFallbackPolicy>,
    quorum_policies: Vec<IvQuorumPolicy>,
    helper_policies: Vec<IvHelperPolicy>,
    derived_input_policies: Vec<IvDerivedInputPolicy>,
    derived_inputs: Vec<IvDerivedInputSet>,
    derived_outputs: Vec<IvDerivedOutput>,
    derived_outputs_start: usize,
    current_subscription_generations: BTreeMap<String, u64>,
}

#[derive(Debug, Clone)]
pub struct IvQueryStateHandle {
    inner: Arc<RwLock<IvQueryState>>,
}

#[derive(Debug, Clone)]
pub struct IvQueryHandle {
    profile_id: String,
    authorization: IvSelectorAuthorization,
    state: IvQueryStateHandle,
    retention_policy: Option<IvRetentionPolicy>,
}

#[derive(Debug, Clone)]
pub struct IvStrategyQueryHandle {
    inner: IvQueryHandle,
}

impl IvStrategyQueryHandle {
    pub(crate) fn new(inner: IvQueryHandle) -> Self {
        Self { inner }
    }

    pub fn authorization(&self) -> &IvSelectorAuthorization {
        self.inner.authorization()
    }

    pub fn query(&self, query: &IvQuery) -> Result<IvQueryProduct, IvQueryError> {
        self.inner.query(query)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct IvRetainedProductKey {
    ts_event_ns: UnixNanos,
    subscription_generation: u64,
    profile_id: String,
    source_id: String,
    selector_fingerprint: String,
    instrument_id: String,
    surface_selector: String,
    basis: IvBasis,
    product_kind: IvProductKind,
    product_key: String,
}

// Query evaluation holds a read lock; mutations are replayed after the guard
// drops to avoid cloning the retained store or upgrading the lock.
#[derive(Debug)]
struct IvQuerySideEffects {
    effects: Vec<IvQuerySideEffect>,
}

#[derive(Debug)]
enum IvQuerySideEffect {
    RetentionMiss(IvRetainedProductKey),
    QueryRejection {
        provenance: Box<IvProvenance>,
        reject_reason: IvRejectReason,
    },
    SourceRejection {
        profile_id: String,
        source_id: String,
        subscription_generation: u64,
        ts_event_ns: UnixNanos,
        reject_reason: IvRejectReason,
        mark_rejected: bool,
    },
    DerivedOutput(Box<IvDerivedOutput>),
    EnforceRetention,
}

struct IvDerivedQueryKey<'a> {
    instrument_id: &'a str,
    helper_policy_id: &'a str,
    as_of_ns: UnixNanos,
    request_inputs: Option<&'a IvDerivedInputSet>,
}

impl IvQuerySideEffects {
    fn new() -> Self {
        Self {
            effects: Vec::new(),
        }
    }

    fn record_retention_miss(&mut self, miss: IvRetainedProductKey) {
        self.effects.push(IvQuerySideEffect::RetentionMiss(miss));
        self.enforce_retention();
    }

    fn record_query_rejection(&mut self, provenance: &IvProvenance, reject_reason: IvRejectReason) {
        self.effects.push(IvQuerySideEffect::QueryRejection {
            provenance: Box::new(provenance.clone()),
            reject_reason,
        });
        self.enforce_retention();
    }

    fn record_derived_rejection(
        &mut self,
        inputs: &IvDerivedInputSet,
        reject_reason: IvRejectReason,
    ) {
        self.effects.push(IvQuerySideEffect::SourceRejection {
            profile_id: inputs.profile_id.clone(),
            source_id: inputs.source_id.clone(),
            subscription_generation: inputs.subscription_generation,
            ts_event_ns: inputs.as_of_ns,
            reject_reason,
            mark_rejected: false,
        });
        self.enforce_retention();
    }

    fn record_derived_output(&mut self, output: IvDerivedOutput) {
        self.effects
            .push(IvQuerySideEffect::DerivedOutput(Box::new(output)));
    }

    fn enforce_retention(&mut self) {
        self.effects.push(IvQuerySideEffect::EnforceRetention);
    }

    fn discard_derived_outputs(&mut self) {
        self.effects
            .retain(|effect| !matches!(effect, IvQuerySideEffect::DerivedOutput(_)));
    }

    fn apply(self, handle: &IvQueryHandle) {
        for effect in self.effects {
            match effect {
                IvQuerySideEffect::RetentionMiss(miss) => {
                    handle.state.record_retention_miss(&miss);
                }
                IvQuerySideEffect::QueryRejection {
                    provenance,
                    reject_reason,
                } => {
                    handle
                        .state
                        .record_query_rejection(provenance.as_ref(), reject_reason);
                }
                IvQuerySideEffect::SourceRejection {
                    profile_id,
                    source_id,
                    subscription_generation,
                    ts_event_ns,
                    reject_reason,
                    mark_rejected,
                } => {
                    handle.state.record_source_rejection(
                        profile_id,
                        source_id,
                        subscription_generation,
                        ts_event_ns,
                        reject_reason,
                        mark_rejected,
                    );
                }
                IvQuerySideEffect::DerivedOutput(output) => {
                    handle.state.record_derived_output(*output);
                }
                IvQuerySideEffect::EnforceRetention => {
                    handle.enforce_retention_policy();
                }
            }
        }
    }
}

impl IvQueryState {
    pub fn new(store: IvStore) -> Self {
        Self {
            store,
            source_health: Vec::new(),
            retention_misses: BTreeSet::new(),
            query_rejections: Vec::new(),
            query_rejections_start: EMPTY_RETENTION_START,
            projection_policies: Vec::new(),
            interpolation_policies: Vec::new(),
            fallback_policies: Vec::new(),
            quorum_policies: Vec::new(),
            helper_policies: Vec::new(),
            derived_input_policies: Vec::new(),
            derived_inputs: Vec::new(),
            derived_outputs: Vec::new(),
            derived_outputs_start: EMPTY_RETENTION_START,
            current_subscription_generations: BTreeMap::new(),
        }
    }

    pub fn with_projection_policies(
        mut self,
        projection_policies: Vec<IvProjectionPolicy>,
    ) -> Self {
        self.projection_policies = projection_policies;
        self
    }

    pub fn with_helper_policies(mut self, helper_policies: Vec<IvHelperPolicy>) -> Self {
        self.helper_policies = helper_policies;
        self
    }

    pub fn with_derived_input_policies(
        mut self,
        derived_input_policies: Vec<IvDerivedInputPolicy>,
    ) -> Self {
        self.derived_input_policies = derived_input_policies;
        self
    }

    pub fn with_interpolation_policies(
        mut self,
        interpolation_policies: Vec<IvInterpolationPolicy>,
    ) -> Self {
        self.interpolation_policies = interpolation_policies;
        self
    }

    pub fn with_fallback_policies(mut self, fallback_policies: Vec<IvFallbackPolicy>) -> Self {
        self.fallback_policies = fallback_policies;
        self
    }

    pub fn with_quorum_policies(mut self, quorum_policies: Vec<IvQuorumPolicy>) -> Self {
        self.quorum_policies = quorum_policies;
        self
    }

    pub fn with_derived_inputs(mut self, derived_inputs: Vec<IvDerivedInputSet>) -> Self {
        self.derived_inputs = derived_inputs;
        self
    }

    pub fn with_source_health(mut self, source_health: Vec<IvSourceHealth>) -> Self {
        self.source_health = source_health;
        self
    }

    pub fn with_current_subscription_generations(
        mut self,
        current_subscription_generations: BTreeMap<String, u64>,
    ) -> Self {
        self.current_subscription_generations = current_subscription_generations;
        self
    }
}

impl IvQueryStateHandle {
    pub fn new(state: IvQueryState) -> Self {
        Self {
            inner: Arc::new(RwLock::new(state)),
        }
    }

    fn read_state(&self) -> RwLockReadGuard<'_, IvQueryState> {
        self.inner.read().expect("IV query state lock poisoned")
    }

    fn write_state(&self) -> RwLockWriteGuard<'_, IvQueryState> {
        self.inner.write().expect("IV query state lock poisoned")
    }

    pub fn snapshot(&self) -> IvQueryState {
        self.read_state().clone()
    }

    pub fn ingest_event(&self, event: IvIngestEvent) -> Result<IvRawEvent, IvStoreError> {
        self.write_state().store.ingest_event(event)
    }

    pub fn raw_event_count(&self) -> usize {
        self.read_state().store.raw_events().len()
    }

    pub fn replace_source_health(&self, source_health: Vec<IvSourceHealth>) {
        self.write_state().source_health = source_health;
    }

    pub fn upsert_source_health(&self, source_health: IvSourceHealth) {
        let mut state = self.write_state();
        if state
            .current_subscription_generations
            .get(&source_health.source_id)
            .is_some_and(|current_generation| {
                source_health.subscription_generation != *current_generation
            })
        {
            return;
        }
        if let Some(existing) = state.source_health.iter_mut().find(|existing| {
            existing.profile_id == source_health.profile_id
                && existing.source_id == source_health.source_id
                && existing.subscription_generation == source_health.subscription_generation
        }) {
            merge_source_health_update(existing, source_health);
        } else {
            state.source_health.push(source_health);
        }
    }

    pub fn record_source_rejection(
        &self,
        profile_id: String,
        source_id: String,
        subscription_generation: u64,
        last_event_ts_ns: super::time::UnixNanos,
        reject_reason: super::error::IvRejectReason,
        mark_rejected: bool,
    ) {
        let mut state = self.write_state();
        if let Some(existing) = state.source_health.iter_mut().find(|existing| {
            existing.profile_id == profile_id
                && existing.source_id == source_id
                && existing.subscription_generation == subscription_generation
        }) {
            existing.last_event_ts_ns = Some(last_event_ts_ns);
            existing.last_reject_reason = Some(reject_reason);
            *existing
                .reject_counts
                .entry(reject_reason)
                .or_insert(INITIAL_REJECT_COUNT) += REJECT_COUNT_INCREMENT;
            apply_source_rejection_flags(existing, reject_reason, mark_rejected);
            return;
        }

        let mut reject_counts = BTreeMap::new();
        reject_counts.insert(reject_reason, REJECT_COUNT_INCREMENT);
        let subscription_state = if mark_rejected {
            IvSourceHealthState::Rejected
        } else if reject_reason == IvRejectReason::StaleData {
            IvSourceHealthState::Stale
        } else {
            IvSourceHealthState::Active
        };
        state.source_health.push(IvSourceHealth {
            profile_id,
            source_id,
            subscription_state,
            last_event_ts_ns: Some(last_event_ts_ns),
            last_reject_reason: Some(reject_reason),
            reject_counts,
            stale_state: reject_reason == IvRejectReason::StaleData,
            retention_state: reject_reason == IvRejectReason::RetentionMiss,
            subscription_generation,
        });
    }

    pub fn record_source_rejection_diagnostic(
        &self,
        profile_id: String,
        source_id: String,
        subscription_generation: u64,
        last_event_ts_ns: super::time::UnixNanos,
        reject_reason: super::error::IvRejectReason,
    ) {
        let mut state = self.write_state();
        if let Some(existing) = state.source_health.iter_mut().find(|existing| {
            existing.profile_id == profile_id
                && existing.source_id == source_id
                && existing.subscription_generation == subscription_generation
        }) {
            existing.last_event_ts_ns = Some(last_event_ts_ns);
            existing.last_reject_reason = Some(reject_reason);
            *existing
                .reject_counts
                .entry(reject_reason)
                .or_insert(INITIAL_REJECT_COUNT) += REJECT_COUNT_INCREMENT;
            return;
        }

        let mut reject_counts = BTreeMap::new();
        reject_counts.insert(reject_reason, REJECT_COUNT_INCREMENT);
        state.source_health.push(IvSourceHealth {
            profile_id,
            source_id,
            subscription_state: rejection_health_state(reject_reason),
            last_event_ts_ns: Some(last_event_ts_ns),
            last_reject_reason: Some(reject_reason),
            reject_counts,
            stale_state: reject_reason == IvRejectReason::StaleData,
            retention_state: reject_reason == IvRejectReason::RetentionMiss,
            subscription_generation,
        });
    }

    pub fn record_query_rejection(&self, provenance: &IvProvenance, reject_reason: IvRejectReason) {
        let mut state = self.write_state();
        record_query_rejection_locked(&mut state, provenance, reject_reason);
    }

    fn record_retention_miss(&self, miss: &IvRetainedProductKey) {
        let mut state = self.write_state();
        if state.retention_misses.contains(miss) {
            record_source_rejection_locked(
                &mut state,
                &miss.profile_id,
                &miss.source_id,
                miss.subscription_generation,
                miss.ts_event_ns,
                IvRejectReason::RetentionMiss,
            );
        }
    }

    pub fn source_health_for(&self, profile_id: &str, source_id: &str) -> Option<IvSourceHealth> {
        let state = self.read_state();
        select_source_health(&state, profile_id, source_id).cloned()
    }

    pub fn enforce_retention(&self, policy: &IvRetentionPolicy) {
        let mut state = self.write_state();
        record_retention_misses(&mut state, policy);
        retain_retention_misses(&mut state, policy);
        state.store.enforce_retention(policy);
        let derived_outputs_start = state.derived_outputs_start;
        state.derived_outputs_start = retain_with_logical_start(
            &mut state.derived_outputs,
            derived_outputs_start,
            policy.max_derived_points,
        );
        let query_rejections_start = state.query_rejections_start;
        state.query_rejections_start = retain_with_logical_start(
            &mut state.query_rejections,
            query_rejections_start,
            policy.max_source_health_events,
        );
        let current_subscription_generations = state.current_subscription_generations.clone();
        retain_source_health_events(
            &mut state.source_health,
            &current_subscription_generations,
            policy.max_source_health_events,
        );
    }

    pub fn derived_outputs(&self) -> Vec<IvDerivedOutput> {
        let state = self.read_state();
        active_slice(&state.derived_outputs, state.derived_outputs_start).to_vec()
    }

    pub fn derived_inputs(&self) -> Vec<IvDerivedInputSet> {
        self.read_state().derived_inputs.clone()
    }

    pub fn query_rejections(&self) -> Vec<IvPolicyDecision> {
        let state = self.read_state();
        active_slice(&state.query_rejections, state.query_rejections_start).to_vec()
    }

    pub fn record_derived_output(&self, output: IvDerivedOutput) {
        if validate_iv_provenance(&output.provenance).is_err()
            || validate_iv_provenance(&output.point.provenance).is_err()
        {
            return;
        }
        let mut state = self.write_state();
        let derived_outputs_start = state.derived_outputs_start.min(state.derived_outputs.len());
        let derived_outputs = &mut state.derived_outputs[derived_outputs_start..];
        if let Some(existing) = derived_outputs
            .iter_mut()
            .find(|existing| same_derived_output_cache_slot(existing, &output))
        {
            *existing = output;
        } else {
            state.derived_outputs.push(output);
        }
    }

    pub fn set_projection_policies(&self, projection_policies: Vec<IvProjectionPolicy>) {
        self.write_state().projection_policies = projection_policies;
    }

    pub fn set_input_bounds(&self, input_bounds: IvNumericBounds) {
        self.write_state().store.set_input_bounds(input_bounds);
    }

    pub fn set_helper_policies(&self, helper_policies: Vec<IvHelperPolicy>) {
        self.write_state().helper_policies = helper_policies;
    }

    pub fn set_derived_input_policies(&self, derived_input_policies: Vec<IvDerivedInputPolicy>) {
        self.write_state().derived_input_policies = derived_input_policies;
    }

    pub fn set_interpolation_policies(&self, interpolation_policies: Vec<IvInterpolationPolicy>) {
        self.write_state().interpolation_policies = interpolation_policies;
    }

    pub fn set_fallback_policies(&self, fallback_policies: Vec<IvFallbackPolicy>) {
        self.write_state().fallback_policies = fallback_policies;
    }

    pub fn set_quorum_policies(&self, quorum_policies: Vec<IvQuorumPolicy>) {
        self.write_state().quorum_policies = quorum_policies;
    }

    pub fn set_derived_inputs(&self, derived_inputs: Vec<IvDerivedInputSet>) {
        self.write_state().derived_inputs = derived_inputs;
    }

    pub fn set_current_subscription_generations(
        &self,
        current_subscription_generations: BTreeMap<String, u64>,
    ) {
        self.write_state().current_subscription_generations = current_subscription_generations;
    }

    pub fn mark_sources_removed(
        &self,
        profile_id: &str,
        source_generations: &BTreeMap<String, u64>,
    ) {
        let mut state = self.write_state();
        for (source_id, subscription_generation) in source_generations {
            let removed_health = IvSourceHealth {
                profile_id: profile_id.to_string(),
                source_id: source_id.clone(),
                subscription_state: super::health::IvSourceHealthState::Removed,
                last_event_ts_ns: None,
                last_reject_reason: None,
                reject_counts: BTreeMap::new(),
                stale_state: false,
                retention_state: false,
                subscription_generation: *subscription_generation,
            };
            if let Some(existing) = state.source_health.iter_mut().find(|health| {
                health.profile_id == profile_id
                    && health.source_id == *source_id
                    && health.subscription_generation == *subscription_generation
            }) {
                transition_source_health_state(
                    existing,
                    super::health::IvSourceHealthState::Removed,
                );
            } else {
                state.source_health.push(removed_health);
            }
        }
    }
}

impl IvQueryHandle {
    pub fn new(
        profile_id: impl Into<String>,
        authorization: IvSelectorAuthorization,
        store: IvStore,
    ) -> Self {
        Self::from_state(
            profile_id,
            authorization,
            IvQueryStateHandle::new(IvQueryState::new(store)),
        )
    }

    pub fn from_state(
        profile_id: impl Into<String>,
        authorization: IvSelectorAuthorization,
        state: IvQueryStateHandle,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            authorization,
            state,
            retention_policy: None,
        }
    }

    pub fn with_retention_policy(mut self, retention_policy: IvRetentionPolicy) -> Self {
        self.retention_policy = Some(retention_policy);
        self.enforce_retention_policy();
        self
    }

    pub fn with_source_health(self, source_health: Vec<IvSourceHealth>) -> Self {
        self.state.replace_source_health(source_health);
        self
    }

    pub fn with_projection_policies(self, projection_policies: Vec<IvProjectionPolicy>) -> Self {
        self.state.set_projection_policies(projection_policies);
        self
    }

    pub fn with_helper_policies(self, helper_policies: Vec<IvHelperPolicy>) -> Self {
        self.state.set_helper_policies(helper_policies);
        self
    }

    pub fn with_derived_input_policies(
        self,
        derived_input_policies: Vec<IvDerivedInputPolicy>,
    ) -> Self {
        self.state
            .set_derived_input_policies(derived_input_policies);
        self
    }

    pub fn with_interpolation_policies(
        self,
        interpolation_policies: Vec<IvInterpolationPolicy>,
    ) -> Self {
        self.state
            .set_interpolation_policies(interpolation_policies);
        self
    }

    pub fn with_fallback_policies(self, fallback_policies: Vec<IvFallbackPolicy>) -> Self {
        self.state.set_fallback_policies(fallback_policies);
        self
    }

    pub fn with_quorum_policies(self, quorum_policies: Vec<IvQuorumPolicy>) -> Self {
        self.state.set_quorum_policies(quorum_policies);
        self
    }

    pub fn with_derived_inputs(self, derived_inputs: Vec<IvDerivedInputSet>) -> Self {
        self.state.set_derived_inputs(derived_inputs);
        self
    }

    pub fn with_current_subscription_generations(
        self,
        current_subscription_generations: BTreeMap<String, u64>,
    ) -> Self {
        self.state
            .set_current_subscription_generations(current_subscription_generations);
        self
    }

    pub fn source_health_for(&self, profile_id: &str, source_id: &str) -> Option<IvSourceHealth> {
        if profile_id != self.profile_id
            || !self.authorization.authorizes(
                &self.authorization.strategy_id,
                IvProductKind::SourceHealth,
                Some(source_id),
                "",
            )
        {
            return None;
        }
        self.state.source_health_for(profile_id, source_id)
    }

    pub fn derived_outputs(&self) -> Vec<IvDerivedOutput> {
        self.state
            .derived_outputs()
            .into_iter()
            .filter(|output| {
                self.authorization.authorizes(
                    &self.authorization.strategy_id,
                    IvProductKind::DerivedIv,
                    Some(&output.point.source_id),
                    &output.point.provenance.selector_fingerprint,
                )
            })
            .collect()
    }

    pub fn derived_inputs(&self) -> Vec<IvDerivedInputSet> {
        self.state
            .derived_inputs()
            .into_iter()
            .filter(|inputs| {
                self.authorization.authorizes(
                    &self.authorization.strategy_id,
                    IvProductKind::DerivedIv,
                    Some(&inputs.source_id),
                    &inputs.selector_fingerprint,
                )
            })
            .collect()
    }

    pub fn query_rejections(&self) -> Vec<IvPolicyDecision> {
        if !self.authorization.is_profile_wide() {
            return Vec::new();
        }
        self.state.query_rejections()
    }

    pub fn authorization(&self) -> &IvSelectorAuthorization {
        &self.authorization
    }

    pub fn query(&self, query: &IvQuery) -> Result<IvQueryProduct, IvQueryError> {
        match query {
            IvQuery::Product(query) => self.query_product(query),
            IvQuery::RawPayload(query) => {
                if query.profile_id != self.profile_id {
                    return Err(IvQueryError::ProfileMismatch);
                }
                if query.strategy_id != self.authorization.strategy_id {
                    return Err(IvQueryError::StrategyNotAuthorized);
                }
                Err(IvQueryError::RawPayloadRejected)
            }
        }
    }

    fn query_product(&self, query: &IvProductQuery) -> Result<IvQueryProduct, IvQueryError> {
        if query.profile_id != self.profile_id {
            return Err(IvQueryError::ProfileMismatch);
        }
        if query.strategy_id != self.authorization.strategy_id {
            return Err(IvQueryError::StrategyNotAuthorized);
        }
        if !selector_supports_product_kind(&query.selector, query.product_kind) {
            return Err(IvQueryError::ProductKindMismatch);
        }

        let (result, side_effects) = {
            let state = self.state.read_state();
            let mut side_effects = IvQuerySideEffects::new();
            let result = match self.query_product_from_state(query, &state, &mut side_effects) {
                Ok(product) => {
                    if let IvQueryProduct::DerivedIv(derived) = &product
                        && should_cache_derived_output(query)
                    {
                        side_effects.record_derived_output((**derived).clone());
                        side_effects.enforce_retention();
                    }
                    Ok(product)
                }
                Err(IvQueryError::RetentionMiss) => {
                    if let Some(miss) = retention_miss_for_query(&state, query) {
                        if self.authorization.authorizes(
                            &query.strategy_id,
                            query.product_kind,
                            Some(&miss.source_id),
                            &miss.selector_fingerprint,
                        ) {
                            side_effects.record_retention_miss(miss);
                            Err(IvQueryError::RetentionMiss)
                        } else {
                            Err(IvQueryError::ProductNotFound)
                        }
                    } else {
                        Err(IvQueryError::RetentionMiss)
                    }
                }
                Err(error) => Err(error),
            };
            if result.is_err() {
                side_effects.discard_derived_outputs();
            }
            (result, side_effects)
        };

        side_effects.apply(self);
        result
    }

    fn query_product_from_state(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
        side_effects: &mut IvQuerySideEffects,
    ) -> Result<IvQueryProduct, IvQueryError> {
        let mut product = self.find_product(query, state, side_effects)?;
        if matches!(product, IvQueryProduct::DerivedInputDiagnostics(_)) {
            return Ok(product);
        }
        let product_is_current = product_satisfies_current_state(&product, state);
        let product_is_authorized = product_is_current
            && self.authorization.authorizes(
                &query.strategy_id,
                query.product_kind,
                product.source_id(),
                product.selector_fingerprint(),
            );
        if !product_is_authorized {
            if let Some(authorized_product) =
                self.find_authorized_current_product(query, state, side_effects)?
            {
                product = authorized_product;
            } else if product_is_current {
                return Err(IvQueryError::StrategyNotAuthorized);
            } else {
                let stale_product_is_authorized = self.authorization.authorizes(
                    &query.strategy_id,
                    query.product_kind,
                    product.source_id(),
                    product.selector_fingerprint(),
                );
                if stale_product_is_authorized && let Some(provenance) = product.provenance() {
                    side_effects.record_query_rejection(provenance, IvRejectReason::StaleData);
                }
                return Err(IvQueryError::ProductNotFound);
            }
        }

        if !self.authorization.authorizes(
            &query.strategy_id,
            query.product_kind,
            product.source_id(),
            product.selector_fingerprint(),
        ) {
            return Err(IvQueryError::StrategyNotAuthorized);
        }

        Ok(product)
    }

    fn find_authorized_current_product(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
        side_effects: &mut IvQuerySideEffects,
    ) -> Result<Option<IvQueryProduct>, IvQueryError> {
        let products = match (&query.product_kind, &query.selector) {
            (
                IvProductKind::SourceHealth,
                IvSelector::SourceHealthQuery {
                    source_filter,
                    state_filter,
                },
            ) => matching_source_health_products(
                state,
                &query.profile_id,
                source_filter,
                state_filter,
            ),
            _ => match self.find_projection_products(query, state, side_effects) {
                Ok(products) => products,
                Err(IvQueryError::ProductNotFound) => return Ok(None),
                Err(error) => return Err(error),
            },
        };

        Ok(products.into_iter().find(|product| {
            product_satisfies_current_state(product, state)
                && self.authorization.authorizes(
                    &query.strategy_id,
                    query.product_kind,
                    product.source_id(),
                    product.selector_fingerprint(),
                )
        }))
    }

    fn find_product(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
        side_effects: &mut IvQuerySideEffects,
    ) -> Result<IvQueryProduct, IvQueryError> {
        match (&query.product_kind, &query.selector) {
            (
                IvProductKind::IvPoint,
                IvSelector::PointQuery {
                    instrument_ids,
                    basis,
                    as_of_ns,
                    source_filter,
                },
            ) => state
                .store
                .iv_points()
                .iter()
                .find(|point| {
                    point.profile_id == query.profile_id
                        && instrument_ids.contains(&point.instrument_id)
                        && point.basis == *basis
                        && point.ts_event_ns == *as_of_ns
                        && source_matches(&point.source_id, source_filter)
                })
                .cloned()
                .map(IvQueryProduct::IvPoint)
                .ok_or_else(|| {
                    retention_miss_for_query(state, query)
                        .map_or(IvQueryError::ProductNotFound, |_| {
                            IvQueryError::RetentionMiss
                        })
                }),
            (
                IvProductKind::IvGreeksPoint,
                IvSelector::PointQuery {
                    instrument_ids,
                    basis,
                    as_of_ns,
                    source_filter,
                },
            ) => state
                .store
                .greeks_points()
                .iter()
                .find(|point| {
                    point.point.profile_id == query.profile_id
                        && instrument_ids.contains(&point.point.instrument_id)
                        && point.point.basis == *basis
                        && point.point.ts_event_ns == *as_of_ns
                        && source_matches(&point.point.source_id, source_filter)
                })
                .cloned()
                .map(IvQueryProduct::IvGreeksPoint)
                .ok_or_else(|| {
                    retention_miss_for_query(state, query)
                        .map_or(IvQueryError::ProductNotFound, |_| {
                            IvQueryError::RetentionMiss
                        })
                }),
            (
                IvProductKind::Smile,
                IvSelector::SmileQuery {
                    series_id,
                    side,
                    basis,
                    as_of_ns,
                },
            ) => matching_smile_products(
                state,
                &query.profile_id,
                series_id,
                side,
                *basis,
                *as_of_ns,
            )
            .into_iter()
            .next()
            .ok_or_else(|| retained_or_not_found(state, query)),
            (
                IvProductKind::Surface,
                IvSelector::SurfaceQuery {
                    series_selectors,
                    basis,
                    as_of_ns,
                },
            ) => matching_surface_products(
                state,
                &query.profile_id,
                series_selectors,
                *basis,
                *as_of_ns,
            )
            .into_iter()
            .next()
            .ok_or_else(|| retained_or_not_found(state, query)),
            (
                IvProductKind::AggregateGreeks,
                IvSelector::AggregateGreeksQuery {
                    aggregate_key,
                    underlying_selectors,
                    as_of_ns,
                },
            ) => state
                .store
                .aggregate_greeks()
                .iter()
                .find(|aggregate| {
                    aggregate.profile_id == query.profile_id
                        && aggregate.aggregate_key == *aggregate_key
                        && aggregate.underlying_selectors == *underlying_selectors
                        && aggregate.ts_event_ns == *as_of_ns
                })
                .cloned()
                .map(IvQueryProduct::AggregateGreeks)
                .ok_or_else(|| retained_or_not_found(state, query)),
            (
                IvProductKind::CustomIvEvidence,
                IvSelector::IvEvidenceQuery {
                    iv_evidence_kind,
                    source_filter,
                    as_of_ns,
                },
            ) => state
                .store
                .iv_evidence()
                .iter()
                .find(|evidence| {
                    evidence.profile_id == query.profile_id
                        && evidence.iv_evidence_kind == *iv_evidence_kind
                        && evidence.ts_event_ns == *as_of_ns
                        && source_matches(&evidence.source_id, source_filter)
                })
                .cloned()
                .map(IvQueryProduct::CustomIvEvidence)
                .ok_or_else(|| retained_or_not_found(state, query)),
            (
                IvProductKind::SourceHealth,
                IvSelector::SourceHealthQuery {
                    source_filter,
                    state_filter,
                },
            ) => matching_source_health_products(
                state,
                &query.profile_id,
                source_filter,
                state_filter,
            )
            .into_iter()
            .next()
            .ok_or(IvQueryError::ProductNotFound),
            (
                IvProductKind::ProjectedScalarIv,
                IvSelector::ProjectedScalarIvQuery {
                    input_selector,
                    projection_policy_id,
                    as_of_ns,
                },
            ) => self.project_scalar_query(
                query,
                state,
                side_effects,
                input_selector,
                projection_policy_id,
                *as_of_ns,
            ),
            (
                IvProductKind::DerivedIv,
                IvSelector::DerivedIvQuery {
                    instrument_id,
                    helper_policy_id,
                    as_of_ns,
                    inputs,
                },
            ) => self.derived_iv_query(
                query,
                state,
                side_effects,
                IvDerivedQueryKey {
                    instrument_id,
                    helper_policy_id,
                    as_of_ns: *as_of_ns,
                    request_inputs: inputs.as_deref(),
                },
            ),
            (
                IvProductKind::DerivedInputDiagnostics,
                IvSelector::DerivedInputDiagnosticsQuery {
                    instrument_id,
                    as_of_ns,
                    source_filter,
                },
            ) => self.derived_input_diagnostics_query(
                query,
                state,
                instrument_id.as_deref(),
                *as_of_ns,
                source_filter,
            ),
            _ => Err(IvQueryError::ProductKindMismatch),
        }
    }

    fn project_scalar_query(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
        side_effects: &mut IvQuerySideEffects,
        input_selector: &IvSelector,
        projection_policy_id: &str,
        as_of_ns: UnixNanos,
    ) -> Result<IvQueryProduct, IvQueryError> {
        if matches!(input_selector, IvSelector::ProjectedScalarIvQuery { .. }) {
            return Err(IvQueryError::UnsupportedProductKind);
        }
        let policy = state
            .projection_policies
            .iter()
            .find(|policy| policy.policy_id == projection_policy_id)
            .ok_or(IvQueryError::ProjectionPolicyNotFound)?;
        let input_query = IvProductQuery {
            strategy_id: query.strategy_id.clone(),
            profile_id: query.profile_id.clone(),
            product_kind: input_selector.product_kind(),
            selector: input_selector.clone(),
        };
        let mut input_products = self.find_projection_input_products(
            &input_query,
            query,
            state,
            side_effects,
            projection_input_timestamp_tolerance(policy, state),
        )?;
        input_products.retain(|product| product_satisfies_current_state(product, state));
        if input_products.is_empty() {
            return Err(IvQueryError::ProductNotFound);
        }
        let all_inputs = projection_inputs_from_products(&input_products)?;
        if !projection_inputs_authorized(
            &self.authorization,
            &query.strategy_id,
            query.product_kind,
            &all_inputs,
        ) {
            return Err(IvQueryError::StrategyNotAuthorized);
        }
        let projection_products = deduplicate_projection_products(input_products.clone(), as_of_ns);
        let mut inputs = projection_inputs_from_products(&projection_products)?;
        let mut fallback_inputs = inputs.clone();
        let mut policy_decisions = Vec::new();

        let mut interpolation_rejected = false;
        if let Some(interpolation_policy_ref) = &policy.interpolation_policy_ref {
            match interpolate_projected_inputs(
                policy,
                state,
                interpolation_policy_ref,
                &projection_products,
            )? {
                ProjectedInputInterpolation::Interpolated(interpolated) => {
                    inputs = interpolated.inputs;
                    fallback_inputs = inputs.clone();
                    policy_decisions.extend(interpolated.policy_decisions);
                }
                ProjectedInputInterpolation::Rejected => {
                    interpolation_rejected = true;
                }
                ProjectedInputInterpolation::NotApplicable => {}
            }
        }

        if interpolation_rejected {
            let output = fallback_only(policy, state, &input_products, &fallback_inputs)?;
            let mut provenance = projected_output_provenance(&input_products, &output)?;
            provenance.policy_decisions.extend(policy_decisions);
            provenance.policy_decisions.extend(output.policy_decisions);
            provenance
                .transformation_steps
                .push(projection_policy_id.to_string());
            provenance.ts_event_ns = as_of_ns;
            validate_projected_output_provenance(&provenance)?;

            return Ok(IvQueryProduct::ProjectedScalarIv(IvProjectedScalarIv {
                profile_id: query.profile_id.clone(),
                source_id: provenance.source_id.clone(),
                selector_fingerprint: provenance.selector_fingerprint.clone(),
                projection_policy_id: projection_policy_id.to_string(),
                value: output.value,
                as_of_ns,
                provenance,
            }));
        }

        if let Some(quorum_policy_ref) = &policy.quorum_policy_ref {
            let quorum_policy = state
                .quorum_policies
                .iter()
                .find(|policy| policy.policy_id == *quorum_policy_ref)
                .ok_or(IvQueryError::ProjectionRejected)?;
            let quorum_output = match resolve_quorum(quorum_policy, &inputs) {
                Ok(output) => output,
                Err(_) => return Err(IvQueryError::ProjectionRejected),
            };
            inputs = quorum_filtered_inputs(quorum_policy, &inputs);
            fallback_inputs = inputs.clone();
            policy_decisions.extend(quorum_output.policy_decisions);
        }

        let output =
            project_or_fallback(policy, state, &input_products, &inputs, &fallback_inputs)?;
        let provenance_products = if output.selected_input.is_some() {
            &input_products
        } else {
            &projection_products
        };
        let mut provenance = projected_output_provenance(provenance_products, &output)?;
        provenance.policy_decisions.extend(policy_decisions);
        provenance.policy_decisions.extend(output.policy_decisions);
        provenance
            .transformation_steps
            .push(projection_policy_id.to_string());
        provenance.ts_event_ns = as_of_ns;
        validate_projected_output_provenance(&provenance)?;

        Ok(IvQueryProduct::ProjectedScalarIv(IvProjectedScalarIv {
            profile_id: query.profile_id.clone(),
            source_id: provenance.source_id.clone(),
            selector_fingerprint: provenance.selector_fingerprint.clone(),
            projection_policy_id: projection_policy_id.to_string(),
            value: output.value,
            as_of_ns,
            provenance,
        }))
    }

    fn find_projection_products(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
        side_effects: &mut IvQuerySideEffects,
    ) -> Result<Vec<IvQueryProduct>, IvQueryError> {
        self.find_projection_products_with_tolerance(query, None, state, side_effects, None)
    }

    fn find_projection_input_products(
        &self,
        query: &IvProductQuery,
        projection_query: &IvProductQuery,
        state: &IvQueryState,
        side_effects: &mut IvQuerySideEffects,
        tolerance_ns: u64,
    ) -> Result<Vec<IvQueryProduct>, IvQueryError> {
        self.find_projection_products_with_tolerance(
            query,
            Some(projection_query),
            state,
            side_effects,
            Some(tolerance_ns),
        )
    }

    fn find_projection_products_with_tolerance(
        &self,
        query: &IvProductQuery,
        projection_query: Option<&IvProductQuery>,
        state: &IvQueryState,
        side_effects: &mut IvQuerySideEffects,
        tolerance_ns: Option<u64>,
    ) -> Result<Vec<IvQueryProduct>, IvQueryError> {
        match (&query.product_kind, &query.selector) {
            (
                IvProductKind::IvPoint,
                IvSelector::PointQuery {
                    instrument_ids,
                    basis,
                    as_of_ns,
                    source_filter,
                },
            ) => {
                let products = state
                    .store
                    .iv_points()
                    .iter()
                    .filter(|point| {
                        point.profile_id == query.profile_id
                            && instrument_ids.contains(&point.instrument_id)
                            && point.basis == *basis
                            && timestamp_matches(point.ts_event_ns, *as_of_ns, tolerance_ns)
                            && source_matches(&point.source_id, source_filter)
                    })
                    .cloned()
                    .map(IvQueryProduct::IvPoint)
                    .collect::<Vec<_>>();
                if products.is_empty() {
                    Err(IvQueryError::ProductNotFound)
                } else {
                    Ok(products)
                }
            }
            (
                IvProductKind::IvGreeksPoint,
                IvSelector::PointQuery {
                    instrument_ids,
                    basis,
                    as_of_ns,
                    source_filter,
                },
            ) => {
                let products = state
                    .store
                    .greeks_points()
                    .iter()
                    .filter(|point| {
                        point.point.profile_id == query.profile_id
                            && instrument_ids.contains(&point.point.instrument_id)
                            && point.point.basis == *basis
                            && timestamp_matches(point.point.ts_event_ns, *as_of_ns, tolerance_ns)
                            && source_matches(&point.point.source_id, source_filter)
                    })
                    .cloned()
                    .map(IvQueryProduct::IvGreeksPoint)
                    .collect::<Vec<_>>();
                if products.is_empty() {
                    Err(IvQueryError::ProductNotFound)
                } else {
                    Ok(products)
                }
            }
            (
                IvProductKind::Smile,
                IvSelector::SmileQuery {
                    series_id,
                    side,
                    basis,
                    as_of_ns,
                },
            ) => {
                let products = matching_smile_products_with_tolerance(
                    state,
                    &query.profile_id,
                    series_id,
                    side,
                    *basis,
                    *as_of_ns,
                    tolerance_ns,
                );
                if products.is_empty() {
                    Err(IvQueryError::ProductNotFound)
                } else {
                    Ok(products)
                }
            }
            (
                IvProductKind::Surface,
                IvSelector::SurfaceQuery {
                    series_selectors,
                    basis,
                    as_of_ns,
                },
            ) => {
                let products = matching_surface_products_with_tolerance(
                    state,
                    &query.profile_id,
                    series_selectors,
                    *basis,
                    *as_of_ns,
                    tolerance_ns,
                );
                if products.is_empty() {
                    Err(IvQueryError::ProductNotFound)
                } else {
                    Ok(products)
                }
            }
            (
                IvProductKind::CustomIvEvidence,
                IvSelector::IvEvidenceQuery {
                    iv_evidence_kind,
                    source_filter,
                    as_of_ns,
                },
            ) => {
                let products = state
                    .store
                    .iv_evidence()
                    .iter()
                    .filter(|evidence| {
                        evidence.profile_id == query.profile_id
                            && evidence.iv_evidence_kind == *iv_evidence_kind
                            && timestamp_matches(evidence.ts_event_ns, *as_of_ns, tolerance_ns)
                            && source_matches(&evidence.source_id, source_filter)
                    })
                    .cloned()
                    .map(IvQueryProduct::CustomIvEvidence)
                    .collect::<Vec<_>>();
                if products.is_empty() {
                    Err(IvQueryError::ProductNotFound)
                } else {
                    Ok(products)
                }
            }
            (
                IvProductKind::AggregateGreeks,
                IvSelector::AggregateGreeksQuery {
                    aggregate_key,
                    underlying_selectors,
                    as_of_ns,
                },
            ) => {
                let products = state
                    .store
                    .aggregate_greeks()
                    .iter()
                    .filter(|aggregate| {
                        aggregate.profile_id == query.profile_id
                            && aggregate.aggregate_key == *aggregate_key
                            && aggregate.underlying_selectors == *underlying_selectors
                            && timestamp_matches(aggregate.ts_event_ns, *as_of_ns, tolerance_ns)
                    })
                    .cloned()
                    .map(IvQueryProduct::AggregateGreeks)
                    .collect::<Vec<_>>();
                if products.is_empty() {
                    Err(IvQueryError::ProductNotFound)
                } else {
                    Ok(products)
                }
            }
            (
                IvProductKind::DerivedIv,
                IvSelector::DerivedIvQuery {
                    instrument_id,
                    helper_policy_id,
                    as_of_ns,
                    inputs,
                },
            ) => self.find_derived_projection_products(
                query,
                projection_query.unwrap_or(query),
                state,
                side_effects,
                IvDerivedQueryKey {
                    instrument_id,
                    helper_policy_id,
                    as_of_ns: *as_of_ns,
                    request_inputs: inputs.as_deref(),
                },
            ),
            _ => self
                .find_product(query, state, side_effects)
                .map(|product| vec![product]),
        }
    }

    fn find_derived_projection_products(
        &self,
        query: &IvProductQuery,
        authorization_query: &IvProductQuery,
        state: &IvQueryState,
        side_effects: &mut IvQuerySideEffects,
        derived_query: IvDerivedQueryKey<'_>,
    ) -> Result<Vec<IvQueryProduct>, IvQueryError> {
        if let Some(inputs) = derived_query.request_inputs {
            if inputs.profile_id != query.profile_id
                || inputs.instrument_id != derived_query.instrument_id
                || inputs.as_of_ns != derived_query.as_of_ns
            {
                return Err(IvQueryError::DerivedInputNotFound);
            }
            if !self.authorizes_derived_input(authorization_query, inputs) {
                return Err(IvQueryError::StrategyNotAuthorized);
            }
            return self
                .derive_iv_from_inputs(
                    state,
                    side_effects,
                    derived_query.helper_policy_id,
                    inputs.clone(),
                )
                .map(|derived| vec![IvQueryProduct::DerivedIv(Box::new(derived))]);
        }

        let matching_outputs = active_slice(&state.derived_outputs, state.derived_outputs_start)
            .iter()
            .filter(|derived| {
                derived_output_matches_query(
                    derived,
                    &query.profile_id,
                    derived_query.instrument_id,
                    derived_query.helper_policy_id,
                    derived_query.as_of_ns,
                )
            });
        let mut unauthorized_candidate = false;
        let mut outputs = Vec::new();
        for derived in matching_outputs {
            if self.authorizes_derived_output(authorization_query, derived) {
                outputs.push(derived.clone());
            } else {
                unauthorized_candidate = true;
            }
        }

        let mut derived_any = false;
        let mut first_derivation_error = None;
        for inputs in state.derived_inputs.iter().filter(|inputs| {
            inputs.profile_id == query.profile_id
                && inputs.instrument_id == derived_query.instrument_id
                && inputs.as_of_ns == derived_query.as_of_ns
        }) {
            if !self.authorizes_derived_input(authorization_query, inputs) {
                unauthorized_candidate = true;
                continue;
            }
            if outputs.iter().any(|output| {
                derived_output_matches_input(output, inputs, derived_query.helper_policy_id)
            }) {
                continue;
            }
            let output = match self.derive_iv_from_inputs(
                state,
                side_effects,
                derived_query.helper_policy_id,
                inputs.clone(),
            ) {
                Ok(output) => output,
                Err(IvQueryError::DerivationRejected) => {
                    first_derivation_error.get_or_insert(IvQueryError::DerivationRejected);
                    continue;
                }
                Err(error) => return Err(error),
            };
            side_effects.record_derived_output(output.clone());
            outputs.push(output);
            derived_any = true;
        }

        if derived_any {
            side_effects.enforce_retention();
        }

        if outputs.is_empty() && unauthorized_candidate && first_derivation_error.is_none() {
            Err(IvQueryError::StrategyNotAuthorized)
        } else if outputs.is_empty() {
            Err(first_derivation_error.unwrap_or(IvQueryError::ProductNotFound))
        } else {
            Ok(outputs
                .into_iter()
                .map(|derived| IvQueryProduct::DerivedIv(Box::new(derived)))
                .collect())
        }
    }

    fn derived_iv_query(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
        side_effects: &mut IvQuerySideEffects,
        derived_query: IvDerivedQueryKey<'_>,
    ) -> Result<IvQueryProduct, IvQueryError> {
        let inputs = if let Some(inputs) = derived_query.request_inputs {
            if inputs.profile_id != query.profile_id
                || inputs.instrument_id != derived_query.instrument_id
                || inputs.as_of_ns != derived_query.as_of_ns
            {
                return Err(IvQueryError::DerivedInputNotFound);
            }
            if !self.authorizes_derived_input(query, inputs) {
                return Err(IvQueryError::StrategyNotAuthorized);
            }
            inputs.clone()
        } else {
            let mut unauthorized_candidate = false;
            state
                .derived_inputs
                .iter()
                .find(|inputs| {
                    let matches_query = inputs.profile_id == query.profile_id
                        && inputs.instrument_id == derived_query.instrument_id
                        && inputs.as_of_ns == derived_query.as_of_ns;
                    if matches_query && !self.authorizes_derived_input(query, inputs) {
                        unauthorized_candidate = true;
                        return false;
                    }
                    matches_query
                })
                .cloned()
                .ok_or(if unauthorized_candidate {
                    IvQueryError::StrategyNotAuthorized
                } else if retention_miss_for_query(state, query).is_some() {
                    IvQueryError::RetentionMiss
                } else {
                    IvQueryError::DerivedInputNotFound
                })?
        };
        self.derive_iv_from_inputs(state, side_effects, derived_query.helper_policy_id, inputs)
            .map(|output| IvQueryProduct::DerivedIv(Box::new(output)))
    }

    fn authorizes_derived_input(&self, query: &IvProductQuery, inputs: &IvDerivedInputSet) -> bool {
        self.authorization.authorizes(
            &query.strategy_id,
            query.product_kind,
            Some(&inputs.source_id),
            &inputs.selector_fingerprint,
        )
    }

    fn authorizes_derived_output(&self, query: &IvProductQuery, output: &IvDerivedOutput) -> bool {
        self.authorization.authorizes(
            &query.strategy_id,
            query.product_kind,
            Some(&output.point.source_id),
            &output.point.provenance.selector_fingerprint,
        )
    }

    fn derived_input_diagnostics_query(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
        instrument_id: Option<&str>,
        as_of_ns: Option<UnixNanos>,
        source_filter: &Option<String>,
    ) -> Result<IvQueryProduct, IvQueryError> {
        let inputs = state
            .derived_inputs
            .iter()
            .filter(|inputs| {
                inputs.profile_id == query.profile_id
                    && instrument_id.is_none_or(|expected| inputs.instrument_id == expected)
                    && as_of_ns.is_none_or(|expected| inputs.as_of_ns == expected)
                    && source_matches(&inputs.source_id, source_filter)
                    && self.authorization.authorizes(
                        &query.strategy_id,
                        query.product_kind,
                        Some(&inputs.source_id),
                        &inputs.selector_fingerprint,
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        if inputs.is_empty() {
            return Err(IvQueryError::ProductNotFound);
        }
        Ok(IvQueryProduct::DerivedInputDiagnostics(
            IvDerivedInputDiagnostics {
                profile_id: query.profile_id.clone(),
                inputs,
            },
        ))
    }

    fn derive_iv_from_inputs(
        &self,
        state: &IvQueryState,
        side_effects: &mut IvQuerySideEffects,
        helper_policy_id: &str,
        inputs: IvDerivedInputSet,
    ) -> Result<IvDerivedOutput, IvQueryError> {
        let policy = select_helper_policy(&state.helper_policies, helper_policy_id)
            .map_err(|_| IvQueryError::HelperPolicyNotFound)?;
        let Some(input_policy) = state
            .derived_input_policies
            .iter()
            .find(|input_policy| input_policy.input_policy_id == policy.input_policy_ref)
        else {
            side_effects.record_derived_rejection(&inputs, IvRejectReason::HelperNotConfigured);
            return Err(IvQueryError::DerivationRejected);
        };
        if !derived_input_satisfies_current_state(&inputs, state) {
            side_effects.record_derived_rejection(&inputs, IvRejectReason::StaleData);
            return Err(IvQueryError::DerivationRejected);
        }
        let current_derived_inputs = state
            .derived_inputs
            .iter()
            .filter(|candidate| derived_input_satisfies_current_state(candidate, state))
            .cloned()
            .collect::<Vec<_>>();
        let inputs = match resolve_derived_input_policy(
            input_policy,
            inputs.clone(),
            &current_derived_inputs,
        ) {
            Ok(inputs) => inputs,
            Err(error) => {
                side_effects.record_derived_rejection(&inputs, derive_reject_reason(&error));
                return Err(IvQueryError::DerivationRejected);
            }
        };
        match derive_iv(policy, inputs.clone()) {
            Ok(output) => Ok(output),
            Err(error) => {
                side_effects.record_derived_rejection(&inputs, derive_reject_reason(&error));
                Err(IvQueryError::DerivationRejected)
            }
        }
    }

    fn enforce_retention_policy(&self) {
        if let Some(policy) = self.retention_policy {
            self.state.enforce_retention(&policy);
        }
    }

    pub fn raw_event(&self, _raw_event_id: &str) -> Result<&IvRawEvent, IvQueryError> {
        Err(IvQueryError::RawPayloadRejected)
    }
}

impl IvQueryProduct {
    fn source_id(&self) -> Option<&str> {
        match self {
            Self::IvPoint(point) => Some(&point.source_id),
            Self::IvGreeksPoint(point) => Some(&point.point.source_id),
            Self::Smile(smile) => Some(&smile.source_id),
            Self::Surface(surface) => Some(&surface.source_id),
            Self::AggregateGreeks(aggregate) => Some(&aggregate.source_id),
            Self::CustomIvEvidence(evidence) => Some(&evidence.source_id),
            Self::ProjectedScalarIv(projected) => Some(&projected.source_id),
            Self::DerivedIv(derived) => Some(&derived.point.source_id),
            Self::DerivedInputDiagnostics(_) => None,
            Self::SourceHealth(health) => Some(&health.source_id),
        }
    }

    fn selector_fingerprint(&self) -> &str {
        match self {
            Self::IvPoint(point) => &point.provenance.selector_fingerprint,
            Self::IvGreeksPoint(point) => &point.point.provenance.selector_fingerprint,
            Self::Smile(smile) => &smile.provenance.selector_fingerprint,
            Self::Surface(surface) => &surface.provenance.selector_fingerprint,
            Self::AggregateGreeks(aggregate) => &aggregate.provenance.selector_fingerprint,
            Self::CustomIvEvidence(evidence) => &evidence.provenance.selector_fingerprint,
            Self::ProjectedScalarIv(projected) => &projected.selector_fingerprint,
            Self::DerivedIv(derived) => &derived.point.provenance.selector_fingerprint,
            Self::DerivedInputDiagnostics(_) => "",
            Self::SourceHealth(_) => "",
        }
    }

    fn provenance(&self) -> Option<&IvProvenance> {
        match self {
            Self::IvPoint(point) => Some(&point.provenance),
            Self::IvGreeksPoint(point) => Some(&point.point.provenance),
            Self::Smile(smile) => Some(&smile.provenance),
            Self::Surface(surface) => Some(&surface.provenance),
            Self::AggregateGreeks(aggregate) => Some(&aggregate.provenance),
            Self::CustomIvEvidence(evidence) => Some(&evidence.provenance),
            Self::ProjectedScalarIv(projected) => Some(&projected.provenance),
            Self::DerivedIv(derived) => Some(&derived.provenance),
            Self::DerivedInputDiagnostics(_) => None,
            Self::SourceHealth(_) => None,
        }
    }
}

fn matching_smile_products(
    state: &IvQueryState,
    profile_id: &str,
    series_id: &str,
    side: &Option<String>,
    basis: IvBasis,
    as_of_ns: UnixNanos,
) -> Vec<IvQueryProduct> {
    matching_smile_products_with_tolerance(
        state, profile_id, series_id, side, basis, as_of_ns, None,
    )
}

fn matching_smile_products_with_tolerance(
    state: &IvQueryState,
    profile_id: &str,
    series_id: &str,
    side: &Option<String>,
    basis: IvBasis,
    as_of_ns: UnixNanos,
    tolerance_ns: Option<u64>,
) -> Vec<IvQueryProduct> {
    state
        .store
        .smiles()
        .iter()
        .filter(|smile| {
            smile.profile_id == profile_id
                && smile.series_id == series_id
                && side.as_ref().is_none_or(|side| smile.side == *side)
                && smile.basis == basis
                && timestamp_matches(smile.ts_event_ns, as_of_ns, tolerance_ns)
        })
        .cloned()
        .map(IvQueryProduct::Smile)
        .collect()
}

fn matching_surface_products(
    state: &IvQueryState,
    profile_id: &str,
    series_selectors: &[String],
    basis: IvBasis,
    as_of_ns: UnixNanos,
) -> Vec<IvQueryProduct> {
    matching_surface_products_with_tolerance(
        state,
        profile_id,
        series_selectors,
        basis,
        as_of_ns,
        None,
    )
}

fn matching_surface_products_with_tolerance(
    state: &IvQueryState,
    profile_id: &str,
    series_selectors: &[String],
    basis: IvBasis,
    as_of_ns: UnixNanos,
    tolerance_ns: Option<u64>,
) -> Vec<IvQueryProduct> {
    let mut seen = BTreeSet::new();
    state
        .store
        .smiles()
        .iter()
        .filter(|smile| {
            smile.profile_id == profile_id
                && series_selectors.contains(&smile.surface_selector)
                && smile.basis == basis
                && timestamp_matches(smile.ts_event_ns, as_of_ns, tolerance_ns)
        })
        .filter_map(|smile| {
            let key = (
                smile.surface_selector.clone(),
                smile.source_id.clone(),
                smile.ts_event_ns,
            );
            if !seen.insert(key) {
                return None;
            }
            state
                .store
                .surface(
                    &smile.surface_selector,
                    &smile.source_id,
                    basis,
                    smile.ts_event_ns,
                )
                .map(IvQueryProduct::Surface)
        })
        .collect()
}

fn deduplicate_projection_products(
    products: Vec<IvQueryProduct>,
    as_of_ns: UnixNanos,
) -> Vec<IvQueryProduct> {
    let mut selected = Vec::new();
    let mut selected_indices = BTreeMap::<ProjectionProductDedupKey, usize>::new();
    for product in products {
        let Some(key) = projection_product_dedup_key(&product) else {
            continue;
        };
        match selected_indices.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(selected.len());
                selected.push(product);
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                let selected_index = *entry.get();
                if projection_product_is_closer(&product, &selected[selected_index], as_of_ns) {
                    selected[selected_index] = product;
                }
            }
        }
    }
    selected
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ProjectionProductDedupKey {
    IvPoint {
        instrument_id: String,
        basis: IvBasis,
        source_id: String,
    },
    IvGreeksPoint {
        instrument_id: String,
        basis: IvBasis,
        source_id: String,
    },
    Smile {
        series_id: String,
        side: String,
        basis: IvBasis,
        source_id: String,
    },
    Surface {
        surface_selector: String,
        basis: IvBasis,
        source_id: String,
    },
    AggregateGreeks {
        aggregate_key: String,
        underlying_selectors: Vec<String>,
        source_id: String,
    },
    CustomIvEvidence {
        iv_evidence_kind: String,
        source_id: String,
    },
    ProjectedScalarIv {
        projection_policy_id: String,
        source_id: String,
    },
    DerivedIv {
        instrument_id: String,
        helper_policy_id: String,
        basis: IvBasis,
        source_id: String,
    },
}

fn projection_product_dedup_key(product: &IvQueryProduct) -> Option<ProjectionProductDedupKey> {
    match product {
        IvQueryProduct::IvPoint(point) => Some(ProjectionProductDedupKey::IvPoint {
            instrument_id: point.instrument_id.clone(),
            basis: point.basis,
            source_id: point.source_id.clone(),
        }),
        IvQueryProduct::IvGreeksPoint(point) => Some(ProjectionProductDedupKey::IvGreeksPoint {
            instrument_id: point.point.instrument_id.clone(),
            basis: point.point.basis,
            source_id: point.point.source_id.clone(),
        }),
        IvQueryProduct::Smile(smile) => Some(ProjectionProductDedupKey::Smile {
            series_id: smile.series_id.clone(),
            side: smile.side.clone(),
            basis: smile.basis,
            source_id: smile.source_id.clone(),
        }),
        IvQueryProduct::Surface(surface) => Some(ProjectionProductDedupKey::Surface {
            surface_selector: surface.surface_selector.clone(),
            basis: surface.basis,
            source_id: surface.source_id.clone(),
        }),
        IvQueryProduct::AggregateGreeks(aggregate) => {
            Some(ProjectionProductDedupKey::AggregateGreeks {
                aggregate_key: aggregate.aggregate_key.clone(),
                underlying_selectors: aggregate.underlying_selectors.clone(),
                source_id: aggregate.source_id.clone(),
            })
        }
        IvQueryProduct::CustomIvEvidence(evidence) => {
            Some(ProjectionProductDedupKey::CustomIvEvidence {
                iv_evidence_kind: evidence.iv_evidence_kind.clone(),
                source_id: evidence.source_id.clone(),
            })
        }
        IvQueryProduct::ProjectedScalarIv(projected) => {
            Some(ProjectionProductDedupKey::ProjectedScalarIv {
                projection_policy_id: projected.projection_policy_id.clone(),
                source_id: projected.source_id.clone(),
            })
        }
        IvQueryProduct::DerivedIv(derived) => Some(ProjectionProductDedupKey::DerivedIv {
            instrument_id: derived.point.instrument_id.clone(),
            helper_policy_id: derived.helper_identity.helper_policy_id.clone(),
            basis: derived.point.basis,
            source_id: derived.point.source_id.clone(),
        }),
        IvQueryProduct::DerivedInputDiagnostics(_) | IvQueryProduct::SourceHealth(_) => None,
    }
}

fn projection_product_is_closer(
    candidate: &IvQueryProduct,
    selected: &IvQueryProduct,
    as_of_ns: UnixNanos,
) -> bool {
    let Some(candidate_ts) = projection_product_ts_event_ns(candidate) else {
        return false;
    };
    let Some(selected_ts) = projection_product_ts_event_ns(selected) else {
        return true;
    };
    let candidate_distance = candidate_ts.get().abs_diff(as_of_ns.get());
    let selected_distance = selected_ts.get().abs_diff(as_of_ns.get());
    candidate_distance < selected_distance
        || (candidate_distance == selected_distance && candidate_ts.get() > selected_ts.get())
}

fn projection_product_ts_event_ns(product: &IvQueryProduct) -> Option<UnixNanos> {
    match product {
        IvQueryProduct::Surface(surface) => Some(surface.as_of_ns),
        _ => product
            .provenance()
            .map(|provenance| provenance.ts_event_ns),
    }
}

fn timestamp_matches(
    candidate_ts: UnixNanos,
    query_ts: UnixNanos,
    tolerance_ns: Option<u64>,
) -> bool {
    match tolerance_ns {
        Some(tolerance_ns) => {
            candidate_ts.get() <= query_ts.get()
                && query_ts.get().saturating_sub(candidate_ts.get()) <= tolerance_ns
        }
        None => candidate_ts == query_ts,
    }
}

fn matching_source_health_products(
    state: &IvQueryState,
    profile_id: &str,
    source_filter: &Option<String>,
    state_filter: &[String],
) -> Vec<IvQueryProduct> {
    if let Some(source_id) = source_filter {
        return select_source_health(state, profile_id, source_id)
            .filter(|health| source_health_state_matches(health, state_filter))
            .cloned()
            .map(IvQueryProduct::SourceHealth)
            .into_iter()
            .collect();
    }

    let source_ids = state
        .source_health
        .iter()
        .filter(|health| health.profile_id == profile_id)
        .map(|health| health.source_id.clone())
        .collect::<BTreeSet<_>>();

    source_ids
        .into_iter()
        .filter_map(|source_id| select_source_health(state, profile_id, &source_id))
        .filter(|health| source_health_state_matches(health, state_filter))
        .cloned()
        .map(IvQueryProduct::SourceHealth)
        .collect()
}

fn projection_inputs(product: &IvQueryProduct) -> Result<Vec<IvPolicyInput>, IvQueryError> {
    let inputs = match product {
        IvQueryProduct::IvPoint(point) => vec![IvPolicyInput {
            product_id: point.instrument_id.clone(),
            source_id: point.source_id.clone(),
            selector_fingerprint: point.provenance.selector_fingerprint.clone(),
            basis: format!("{:?}", point.basis),
            convention: iv_convention_name(&point.convention),
            value: point.iv,
            ts_event_ns: point.ts_event_ns,
        }],
        IvQueryProduct::IvGreeksPoint(point) => vec![IvPolicyInput {
            product_id: point.point.instrument_id.clone(),
            source_id: point.point.source_id.clone(),
            selector_fingerprint: point.point.provenance.selector_fingerprint.clone(),
            basis: format!("{:?}", point.point.basis),
            convention: iv_convention_name(&point.point.convention),
            value: point.point.iv,
            ts_event_ns: point.point.ts_event_ns,
        }],
        IvQueryProduct::Smile(smile) => smile
            .points_by_strike
            .iter()
            .map(|point| IvPolicyInput {
                product_id: smile.series_id.clone(),
                source_id: smile.source_id.clone(),
                selector_fingerprint: smile.provenance.selector_fingerprint.clone(),
                basis: format!("{:?}", smile.basis),
                convention: iv_convention_name(&smile.convention),
                value: point.iv,
                ts_event_ns: smile.ts_event_ns,
            })
            .collect(),
        IvQueryProduct::Surface(surface) => surface
            .smiles
            .iter()
            .flat_map(|smile| {
                smile.points_by_strike.iter().map(|point| IvPolicyInput {
                    product_id: smile.series_id.clone(),
                    source_id: smile.source_id.clone(),
                    selector_fingerprint: smile.provenance.selector_fingerprint.clone(),
                    basis: format!("{:?}", smile.basis),
                    convention: iv_convention_name(&smile.convention),
                    value: point.iv,
                    ts_event_ns: smile.ts_event_ns,
                })
            })
            .collect(),
        IvQueryProduct::AggregateGreeks(aggregate) => {
            let Some(aggregate_iv) = &aggregate.aggregate_iv else {
                return Err(IvQueryError::ProjectionRejected);
            };
            vec![IvPolicyInput {
                product_id: aggregate.aggregate_key.clone(),
                source_id: aggregate.source_id.clone(),
                selector_fingerprint: aggregate.provenance.selector_fingerprint.clone(),
                basis: format!("{:?}", aggregate_iv.basis),
                convention: iv_convention_name(&aggregate_iv.convention),
                value: aggregate_iv.value,
                ts_event_ns: aggregate.ts_event_ns,
            }]
        }
        IvQueryProduct::CustomIvEvidence(evidence) => vec![IvPolicyInput {
            product_id: evidence.iv_evidence_kind.clone(),
            source_id: evidence.source_id.clone(),
            selector_fingerprint: evidence.provenance.selector_fingerprint.clone(),
            basis: evidence.iv_evidence_kind.clone(),
            convention: evidence.provenance.nt_symbol.clone(),
            value: evidence.value,
            ts_event_ns: evidence.ts_event_ns,
        }],
        IvQueryProduct::DerivedIv(derived) => vec![IvPolicyInput {
            product_id: derived.point.instrument_id.clone(),
            source_id: derived.point.source_id.clone(),
            selector_fingerprint: derived.point.provenance.selector_fingerprint.clone(),
            basis: format!("{:?}", derived.point.basis),
            convention: iv_convention_name(&derived.point.convention),
            value: derived.point.iv,
            ts_event_ns: derived.point.ts_event_ns,
        }],
        IvQueryProduct::ProjectedScalarIv(_)
        | IvQueryProduct::DerivedInputDiagnostics(_)
        | IvQueryProduct::SourceHealth(_) => return Err(IvQueryError::UnsupportedProductKind),
    };

    if inputs.is_empty() {
        Err(IvQueryError::ProductNotFound)
    } else {
        Ok(inputs)
    }
}

fn projection_inputs_from_products(
    products: &[IvQueryProduct],
) -> Result<Vec<IvPolicyInput>, IvQueryError> {
    let mut inputs = Vec::new();
    for product in products {
        inputs.extend(projection_inputs(product)?);
    }

    if inputs.is_empty() {
        Err(IvQueryError::ProductNotFound)
    } else {
        Ok(inputs)
    }
}

fn projection_inputs_authorized(
    authorization: &IvSelectorAuthorization,
    strategy_id: &str,
    product_kind: IvProductKind,
    inputs: &[IvPolicyInput],
) -> bool {
    inputs.iter().all(|input| {
        authorization.authorizes(
            strategy_id,
            product_kind,
            Some(&input.source_id),
            &input.selector_fingerprint,
        )
    })
}

fn should_cache_derived_output(query: &IvProductQuery) -> bool {
    matches!(
        &query.selector,
        IvSelector::DerivedIvQuery { inputs: None, .. }
    )
}

fn same_derived_output_cache_slot(left: &IvDerivedOutput, right: &IvDerivedOutput) -> bool {
    left.point.profile_id == right.point.profile_id
        && left.point.source_id == right.point.source_id
        && left.point.instrument_id == right.point.instrument_id
        && left.point.basis == right.point.basis
        && left.point.convention == right.point.convention
        && left.point.ts_event_ns == right.point.ts_event_ns
        && left.helper_identity.helper_policy_id == right.helper_identity.helper_policy_id
}

fn derived_output_matches_query(
    output: &IvDerivedOutput,
    profile_id: &str,
    instrument_id: &str,
    helper_policy_id: &str,
    as_of_ns: UnixNanos,
) -> bool {
    output.point.profile_id == profile_id
        && output.point.instrument_id == instrument_id
        && output.helper_identity.helper_policy_id == helper_policy_id
        && output.point.ts_event_ns == as_of_ns
}

fn derived_output_matches_input(
    output: &IvDerivedOutput,
    inputs: &IvDerivedInputSet,
    helper_policy_id: &str,
) -> bool {
    output.point.profile_id == inputs.profile_id
        && output.point.source_id == inputs.source_id
        && output.point.instrument_id == inputs.instrument_id
        && output.point.basis == inputs.basis
        && output.point.convention == inputs.convention
        && output.point.ts_event_ns == inputs.as_of_ns
        && output.helper_identity.helper_policy_id == helper_policy_id
}

fn projection_input_timestamp_tolerance(policy: &IvProjectionPolicy, state: &IvQueryState) -> u64 {
    policy
        .fallback_policy_ref
        .as_ref()
        .and_then(|fallback_policy_ref| {
            state
                .fallback_policies
                .iter()
                .find(|fallback_policy| fallback_policy.policy_id == *fallback_policy_ref)
        })
        .map_or(policy.max_projection_input_skew_ns, |fallback_policy| {
            policy
                .max_projection_input_skew_ns
                .max(fallback_policy.maximum_timestamp_skew_ns)
        })
}

fn project_or_fallback(
    policy: &IvProjectionPolicy,
    state: &IvQueryState,
    input_products: &[IvQueryProduct],
    inputs: &[IvPolicyInput],
    fallback_inputs: &[IvPolicyInput],
) -> Result<QueryPolicyOutput, IvQueryError> {
    match project_scalar(policy, inputs) {
        Ok(output) => Ok(QueryPolicyOutput::from_policy_output(output)),
        Err(_) => fallback_only(policy, state, input_products, fallback_inputs),
    }
}

struct QueryPolicyOutput {
    value: f64,
    policy_decisions: Vec<IvPolicyDecision>,
    selected_input: Option<SelectedProjectionInput>,
}

impl QueryPolicyOutput {
    fn from_policy_output(output: IvPolicyOutput) -> Self {
        Self {
            value: output.value,
            policy_decisions: output.policy_decisions,
            selected_input: None,
        }
    }
}

#[derive(Clone)]
struct SelectedProjectionInput {
    product_id: String,
    source_id: String,
    selector_fingerprint: String,
    ts_event_ns: UnixNanos,
}

impl From<&IvPolicyInput> for SelectedProjectionInput {
    fn from(input: &IvPolicyInput) -> Self {
        Self {
            product_id: input.product_id.clone(),
            source_id: input.source_id.clone(),
            selector_fingerprint: input.selector_fingerprint.clone(),
            ts_event_ns: input.ts_event_ns,
        }
    }
}

fn fallback_only(
    policy: &IvProjectionPolicy,
    state: &IvQueryState,
    input_products: &[IvQueryProduct],
    inputs: &[IvPolicyInput],
) -> Result<QueryPolicyOutput, IvQueryError> {
    let Some(fallback_policy_ref) = &policy.fallback_policy_ref else {
        return Err(IvQueryError::ProjectionRejected);
    };
    let fallback_policy = state
        .fallback_policies
        .iter()
        .find(|fallback_policy| fallback_policy.policy_id == *fallback_policy_ref)
        .ok_or(IvQueryError::ProjectionRejected)?;
    let candidates = inputs
        .iter()
        .map(|input| IvFallbackCandidate {
            candidate_id: input.product_id.clone(),
            value: input.value,
            eligible: fallback_policy.eligible_sources.is_empty()
                || fallback_policy.eligible_sources.contains(&input.source_id),
        })
        .collect::<Vec<_>>();
    let selected_input = fallback_policy
        .candidate_order
        .iter()
        .find_map(|candidate_id| {
            inputs
                .iter()
                .find(|input| {
                    input.product_id == *candidate_id
                        && (fallback_policy.eligible_sources.is_empty()
                            || fallback_policy.eligible_sources.contains(&input.source_id))
                })
                .map(SelectedProjectionInput::from)
        });
    if projection_input_skew(inputs) > fallback_policy.maximum_timestamp_skew_ns {
        return Err(IvQueryError::ProjectionRejected);
    }
    let Some(selected_input) = selected_input else {
        return Err(IvQueryError::ProjectionRejected);
    };
    let selected_product = selected_product_for_input(input_products, &selected_input)
        .ok_or(IvQueryError::ProjectionRejected)?;
    if !provenance_satisfies_required_fields(
        selected_product
            .provenance()
            .ok_or(IvQueryError::ProjectionRejected)?,
        &fallback_policy.required_provenance_fields,
    ) {
        return Err(IvQueryError::ProjectionRejected);
    }
    let output = resolve_fallback(fallback_policy, &candidates)
        .map_err(|_| IvQueryError::ProjectionRejected)?;
    let convention = IvConvention::Named(
        inputs
            .iter()
            .find(|input| input.product_id == selected_input.product_id)
            .ok_or(IvQueryError::ProjectionRejected)?
            .convention
            .clone(),
    );
    if !policy.output_bounds.accepts(output.value, &convention) {
        return Err(IvQueryError::ProjectionRejected);
    }
    Ok(QueryPolicyOutput {
        value: output.value,
        policy_decisions: output.policy_decisions,
        selected_input: Some(selected_input),
    })
}

fn quorum_filtered_inputs(policy: &IvQuorumPolicy, inputs: &[IvPolicyInput]) -> Vec<IvPolicyInput> {
    if policy.eligible_sources.is_empty() {
        return inputs.to_vec();
    }
    inputs
        .iter()
        .filter(|input| policy.eligible_sources.contains(&input.source_id))
        .cloned()
        .collect()
}

fn projected_output_provenance(
    input_products: &[IvQueryProduct],
    output: &QueryPolicyOutput,
) -> Result<IvProvenance, IvQueryError> {
    let selected_product = output
        .selected_input
        .as_ref()
        .and_then(|selected_input| selected_product_for_input(input_products, selected_input));
    selected_product
        .or_else(|| input_products.first())
        .ok_or(IvQueryError::ProductNotFound)?
        .provenance()
        .cloned()
        .ok_or(IvQueryError::UnsupportedProductKind)
}

fn validate_projected_output_provenance(provenance: &IvProvenance) -> Result<(), IvQueryError> {
    validate_iv_provenance(provenance).map_err(|_| IvQueryError::ProjectionRejected)?;
    if provenance.policy_decisions.is_empty() || provenance.transformation_steps.is_empty() {
        return Err(IvQueryError::ProjectionRejected);
    }
    Ok(())
}

impl SelectedProjectionInput {
    fn matches(&self, input: &IvPolicyInput) -> bool {
        self.product_id == input.product_id
            && self.source_id == input.source_id
            && self.selector_fingerprint == input.selector_fingerprint
            && self.ts_event_ns == input.ts_event_ns
    }
}

fn selected_product_for_input<'a>(
    input_products: &'a [IvQueryProduct],
    selected_input: &SelectedProjectionInput,
) -> Option<&'a IvQueryProduct> {
    input_products.iter().find(|product| {
        projection_inputs(product)
            .is_ok_and(|inputs| inputs.iter().any(|input| selected_input.matches(input)))
    })
}

fn projection_input_skew(inputs: &[IvPolicyInput]) -> u64 {
    match (
        inputs.iter().map(|input| input.ts_event_ns.get()).min(),
        inputs.iter().map(|input| input.ts_event_ns.get()).max(),
    ) {
        (Some(min), Some(max)) => max.saturating_sub(min),
        _ => 0,
    }
}

fn provenance_satisfies_required_fields(
    provenance: &IvProvenance,
    required_fields: &[String],
) -> bool {
    required_fields.iter().all(|field| match field.as_str() {
        "raw_event_id" => provenance
            .raw_event_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        "payload_kind" => provenance
            .payload_kind
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        "nt_revision" => !provenance.nt_revision.trim().is_empty(),
        "nt_symbol" => !provenance.nt_symbol.trim().is_empty(),
        "nt_evidence_path" => !provenance.nt_evidence_path.trim().is_empty(),
        "input_event_ids" => !provenance.input_event_ids.is_empty(),
        "helper_identity" => provenance.helper_identity.is_some(),
        "policy_decisions" => !provenance.policy_decisions.is_empty(),
        _ => false,
    })
}

enum ProjectedInputInterpolation {
    NotApplicable,
    Interpolated(InterpolatedProjectionInputs),
    Rejected,
}

struct InterpolatedProjectionInputs {
    inputs: Vec<IvPolicyInput>,
    policy_decisions: Vec<IvPolicyDecision>,
}

fn interpolate_projected_inputs(
    projection_policy: &IvProjectionPolicy,
    state: &IvQueryState,
    interpolation_policy_id: &str,
    input_products: &[IvQueryProduct],
) -> Result<ProjectedInputInterpolation, IvQueryError> {
    if projection_policy.strike_selection == IvStrikeSelection::AllConfiguredStrikes {
        return Ok(ProjectedInputInterpolation::NotApplicable);
    }

    let policy = state
        .interpolation_policies
        .iter()
        .find(|policy| policy.policy_id == interpolation_policy_id)
        .ok_or(IvQueryError::ProjectionRejected)?;

    let mut inputs = Vec::new();
    let mut policy_decisions = Vec::new();
    let mut saw_interpolatable_product = false;
    for product in input_products {
        match product {
            IvQueryProduct::Smile(smile) => {
                saw_interpolatable_product = true;
                interpolate_smile_input(
                    projection_policy,
                    policy,
                    smile,
                    &mut inputs,
                    &mut policy_decisions,
                );
            }
            IvQueryProduct::Surface(surface) => {
                saw_interpolatable_product = true;
                for smile in &surface.smiles {
                    interpolate_smile_input(
                        projection_policy,
                        policy,
                        smile,
                        &mut inputs,
                        &mut policy_decisions,
                    );
                }
            }
            _ => return Ok(ProjectedInputInterpolation::NotApplicable),
        }
    }

    if inputs.is_empty() {
        if saw_interpolatable_product {
            Ok(ProjectedInputInterpolation::Rejected)
        } else {
            Ok(ProjectedInputInterpolation::NotApplicable)
        }
    } else {
        Ok(ProjectedInputInterpolation::Interpolated(
            InterpolatedProjectionInputs {
                inputs,
                policy_decisions,
            },
        ))
    }
}

fn interpolate_smile_input(
    projection_policy: &IvProjectionPolicy,
    interpolation_policy: &IvInterpolationPolicy,
    smile: &IvSmile,
    inputs: &mut Vec<IvPolicyInput>,
    policy_decisions: &mut Vec<IvPolicyDecision>,
) {
    if !interpolation_policy.eligible_sources.is_empty()
        && !interpolation_policy
            .eligible_sources
            .contains(&smile.source_id)
    {
        return;
    }

    let strike = match projection_policy.strike_selection {
        IvStrikeSelection::AllConfiguredStrikes => return,
        IvStrikeSelection::AtmStrike => smile.atm_strike,
        IvStrikeSelection::FirstConfiguredStrike => {
            smile.points_by_strike.first().map(|point| point.strike)
        }
    };
    let Some(strike) = strike else {
        return;
    };

    let Ok(output) = interpolate_smile(interpolation_policy, &smile.points_by_strike, strike)
    else {
        return;
    };

    policy_decisions.extend(output.policy_decisions);
    inputs.push(IvPolicyInput {
        product_id: smile.series_id.clone(),
        source_id: smile.source_id.clone(),
        selector_fingerprint: smile.provenance.selector_fingerprint.clone(),
        basis: format!("{:?}", smile.basis),
        convention: iv_convention_name(&smile.convention),
        value: output.value,
        ts_event_ns: smile.ts_event_ns,
    });
}

fn iv_convention_name(convention: &IvConvention) -> String {
    match convention {
        IvConvention::Named(name) => name.clone(),
    }
}

fn source_matches(actual: &str, filter: &Option<String>) -> bool {
    filter.as_ref().is_none_or(|expected| actual == expected)
}

fn source_health_state_matches(health: &IvSourceHealth, state_filter: &[String]) -> bool {
    state_filter.is_empty()
        || state_filter.iter().any(|expected| {
            expected == health.subscription_state.as_str()
                || (expected == super::health::IvSourceHealthState::Rejected.as_str()
                    && health.last_reject_reason.is_some())
        })
}

fn product_satisfies_current_state(product: &IvQueryProduct, state: &IvQueryState) -> bool {
    if let IvQueryProduct::SourceHealth(health) = product {
        return select_source_health(state, &health.profile_id, &health.source_id).is_some_and(
            |current_health| {
                current_health.subscription_generation == health.subscription_generation
            },
        );
    }

    let Some(provenance) = product.provenance() else {
        return false;
    };
    if !provenance.source_health_state.can_satisfy_current_query() {
        return false;
    }
    if let Some(current_generation) = state
        .current_subscription_generations
        .get(&provenance.source_id)
        && *current_generation != provenance.subscription_generation
    {
        return false;
    }
    if let Some(current_health) =
        select_source_health(state, &provenance.profile_id, &provenance.source_id)
    {
        return current_health.can_satisfy_current_query()
            && current_health.subscription_generation == provenance.subscription_generation;
    }
    state.source_health.is_empty()
}

fn derived_input_satisfies_current_state(inputs: &IvDerivedInputSet, state: &IvQueryState) -> bool {
    if !inputs.source_health_state.can_satisfy_current_query() {
        return false;
    }
    if let Some(current_generation) = state
        .current_subscription_generations
        .get(&inputs.source_id)
        && *current_generation != inputs.subscription_generation
    {
        return false;
    }
    if let Some(current_health) = select_source_health(state, &inputs.profile_id, &inputs.source_id)
    {
        return current_health.can_satisfy_current_query()
            && current_health.subscription_generation == inputs.subscription_generation;
    }
    state.source_health.is_empty()
}

fn selector_supports_product_kind(selector: &IvSelector, product_kind: IvProductKind) -> bool {
    selector.product_kind() == product_kind
        || (product_kind == IvProductKind::IvGreeksPoint
            && matches!(selector, IvSelector::PointQuery { .. }))
}

fn derive_reject_reason(error: &IvDeriveError) -> IvRejectReason {
    match error {
        IvDeriveError::HelperPolicyNotFound { .. } => IvRejectReason::HelperNotConfigured,
        IvDeriveError::MissingInput { .. } => IvRejectReason::MissingDerivedInput,
        IvDeriveError::Rejected { reason, .. } => *reason,
    }
}

fn record_retention_misses(state: &mut IvQueryState, policy: &IvRetentionPolicy) {
    let mut misses = Vec::new();
    let evicted_iv_points = state
        .store
        .iv_points()
        .len()
        .saturating_sub(policy.max_indexed_points);
    for point in state.store.iv_points().iter().take(evicted_iv_points) {
        misses.push(retained_iv_point_key(point, IvProductKind::IvPoint));
    }

    let evicted_greeks_points = state
        .store
        .greeks_points()
        .len()
        .saturating_sub(policy.max_indexed_points);
    for point in state
        .store
        .greeks_points()
        .iter()
        .take(evicted_greeks_points)
    {
        misses.push(retained_iv_point_key(
            &point.point,
            IvProductKind::IvGreeksPoint,
        ));
    }

    let evicted_smiles = state.store.smiles().len().saturating_sub(policy.max_smiles);
    for smile in state.store.smiles().iter().take(evicted_smiles) {
        misses.push(retained_smile_key(smile));
    }
    let smiles_after_smile_limit =
        &state.store.smiles()[evicted_smiles.min(state.store.smiles().len())..];
    let evicted_surfaces = evicted_surface_keys(smiles_after_smile_limit, policy.max_surfaces);
    for smile in smiles_after_smile_limit
        .iter()
        .filter(|smile| evicted_surfaces.contains(&surface_retention_key(smile)))
    {
        misses.push(retained_smile_key(smile));
    }

    let evicted_aggregate_greeks = state
        .store
        .aggregate_greeks()
        .len()
        .saturating_sub(policy.max_indexed_points);
    for aggregate in state
        .store
        .aggregate_greeks()
        .iter()
        .take(evicted_aggregate_greeks)
    {
        misses.push(retained_aggregate_greeks_key(aggregate));
    }

    let evicted_iv_evidence = state
        .store
        .iv_evidence()
        .len()
        .saturating_sub(policy.max_indexed_points);
    for evidence in state.store.iv_evidence().iter().take(evicted_iv_evidence) {
        misses.push(retained_iv_evidence_key(evidence));
    }

    let evicted_derived_outputs = active_slice(&state.derived_outputs, state.derived_outputs_start)
        .len()
        .saturating_sub(policy.max_derived_points);
    for output in active_slice(&state.derived_outputs, state.derived_outputs_start)
        .iter()
        .take(evicted_derived_outputs)
    {
        misses.push(retained_derived_output_key(output));
    }

    for miss in misses {
        record_retained_product_miss(state, miss);
    }
}

fn record_retained_product_miss(state: &mut IvQueryState, miss: IvRetainedProductKey) {
    if state.retention_misses.insert(miss.clone()) {
        record_source_rejection_locked(
            state,
            &miss.profile_id,
            &miss.source_id,
            miss.subscription_generation,
            miss.ts_event_ns,
            IvRejectReason::RetentionMiss,
        );
    }
}

fn retain_retention_misses(state: &mut IvQueryState, policy: &IvRetentionPolicy) {
    let max_len = retention_miss_capacity(policy);
    if state.retention_misses.len() <= max_len {
        return;
    }
    if max_len == 0 {
        state.retention_misses.clear();
        return;
    }

    while state.retention_misses.len() > max_len {
        state.retention_misses.pop_first();
    }
}

fn retention_miss_capacity(policy: &IvRetentionPolicy) -> usize {
    policy
        .max_indexed_points
        .saturating_mul(RETENTION_MISS_MAX_INDEXED_PRODUCT_KINDS.len())
        .saturating_add(policy.max_smiles)
        .saturating_add(policy.max_surfaces)
        .saturating_add(policy.max_derived_points)
}

fn retained_iv_point_key(point: &IvPoint, product_kind: IvProductKind) -> IvRetainedProductKey {
    IvRetainedProductKey {
        ts_event_ns: point.ts_event_ns,
        subscription_generation: point.provenance.subscription_generation,
        profile_id: point.profile_id.clone(),
        source_id: point.source_id.clone(),
        selector_fingerprint: point.provenance.selector_fingerprint.clone(),
        instrument_id: point.instrument_id.clone(),
        surface_selector: point.instrument_id.clone(),
        basis: point.basis,
        product_kind,
        product_key: point.instrument_id.clone(),
    }
}

fn retained_smile_key(smile: &IvSmile) -> IvRetainedProductKey {
    IvRetainedProductKey {
        ts_event_ns: smile.ts_event_ns,
        subscription_generation: smile.provenance.subscription_generation,
        profile_id: smile.profile_id.clone(),
        source_id: smile.source_id.clone(),
        selector_fingerprint: smile.provenance.selector_fingerprint.clone(),
        instrument_id: smile.series_id.clone(),
        surface_selector: smile.surface_selector.clone(),
        basis: smile.basis,
        product_kind: IvProductKind::Smile,
        product_key: smile_key(&smile.series_id, smile.side.as_str()),
    }
}

fn retained_aggregate_greeks_key(aggregate: &IvAggregateGreeks) -> IvRetainedProductKey {
    IvRetainedProductKey {
        ts_event_ns: aggregate.ts_event_ns,
        subscription_generation: aggregate.provenance.subscription_generation,
        profile_id: aggregate.profile_id.clone(),
        source_id: aggregate.source_id.clone(),
        selector_fingerprint: aggregate.provenance.selector_fingerprint.clone(),
        instrument_id: aggregate.aggregate_key.clone(),
        surface_selector: aggregate.aggregate_key.clone(),
        basis: IvBasis::Mark,
        product_kind: IvProductKind::AggregateGreeks,
        product_key: aggregate_greeks_key(
            &aggregate.aggregate_key,
            &aggregate.underlying_selectors,
        ),
    }
}

fn retained_iv_evidence_key(evidence: &IvEvidence) -> IvRetainedProductKey {
    IvRetainedProductKey {
        ts_event_ns: evidence.ts_event_ns,
        subscription_generation: evidence.provenance.subscription_generation,
        profile_id: evidence.profile_id.clone(),
        source_id: evidence.source_id.clone(),
        selector_fingerprint: evidence.provenance.selector_fingerprint.clone(),
        instrument_id: evidence.iv_evidence_kind.clone(),
        surface_selector: evidence.iv_evidence_kind.clone(),
        basis: IvBasis::Mark,
        product_kind: IvProductKind::CustomIvEvidence,
        product_key: evidence.iv_evidence_kind.clone(),
    }
}

fn retained_derived_output_key(output: &IvDerivedOutput) -> IvRetainedProductKey {
    IvRetainedProductKey {
        ts_event_ns: output.point.ts_event_ns,
        subscription_generation: output.point.provenance.subscription_generation,
        profile_id: output.point.profile_id.clone(),
        source_id: output.point.source_id.clone(),
        selector_fingerprint: output.point.provenance.selector_fingerprint.clone(),
        instrument_id: output.point.instrument_id.clone(),
        surface_selector: output.point.instrument_id.clone(),
        basis: output.point.basis,
        product_kind: IvProductKind::DerivedIv,
        product_key: derived_output_key(
            &output.point.instrument_id,
            &output.helper_identity.helper_policy_id,
        ),
    }
}

fn smile_key(series_id: &str, side: &str) -> String {
    format!("{series_id}:{side}")
}

fn aggregate_greeks_key(aggregate_key: &str, underlying_selectors: &[String]) -> String {
    format!("{}:{}", aggregate_key, underlying_selectors.join("\u{1f}"))
}

fn derived_output_key(instrument_id: &str, helper_policy_id: &str) -> String {
    format!("{instrument_id}:{helper_policy_id}")
}

fn retention_miss_for_query(
    state: &IvQueryState,
    query: &IvProductQuery,
) -> Option<IvRetainedProductKey> {
    match (&query.product_kind, &query.selector) {
        (
            IvProductKind::IvPoint | IvProductKind::IvGreeksPoint,
            IvSelector::PointQuery {
                instrument_ids,
                basis,
                as_of_ns,
                source_filter,
            },
        ) => state
            .retention_misses
            .iter()
            .find(|miss| {
                miss.profile_id == query.profile_id
                    && miss.product_kind == query.product_kind
                    && instrument_ids.contains(&miss.instrument_id)
                    && miss.basis == *basis
                    && miss.ts_event_ns == *as_of_ns
                    && source_matches(&miss.source_id, source_filter)
            })
            .cloned(),
        (
            IvProductKind::Smile,
            IvSelector::SmileQuery {
                series_id,
                side,
                basis,
                as_of_ns,
            },
        ) => state
            .retention_misses
            .iter()
            .find(|miss| {
                miss.profile_id == query.profile_id
                    && miss.product_kind == IvProductKind::Smile
                    && miss.instrument_id == *series_id
                    && miss.basis == *basis
                    && miss.ts_event_ns == *as_of_ns
                    && side
                        .as_ref()
                        .is_none_or(|side| miss.product_key == smile_key(series_id, side))
            })
            .cloned(),
        (
            IvProductKind::Surface,
            IvSelector::SurfaceQuery {
                series_selectors,
                basis,
                as_of_ns,
            },
        ) => state
            .retention_misses
            .iter()
            .find(|miss| {
                miss.profile_id == query.profile_id
                    && miss.product_kind == IvProductKind::Smile
                    && series_selectors.contains(&miss.surface_selector)
                    && miss.basis == *basis
                    && miss.ts_event_ns == *as_of_ns
            })
            .cloned(),
        (
            IvProductKind::AggregateGreeks,
            IvSelector::AggregateGreeksQuery {
                aggregate_key,
                underlying_selectors,
                as_of_ns,
            },
        ) => state
            .retention_misses
            .iter()
            .find(|miss| {
                miss.profile_id == query.profile_id
                    && miss.product_kind == IvProductKind::AggregateGreeks
                    && miss.ts_event_ns == *as_of_ns
                    && miss.product_key == aggregate_greeks_key(aggregate_key, underlying_selectors)
            })
            .cloned(),
        (
            IvProductKind::CustomIvEvidence,
            IvSelector::IvEvidenceQuery {
                iv_evidence_kind,
                source_filter,
                as_of_ns,
            },
        ) => state
            .retention_misses
            .iter()
            .find(|miss| {
                miss.profile_id == query.profile_id
                    && miss.product_kind == IvProductKind::CustomIvEvidence
                    && miss.product_key == *iv_evidence_kind
                    && miss.ts_event_ns == *as_of_ns
                    && source_matches(&miss.source_id, source_filter)
            })
            .cloned(),
        (
            IvProductKind::DerivedIv,
            IvSelector::DerivedIvQuery {
                instrument_id,
                helper_policy_id,
                as_of_ns,
                ..
            },
        ) => state
            .retention_misses
            .iter()
            .find(|miss| {
                miss.profile_id == query.profile_id
                    && miss.product_kind == IvProductKind::DerivedIv
                    && miss.ts_event_ns == *as_of_ns
                    && miss.product_key == derived_output_key(instrument_id, helper_policy_id)
            })
            .cloned(),
        _ => None,
    }
}

fn retained_or_not_found(state: &IvQueryState, query: &IvProductQuery) -> IvQueryError {
    retention_miss_for_query(state, query).map_or(IvQueryError::ProductNotFound, |_| {
        IvQueryError::RetentionMiss
    })
}

fn record_query_rejection_locked(
    state: &mut IvQueryState,
    provenance: &IvProvenance,
    reject_reason: IvRejectReason,
) {
    record_source_rejection_locked(
        state,
        &provenance.profile_id,
        &provenance.source_id,
        provenance.subscription_generation,
        provenance.ts_event_ns,
        reject_reason,
    );
}

fn record_source_rejection_locked(
    state: &mut IvQueryState,
    profile_id: &str,
    source_id: &str,
    subscription_generation: u64,
    ts_event_ns: UnixNanos,
    reject_reason: IvRejectReason,
) {
    if let Some(existing) = state.source_health.iter_mut().find(|existing| {
        existing.profile_id == profile_id
            && existing.source_id == source_id
            && existing.subscription_generation == subscription_generation
    }) {
        existing.last_event_ts_ns = Some(ts_event_ns);
        existing.last_reject_reason = Some(reject_reason);
        *existing
            .reject_counts
            .entry(reject_reason)
            .or_insert(INITIAL_REJECT_COUNT) += REJECT_COUNT_INCREMENT;
        apply_source_rejection_flags(existing, reject_reason, false);
    } else {
        let mut reject_counts = BTreeMap::new();
        reject_counts.insert(reject_reason, REJECT_COUNT_INCREMENT);
        state.source_health.push(IvSourceHealth {
            profile_id: profile_id.to_string(),
            source_id: source_id.to_string(),
            subscription_state: rejection_health_state(reject_reason),
            last_event_ts_ns: Some(ts_event_ns),
            last_reject_reason: Some(reject_reason),
            reject_counts,
            stale_state: reject_reason == IvRejectReason::StaleData,
            retention_state: reject_reason == IvRejectReason::RetentionMiss,
            subscription_generation,
        });
    }
    push_query_rejection_decision_locked(state, reject_reason, subscription_generation);
}

fn push_query_rejection_decision_locked(
    state: &mut IvQueryState,
    reject_reason: IvRejectReason,
    subscription_generation: u64,
) {
    state
        .query_rejections
        .push(IvPolicyDecision::RejectionDecision {
            reject_reason,
            failed_field: None,
            policy_id: None,
            source_health_state: rejection_health_state(reject_reason),
            subscription_generation,
        });
}

fn apply_source_rejection_flags(
    health: &mut IvSourceHealth,
    reject_reason: IvRejectReason,
    mark_rejected: bool,
) {
    if mark_rejected {
        transition_source_health_state(health, IvSourceHealthState::Rejected);
    } else if reject_reason == IvRejectReason::StaleData {
        transition_source_health_state(health, IvSourceHealthState::Stale);
    }
    health.stale_state |= reject_reason == IvRejectReason::StaleData;
    health.retention_state |= reject_reason == IvRejectReason::RetentionMiss;
}

fn transition_source_health_state(health: &mut IvSourceHealth, next: IvSourceHealthState) {
    if health.subscription_state.can_transition_to(next) {
        health.subscription_state = next;
    }
}

fn rejection_health_state(reject_reason: IvRejectReason) -> IvSourceHealthState {
    if reject_reason == IvRejectReason::StaleData {
        IvSourceHealthState::Stale
    } else {
        IvSourceHealthState::Active
    }
}

fn active_slice<T>(values: &[T], start: usize) -> &[T] {
    &values[start.min(values.len())..]
}

fn retain_with_logical_start<T>(values: &mut Vec<T>, start: usize, max_len: usize) -> usize {
    let mut retained_start = start.min(values.len());
    let active_len = values.len().saturating_sub(retained_start);
    if active_len > max_len {
        retained_start += active_len - max_len;
    }
    if retained_start > 0 && (max_len == 0 || retained_start > max_len) {
        values.drain(..retained_start);
        retained_start = EMPTY_RETENTION_START;
    }
    retained_start
}

fn select_source_health<'a>(
    state: &'a IvQueryState,
    profile_id: &str,
    source_id: &str,
) -> Option<&'a IvSourceHealth> {
    if let Some(current_generation) = state.current_subscription_generations.get(source_id) {
        return state.source_health.iter().find(|health| {
            health.profile_id == profile_id
                && health.source_id == source_id
                && health.subscription_generation == *current_generation
        });
    }

    state
        .source_health
        .iter()
        .filter(|health| health.profile_id == profile_id && health.source_id == source_id)
        .max_by_key(|health| health.subscription_generation)
}

fn merge_source_health_update(existing: &mut IvSourceHealth, mut incoming: IvSourceHealth) {
    let successful_active_event = incoming.subscription_state == IvSourceHealthState::Active
        && incoming.last_event_ts_ns.is_some()
        && !incoming.stale_state;
    if incoming.last_event_ts_ns.is_none() {
        incoming.last_event_ts_ns = existing.last_event_ts_ns;
    }
    if incoming.last_reject_reason.is_none() && !successful_active_event {
        incoming.last_reject_reason = existing.last_reject_reason;
    }
    for (reason, count) in &existing.reject_counts {
        *incoming
            .reject_counts
            .entry(*reason)
            .or_insert(INITIAL_REJECT_COUNT) += count;
    }
    if !successful_active_event {
        incoming.stale_state |= existing.stale_state;
    }
    incoming.retention_state |= existing.retention_state;
    if !existing
        .subscription_state
        .can_transition_to(incoming.subscription_state)
    {
        incoming.subscription_state = existing.subscription_state;
    }
    *existing = incoming;
}

fn retain_source_health_events(
    source_health: &mut Vec<IvSourceHealth>,
    current_subscription_generations: &BTreeMap<String, u64>,
    max_events: usize,
) {
    if source_health.len() <= max_events {
        return;
    }

    let mut current = Vec::new();
    let mut historical = Vec::new();
    for health in source_health.iter().cloned() {
        if current_subscription_generations
            .get(&health.source_id)
            .is_some_and(|generation| *generation == health.subscription_generation)
        {
            current.push(health);
        } else {
            historical.push(health);
        }
    }

    if max_events == 0 || current.len() >= max_events {
        *source_health = current;
        return;
    }

    let historical_limit = max_events - current.len();
    if historical.len() > historical_limit {
        let retained_start = historical.len() - historical_limit;
        historical.drain(..retained_start);
    }
    historical.extend(current);
    *source_health = historical;
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        panic::{AssertUnwindSafe, catch_unwind},
    };

    use crate::bolt_v3_iv::authz::IvAuthorizationMode;
    use crate::bolt_v3_iv::bounds::{IvBoundUnit, IvConventionBounds};
    use crate::bolt_v3_iv::health::IvSourceHealthState;
    use crate::bolt_v3_iv::policy::{
        IvBasisSelection, IvEvidenceMapping, IvProjectionKind, IvTenorSelection,
    };

    use super::*;

    fn test_projection_bounds() -> IvNumericBounds {
        IvNumericBounds {
            finite_required: true,
            positive_required: true,
            inclusive_min: Some(0.0),
            inclusive_max: Some(1.0),
            exclusive_min: None,
            exclusive_max: None,
            unit: IvBoundUnit::Unitless,
            allowed_conventions: IvConventionBounds {
                allowed_conventions: [
                    IvConvention::Named("configured-convention".to_string()),
                    IvConvention::Named("test_convention".to_string()),
                ]
                .into_iter()
                .collect(),
            },
        }
    }

    #[test]
    #[should_panic(expected = "IV query state lock poisoned")]
    fn query_state_handle_read_panics_on_poisoned_lock() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        let inner = handle.inner.clone();
        let poison_result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = inner.write().unwrap();
            panic!("poison query state lock");
        }));
        assert!(poison_result.is_err());
        assert!(inner.read().is_err());

        handle.snapshot();
    }

    #[test]
    #[should_panic(expected = "IV query state lock poisoned")]
    fn query_state_handle_write_panics_on_poisoned_lock() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        let inner = handle.inner.clone();
        let poison_result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = inner.write().unwrap();
            panic!("poison query state lock");
        }));
        assert!(poison_result.is_err());
        assert!(inner.write().is_err());

        handle.replace_source_health(Vec::new());
    }

    #[test]
    fn retained_product_keys_iterate_by_event_time_for_btree_retention() {
        let keys = BTreeSet::from([
            retained_key("test-profile-a", "test-source-a", 1, 30),
            retained_key("test-profile-z", "test-source-z", 1, 10),
            retained_key("test-profile-m", "test-source-m", 1, 20),
        ]);

        let timestamps = keys.iter().map(|key| key.ts_event_ns).collect::<Vec<_>>();

        assert_eq!(
            timestamps,
            vec![UnixNanos::new(10), UnixNanos::new(20), UnixNanos::new(30)]
        );
    }

    #[test]
    fn query_retention_does_not_front_move_retained_rejections() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        let dropped = rejection_decision(IvRejectReason::SelectorNotAuthorized, 1);
        let retained = rejection_decision(IvRejectReason::InvalidIvValue, 2);
        let retained_before = {
            let mut state = handle.write_state();
            state.query_rejections.push(dropped);
            state.query_rejections.push(retained.clone());
            state
                .query_rejections
                .iter()
                .find(|decision| **decision == retained)
                .map(std::ptr::from_ref)
                .expect("retained rejection should be recorded before retention")
        };

        handle.enforce_retention(&IvRetentionPolicy {
            max_raw_events: 1,
            max_indexed_points: 1,
            max_smiles: 1,
            max_surfaces: 1,
            max_derived_points: 1,
            max_source_health_events: 1,
        });

        let retained_after = {
            let state = handle.read_state();
            state
                .query_rejections
                .iter()
                .find(|decision| **decision == retained)
                .map(std::ptr::from_ref)
                .expect("retained rejection should remain after retention")
        };
        assert_eq!(handle.query_rejections(), vec![retained]);
        assert_eq!(retained_before, retained_after);
    }

    #[test]
    fn query_side_effect_rejections_enforce_profile_retention_bounds() {
        let handle = IvQueryHandle::new(
            "test_profile",
            IvSelectorAuthorization {
                authorization_mode: IvAuthorizationMode::ProfileWide,
                strategy_id: "test_strategy".to_string(),
                allowed_product_kinds: BTreeSet::new(),
                allowed_selector_fingerprints: BTreeSet::new(),
                allowed_source_ids: BTreeSet::new(),
            },
            IvStore::empty(),
        )
        .with_retention_policy(IvRetentionPolicy {
            max_raw_events: 1,
            max_indexed_points: 1,
            max_smiles: 1,
            max_surfaces: 1,
            max_derived_points: 1,
            max_source_health_events: 1,
        });

        for subscription_generation in 1..=3 {
            let mut provenance = test_point("test_source", subscription_generation).provenance;
            provenance.subscription_generation = subscription_generation;
            let mut side_effects = IvQuerySideEffects::new();
            side_effects.record_query_rejection(&provenance, IvRejectReason::StaleData);
            side_effects.apply(&handle);
        }

        assert_eq!(
            handle.query_rejections(),
            vec![rejection_decision(IvRejectReason::StaleData, 3)]
        );
    }

    #[test]
    fn fallback_policy_rejects_inputs_exceeding_maximum_timestamp_skew() {
        let projection_policy = IvProjectionPolicy {
            policy_id: "test_projection_policy".to_string(),
            projection_kind: IvProjectionKind::Mean,
            basis_selection: IvBasisSelection::PreserveInputBasis,
            source_eligibility: vec!["test_source_a".to_string(), "test_source_b".to_string()],
            strike_selection: IvStrikeSelection::AllConfiguredStrikes,
            tenor_selection: IvTenorSelection::AllConfiguredTenors,
            evidence_mapping: IvEvidenceMapping::PreserveEvidenceKind,
            minimum_points: 3,
            max_projection_input_skew_ns: 100,
            output_bounds: test_projection_bounds(),
            fallback_policy_ref: Some("test_fallback_policy".to_string()),
            interpolation_policy_ref: None,
            quorum_policy_ref: None,
        };
        let state =
            IvQueryState::new(IvStore::empty()).with_fallback_policies(vec![IvFallbackPolicy {
                policy_id: "test_fallback_policy".to_string(),
                candidate_order: vec!["test_instrument".to_string()],
                eligible_sources: Vec::new(),
                maximum_timestamp_skew_ns: 5,
                required_provenance_fields: Vec::new(),
            }]);
        let products = vec![
            IvQueryProduct::IvPoint(test_point("test_source_a", 100)),
            IvQueryProduct::IvPoint(test_point("test_source_b", 120)),
        ];
        let inputs = projection_inputs_from_products(&products).unwrap();

        assert!(matches!(
            fallback_only(&projection_policy, &state, &products, &inputs),
            Err(IvQueryError::ProjectionRejected)
        ));
    }

    fn retained_key(
        profile_id: &str,
        source_id: &str,
        subscription_generation: u64,
        ts_event_ns: u64,
    ) -> IvRetainedProductKey {
        IvRetainedProductKey {
            profile_id: profile_id.to_string(),
            source_id: source_id.to_string(),
            selector_fingerprint: "test_selector".to_string(),
            subscription_generation,
            instrument_id: "test_instrument".to_string(),
            surface_selector: "test_surface_selector".to_string(),
            basis: IvBasis::Mark,
            ts_event_ns: UnixNanos::new(ts_event_ns),
            product_kind: IvProductKind::IvPoint,
            product_key: "test_instrument".to_string(),
        }
    }

    fn rejection_decision(
        reject_reason: IvRejectReason,
        subscription_generation: u64,
    ) -> IvPolicyDecision {
        IvPolicyDecision::RejectionDecision {
            reject_reason,
            failed_field: None,
            policy_id: None,
            source_health_state: rejection_health_state(reject_reason),
            subscription_generation,
        }
    }

    fn test_point(source_id: &str, ts_event_ns: u64) -> IvPoint {
        let timestamp = UnixNanos::new(ts_event_ns);
        IvPoint {
            profile_id: "test_profile".to_string(),
            source_id: source_id.to_string(),
            instrument_id: "test_instrument".to_string(),
            basis: IvBasis::Mark,
            iv: 0.42,
            convention: IvConvention::Named("test_convention".to_string()),
            ts_event_ns: timestamp,
            ts_init_ns: Some(timestamp),
            provenance: IvProvenance {
                profile_id: "test_profile".to_string(),
                source_id: source_id.to_string(),
                source_kind: crate::bolt_v3_iv::types::IvSourceKind::OptionGreeks,
                selector_fingerprint: format!("test_selector_{source_id}"),
                nt_revision: crate::bolt_v3_iv::runtime::cargo_pinned_nt_revision().to_string(),
                nt_evidence_path: "test/evidence.rs".to_string(),
                nt_symbol: "TestNtSymbol".to_string(),
                raw_event_id: Some(format!("test_raw_event_{source_id}")),
                payload_kind: Some("option_greeks".to_string()),
                input_event_ids: Vec::new(),
                helper_identity: None,
                policy_decisions: Vec::new(),
                transformation_steps: Vec::new(),
                ts_event_ns: timestamp,
                ts_init_ns: Some(timestamp),
                received_ts_ns: timestamp,
                ingest_sequence: 1,
                subscription_generation: 1,
                source_health_state: IvSourceHealthState::Active,
                reject_reason: None,
            },
        }
    }

    #[test]
    fn projected_output_provenance_requires_projection_transformation_step() {
        let mut provenance = test_point("test-source", 1).provenance;
        provenance
            .policy_decisions
            .push(IvPolicyDecision::ProjectionDecision {
                policy_id: "test-projection-policy".to_string(),
                input_product_ids: vec!["test-input".to_string()],
                selector_fingerprints: vec!["test-selector".to_string()],
                projection_kind: "mean".to_string(),
                basis: "mark".to_string(),
                convention: "test-convention".to_string(),
                max_projection_input_skew_ns: 1,
                accepted_input_ids: vec!["test-input".to_string()],
                rejected_input_ids: Vec::new(),
            });

        assert_eq!(validate_iv_provenance(&provenance), Ok(()));
        assert_eq!(
            validate_projected_output_provenance(&provenance),
            Err(IvQueryError::ProjectionRejected)
        );

        provenance
            .transformation_steps
            .push("test-projection-policy".to_string());
        assert_eq!(validate_projected_output_provenance(&provenance), Ok(()));
    }

    #[test]
    fn upsert_source_health_preserves_historical_generation_entries() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        handle
            .write_state()
            .current_subscription_generations
            .insert("test-source".to_string(), 1);
        handle.upsert_source_health(source_health_with_generation(
            "test-source",
            IvSourceHealthState::Active,
            1,
        ));
        handle
            .write_state()
            .current_subscription_generations
            .insert("test-source".to_string(), 2);
        handle.upsert_source_health(source_health_with_generation(
            "test-source",
            IvSourceHealthState::Active,
            2,
        ));

        let generations = handle
            .snapshot()
            .source_health
            .iter()
            .filter(|health| health.source_id == "test-source")
            .map(|health| health.subscription_generation)
            .collect::<Vec<_>>();

        assert_eq!(generations, vec![1, 2]);
    }

    #[test]
    fn mark_sources_removed_updates_exact_generation_only() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        handle.upsert_source_health(source_health_with_generation(
            "test-source",
            IvSourceHealthState::Active,
            1,
        ));
        handle.upsert_source_health(source_health_with_generation(
            "test-source",
            IvSourceHealthState::Active,
            2,
        ));
        let removed = BTreeMap::from([("test-source".to_string(), 2)]);

        handle.mark_sources_removed("test-profile", &removed);

        let snapshot = handle.snapshot();
        let generation_one = snapshot
            .source_health
            .iter()
            .find(|health| health.source_id == "test-source" && health.subscription_generation == 1)
            .unwrap();
        let generation_two = snapshot
            .source_health
            .iter()
            .find(|health| health.source_id == "test-source" && health.subscription_generation == 2)
            .unwrap();
        assert_eq!(
            generation_one.subscription_state,
            IvSourceHealthState::Active
        );
        assert_eq!(
            generation_two.subscription_state,
            IvSourceHealthState::Removed
        );
    }

    #[test]
    fn mark_sources_removed_preserves_rejection_history_for_exact_generation() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        handle
            .write_state()
            .current_subscription_generations
            .insert("test-source".to_string(), 2);
        handle.record_source_rejection(
            "test-profile".to_string(),
            "test-source".to_string(),
            2,
            UnixNanos::new(42),
            IvRejectReason::InvalidIvValue,
            false,
        );
        let removed = BTreeMap::from([("test-source".to_string(), 2)]);

        handle.mark_sources_removed("test-profile", &removed);

        let health = handle
            .source_health_for("test-profile", "test-source")
            .unwrap();
        assert_eq!(health.subscription_state, IvSourceHealthState::Removed);
        assert_eq!(
            health.last_reject_reason,
            Some(IvRejectReason::InvalidIvValue)
        );
        assert_eq!(health.last_event_ts_ns, Some(UnixNanos::new(42)));
        assert_eq!(
            health.reject_counts.get(&IvRejectReason::InvalidIvValue),
            Some(&1)
        );
    }

    #[test]
    fn record_source_rejection_does_not_replace_removed_generation_state() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        handle
            .write_state()
            .current_subscription_generations
            .insert("test-source".to_string(), 2);
        handle.upsert_source_health(source_health_with_generation(
            "test-source",
            IvSourceHealthState::Removed,
            2,
        ));

        handle.record_source_rejection(
            "test-profile".to_string(),
            "test-source".to_string(),
            2,
            UnixNanos::new(42),
            IvRejectReason::SourceNotConfigured,
            true,
        );

        let health = handle
            .source_health_for("test-profile", "test-source")
            .unwrap();
        assert_eq!(health.subscription_state, IvSourceHealthState::Removed);
        assert_eq!(
            health.last_reject_reason,
            Some(IvRejectReason::SourceNotConfigured)
        );
        assert_eq!(
            health
                .reject_counts
                .get(&IvRejectReason::SourceNotConfigured),
            Some(&1)
        );
    }

    #[test]
    fn upsert_source_health_does_not_reactivate_removed_generation_state() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        handle
            .write_state()
            .current_subscription_generations
            .insert("test-source".to_string(), 2);
        handle.upsert_source_health(source_health_with_generation(
            "test-source",
            IvSourceHealthState::Removed,
            2,
        ));

        handle.upsert_source_health(source_health_with_generation(
            "test-source",
            IvSourceHealthState::Active,
            2,
        ));

        let health = handle
            .source_health_for("test-profile", "test-source")
            .unwrap();
        assert_eq!(health.subscription_state, IvSourceHealthState::Removed);
    }

    #[test]
    fn record_source_rejection_without_mark_rejected_does_not_reject_missing_health_row() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        handle.record_source_rejection(
            "test-profile".to_string(),
            "test-source".to_string(),
            2,
            UnixNanos::new(42),
            IvRejectReason::InvalidIvValue,
            false,
        );

        let health = handle
            .source_health_for("test-profile", "test-source")
            .unwrap();
        assert_eq!(health.subscription_state, IvSourceHealthState::Active);
        assert_eq!(
            health.last_reject_reason,
            Some(IvRejectReason::InvalidIvValue)
        );
        assert_eq!(health.last_event_ts_ns, Some(UnixNanos::new(42)));
        assert_eq!(
            health.reject_counts.get(&IvRejectReason::InvalidIvValue),
            Some(&1)
        );
    }

    #[test]
    fn upsert_source_health_preserves_rejection_history_for_same_generation() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        handle
            .write_state()
            .current_subscription_generations
            .insert("test-source".to_string(), 3);
        handle.record_source_rejection(
            "test-profile".to_string(),
            "test-source".to_string(),
            3,
            UnixNanos::new(42),
            IvRejectReason::InvalidIvValue,
            false,
        );

        handle.upsert_source_health(source_health_with_generation(
            "test-source",
            IvSourceHealthState::Active,
            3,
        ));

        let health = handle
            .source_health_for("test-profile", "test-source")
            .unwrap();
        assert_eq!(
            health.last_reject_reason,
            Some(IvRejectReason::InvalidIvValue)
        );
        assert_eq!(health.last_event_ts_ns, Some(UnixNanos::new(42)));
        assert_eq!(
            health.reject_counts.get(&IvRejectReason::InvalidIvValue),
            Some(&1)
        );
        assert_eq!(health.subscription_state, IvSourceHealthState::Active);
    }

    fn source_health_with_generation(
        source_id: &str,
        state: IvSourceHealthState,
        subscription_generation: u64,
    ) -> IvSourceHealth {
        IvSourceHealth {
            profile_id: "test-profile".to_string(),
            source_id: source_id.to_string(),
            subscription_state: state,
            last_event_ts_ns: None,
            last_reject_reason: None,
            reject_counts: BTreeMap::new(),
            stale_state: false,
            retention_state: false,
            subscription_generation,
        }
    }
}
