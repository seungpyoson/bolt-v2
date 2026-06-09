use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use serde::{Deserialize, Serialize};

use super::{
    authz::IvSelectorAuthorization,
    derive::{
        IvDeriveError, IvDerivedInputPolicy, IvDerivedInputSet, IvDerivedOutput, IvHelperPolicy,
        derive_iv, resolve_derived_input_policy, select_helper_policy,
    },
    error::IvRejectReason,
    health::IvSourceHealth,
    ingest::{IvIngestEvent, IvRawEvent},
    policy::{
        IvFallbackCandidate, IvFallbackPolicy, IvInterpolationPolicy, IvPolicyInput,
        IvPolicyOutput, IvProjectionPolicy, IvQuorumPolicy, IvStrikeSelection, interpolate_smile,
        project_scalar, resolve_fallback, resolve_quorum,
    },
    provenance::{IvPolicyDecision, IvProvenance},
    selector::IvSelector,
    store::{
        IvAggregateGreeks, IvEvidence, IvGreeksPoint, IvPoint, IvRetentionPolicy, IvSmile, IvStore,
        IvStoreError, IvSurface,
    },
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvProductKind},
};

const INITIAL_REJECT_COUNT: u64 = 0;
const REJECT_COUNT_INCREMENT: u64 = 1;

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
    SourceHealth(IvSourceHealth),
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
    projection_policies: Vec<IvProjectionPolicy>,
    interpolation_policies: Vec<IvInterpolationPolicy>,
    fallback_policies: Vec<IvFallbackPolicy>,
    quorum_policies: Vec<IvQuorumPolicy>,
    helper_policies: Vec<IvHelperPolicy>,
    derived_input_policies: Vec<IvDerivedInputPolicy>,
    derived_inputs: Vec<IvDerivedInputSet>,
    derived_outputs: Vec<IvDerivedOutput>,
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

impl IvQueryState {
    pub fn new(store: IvStore) -> Self {
        Self {
            store,
            source_health: Vec::new(),
            projection_policies: Vec::new(),
            interpolation_policies: Vec::new(),
            fallback_policies: Vec::new(),
            quorum_policies: Vec::new(),
            helper_policies: Vec::new(),
            derived_input_policies: Vec::new(),
            derived_inputs: Vec::new(),
            derived_outputs: Vec::new(),
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
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_state(&self) -> RwLockWriteGuard<'_, IvQueryState> {
        self.inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
            if mark_rejected {
                existing.subscription_state = super::health::IvSourceHealthState::Rejected;
            }
            return;
        }

        let mut reject_counts = BTreeMap::new();
        reject_counts.insert(reject_reason, REJECT_COUNT_INCREMENT);
        let subscription_state = if mark_rejected {
            super::health::IvSourceHealthState::Rejected
        } else {
            super::health::IvSourceHealthState::Active
        };
        state.source_health.push(IvSourceHealth {
            profile_id,
            source_id,
            subscription_state,
            last_event_ts_ns: Some(last_event_ts_ns),
            last_reject_reason: Some(reject_reason),
            reject_counts,
            stale_state: false,
            retention_state: false,
            subscription_generation,
        });
    }

    pub fn source_health_for(&self, profile_id: &str, source_id: &str) -> Option<IvSourceHealth> {
        let state = self.read_state();
        select_source_health(&state, profile_id, source_id).cloned()
    }

    pub fn enforce_retention(&self, policy: &IvRetentionPolicy) {
        let mut state = self.write_state();
        state.store.enforce_retention(policy);
        truncate_front(&mut state.derived_outputs, policy.max_derived_points);
        let current_subscription_generations = state.current_subscription_generations.clone();
        retain_source_health_events(
            &mut state.source_health,
            &current_subscription_generations,
            policy.max_source_health_events,
        );
    }

    pub fn derived_outputs(&self) -> Vec<IvDerivedOutput> {
        self.read_state().derived_outputs.clone()
    }

    pub fn record_derived_output(&self, output: IvDerivedOutput) {
        let mut state = self.write_state();
        if let Some(existing) = state
            .derived_outputs
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
                existing.subscription_state = super::health::IvSourceHealthState::Removed;
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

    pub fn state_handle(&self) -> IvQueryStateHandle {
        self.state.clone()
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
        if !selector_supports_product_kind(&query.selector, query.product_kind) {
            return Err(IvQueryError::ProductKindMismatch);
        }

        let product = if query_requires_snapshot(query) {
            let state = self.state.snapshot();
            self.query_product_from_state(query, &state)?
        } else {
            let state = self.state.read_state();
            self.query_product_from_state(query, &state)?
        };

        if let IvQueryProduct::DerivedIv(derived) = &product
            && should_cache_derived_output(query)
        {
            self.state.record_derived_output((**derived).clone());
            self.enforce_retention_policy();
        }

        Ok(product)
    }

    fn query_product_from_state(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
    ) -> Result<IvQueryProduct, IvQueryError> {
        let mut product = self.find_product(query, state)?;
        if !product_satisfies_current_state(&product, state) {
            return Err(IvQueryError::ProductNotFound);
        }
        if !self.authorization.authorizes(
            &query.strategy_id,
            query.product_kind,
            product.source_id(),
            product.selector_fingerprint(),
        ) {
            product = self
                .find_authorized_current_product(query, state)?
                .ok_or(IvQueryError::StrategyNotAuthorized)?;
        }

        Ok(product)
    }

    fn find_authorized_current_product(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
    ) -> Result<Option<IvQueryProduct>, IvQueryError> {
        let products = match (&query.product_kind, &query.selector) {
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
            ),
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
            ),
            _ => return Ok(None),
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
                .ok_or(IvQueryError::ProductNotFound),
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
                .ok_or(IvQueryError::ProductNotFound),
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
            .ok_or(IvQueryError::ProductNotFound),
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
            .ok_or(IvQueryError::ProductNotFound),
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
                .ok_or(IvQueryError::ProductNotFound),
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
                .ok_or(IvQueryError::ProductNotFound),
            (
                IvProductKind::SourceHealth,
                IvSelector::SourceHealthQuery {
                    source_filter,
                    state_filter,
                },
            ) => {
                let health = if let Some(source_id) = source_filter {
                    select_source_health(state, &query.profile_id, source_id)
                        .filter(|health| source_health_state_matches(health, state_filter))
                } else {
                    state.source_health.iter().find(|health| {
                        health.profile_id == query.profile_id
                            && source_health_state_matches(health, state_filter)
                    })
                };
                health
                    .cloned()
                    .map(IvQueryProduct::SourceHealth)
                    .ok_or(IvQueryError::ProductNotFound)
            }
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
                instrument_id,
                helper_policy_id,
                *as_of_ns,
                inputs.as_deref(),
            ),
            _ => Err(IvQueryError::ProductKindMismatch),
        }
    }

    fn project_scalar_query(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
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
        let input_products = self.find_projection_products(&input_query, state)?;
        if !input_products
            .iter()
            .all(|product| product_satisfies_current_state(product, state))
        {
            return Err(IvQueryError::ProductNotFound);
        }
        let mut inputs = projection_inputs_from_products(&input_products)?;
        if !projection_inputs_authorized(
            &self.authorization,
            &query.strategy_id,
            query.product_kind,
            &inputs,
        ) {
            return Err(IvQueryError::StrategyNotAuthorized);
        }
        let mut policy_decisions = Vec::new();

        if let Some(interpolation_policy_ref) = &policy.interpolation_policy_ref
            && let Some(interpolated) = interpolate_projected_inputs(
                policy,
                state,
                interpolation_policy_ref,
                &input_products,
            )?
        {
            inputs = interpolated.inputs;
            policy_decisions.extend(interpolated.policy_decisions);
        }

        if let Some(quorum_policy_ref) = &policy.quorum_policy_ref {
            let quorum_policy = state
                .quorum_policies
                .iter()
                .find(|policy| policy.policy_id == *quorum_policy_ref)
                .ok_or(IvQueryError::ProjectionRejected)?;
            let quorum_output = resolve_quorum(quorum_policy, &inputs)
                .map_err(|_| IvQueryError::ProjectionRejected)?;
            policy_decisions.extend(quorum_output.policy_decisions);
        }

        let output = project_or_fallback(policy, state, &inputs)?;
        let mut provenance = input_products
            .first()
            .ok_or(IvQueryError::ProductNotFound)?
            .provenance()
            .cloned()
            .ok_or(IvQueryError::UnsupportedProductKind)?;
        provenance.policy_decisions.extend(policy_decisions);
        provenance.policy_decisions.extend(output.policy_decisions);
        provenance.ts_event_ns = as_of_ns;

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
                            && point.ts_event_ns == *as_of_ns
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
                            && point.point.ts_event_ns == *as_of_ns
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
                let products = matching_smile_products(
                    state,
                    &query.profile_id,
                    series_id,
                    side,
                    *basis,
                    *as_of_ns,
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
                let products = matching_surface_products(
                    state,
                    &query.profile_id,
                    series_selectors,
                    *basis,
                    *as_of_ns,
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
                            && evidence.ts_event_ns == *as_of_ns
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
                            && aggregate.ts_event_ns == *as_of_ns
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
                state,
                instrument_id,
                helper_policy_id,
                *as_of_ns,
                inputs.as_deref(),
            ),
            _ => self.find_product(query, state).map(|product| vec![product]),
        }
    }

    fn find_derived_projection_products(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
        instrument_id: &str,
        helper_policy_id: &str,
        as_of_ns: UnixNanos,
        request_inputs: Option<&IvDerivedInputSet>,
    ) -> Result<Vec<IvQueryProduct>, IvQueryError> {
        if request_inputs.is_some() {
            return self.find_product(query, state).map(|product| vec![product]);
        }

        let mut outputs = state
            .derived_outputs
            .iter()
            .filter(|derived| {
                derived_output_matches_query(
                    derived,
                    &query.profile_id,
                    instrument_id,
                    helper_policy_id,
                    as_of_ns,
                )
            })
            .cloned()
            .collect::<Vec<_>>();

        let mut derived_any = false;
        let mut first_derivation_error = None;
        for inputs in state.derived_inputs.iter().filter(|inputs| {
            inputs.profile_id == query.profile_id
                && inputs.instrument_id == instrument_id
                && inputs.as_of_ns == as_of_ns
        }) {
            if outputs
                .iter()
                .any(|output| derived_output_matches_input(output, inputs, helper_policy_id))
            {
                continue;
            }
            let output = match self.derive_iv_from_inputs(state, helper_policy_id, inputs.clone()) {
                Ok(output) => output,
                Err(IvQueryError::DerivationRejected) => {
                    first_derivation_error.get_or_insert(IvQueryError::DerivationRejected);
                    continue;
                }
                Err(error) => return Err(error),
            };
            self.state.record_derived_output(output.clone());
            outputs.push(output);
            derived_any = true;
        }

        if derived_any {
            self.enforce_retention_policy();
        }

        if outputs.is_empty() {
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
        instrument_id: &str,
        helper_policy_id: &str,
        as_of_ns: UnixNanos,
        request_inputs: Option<&IvDerivedInputSet>,
    ) -> Result<IvQueryProduct, IvQueryError> {
        let inputs = if let Some(inputs) = request_inputs {
            if inputs.profile_id != query.profile_id
                || inputs.instrument_id != instrument_id
                || inputs.as_of_ns != as_of_ns
            {
                return Err(IvQueryError::DerivedInputNotFound);
            }
            inputs.clone()
        } else {
            state
                .derived_inputs
                .iter()
                .find(|inputs| {
                    inputs.profile_id == query.profile_id
                        && inputs.instrument_id == instrument_id
                        && inputs.as_of_ns == as_of_ns
                })
                .cloned()
                .ok_or(IvQueryError::DerivedInputNotFound)?
        };
        self.derive_iv_from_inputs(state, helper_policy_id, inputs)
            .map(|output| IvQueryProduct::DerivedIv(Box::new(output)))
    }

    fn derive_iv_from_inputs(
        &self,
        state: &IvQueryState,
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
            self.record_derived_rejection(&inputs, IvRejectReason::HelperNotConfigured);
            return Err(IvQueryError::DerivationRejected);
        };
        if !derived_input_satisfies_current_state(&inputs, state) {
            self.record_derived_rejection(&inputs, IvRejectReason::StaleData);
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
                self.record_derived_rejection(&inputs, derive_reject_reason(&error));
                return Err(IvQueryError::DerivationRejected);
            }
        };
        match derive_iv(policy, inputs.clone()) {
            Ok(output) => Ok(output),
            Err(error) => {
                self.record_derived_rejection(&inputs, derive_reject_reason(&error));
                Err(IvQueryError::DerivationRejected)
            }
        }
    }

    fn record_derived_rejection(&self, inputs: &IvDerivedInputSet, reject_reason: IvRejectReason) {
        self.state.record_source_rejection(
            inputs.profile_id.clone(),
            inputs.source_id.clone(),
            inputs.subscription_generation,
            inputs.as_of_ns,
            reject_reason,
            false,
        );
        self.enforce_retention_policy();
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
    state
        .store
        .smiles()
        .iter()
        .filter(|smile| {
            smile.profile_id == profile_id
                && smile.series_id == series_id
                && side.as_ref().is_none_or(|side| smile.side == *side)
                && smile.basis == basis
                && smile.ts_event_ns == as_of_ns
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
    let mut seen = BTreeSet::new();
    state
        .store
        .smiles()
        .iter()
        .filter(|smile| {
            smile.profile_id == profile_id
                && series_selectors.contains(&smile.surface_selector)
                && smile.basis == basis
                && smile.ts_event_ns == as_of_ns
        })
        .filter_map(|smile| {
            let key = (smile.surface_selector.clone(), smile.source_id.clone());
            if !seen.insert(key) {
                return None;
            }
            state
                .store
                .surface(&smile.surface_selector, &smile.source_id, basis, as_of_ns)
                .map(IvQueryProduct::Surface)
        })
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
        IvQueryProduct::ProjectedScalarIv(_) | IvQueryProduct::SourceHealth(_) => {
            return Err(IvQueryError::UnsupportedProductKind);
        }
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

fn query_requires_snapshot(query: &IvProductQuery) -> bool {
    selector_requires_query_time_writes(&query.selector)
}

fn selector_requires_query_time_writes(selector: &IvSelector) -> bool {
    match selector {
        IvSelector::DerivedIvQuery { .. } => true,
        IvSelector::ProjectedScalarIvQuery { input_selector, .. } => {
            selector_requires_query_time_writes(input_selector)
        }
        _ => false,
    }
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

fn project_or_fallback(
    policy: &IvProjectionPolicy,
    state: &IvQueryState,
    inputs: &[IvPolicyInput],
) -> Result<IvPolicyOutput, IvQueryError> {
    match project_scalar(policy, inputs) {
        Ok(output) => Ok(output),
        Err(_) => {
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
            resolve_fallback(fallback_policy, &candidates)
                .map_err(|_| IvQueryError::ProjectionRejected)
        }
    }
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
) -> Result<Option<InterpolatedProjectionInputs>, IvQueryError> {
    if projection_policy.strike_selection == IvStrikeSelection::AllConfiguredStrikes {
        return Ok(None);
    }

    let policy = state
        .interpolation_policies
        .iter()
        .find(|policy| policy.policy_id == interpolation_policy_id)
        .ok_or(IvQueryError::ProjectionRejected)?;

    let mut inputs = Vec::new();
    let mut policy_decisions = Vec::new();
    for product in input_products {
        match product {
            IvQueryProduct::Smile(smile) => {
                interpolate_smile_input(
                    projection_policy,
                    policy,
                    smile,
                    &mut inputs,
                    &mut policy_decisions,
                );
            }
            IvQueryProduct::Surface(surface) => {
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
            _ => return Ok(None),
        }
    }

    if inputs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(InterpolatedProjectionInputs {
            inputs,
            policy_decisions,
        }))
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
    if matches!(product, IvQueryProduct::SourceHealth(_)) {
        return true;
    }

    let Some(provenance) = product.provenance() else {
        return false;
    };
    if !provenance.source_health_state.can_satisfy_current_query() {
        return false;
    }
    if !state.current_subscription_generations.is_empty() {
        let Some(current_generation) = state
            .current_subscription_generations
            .get(&provenance.source_id)
        else {
            return false;
        };
        if *current_generation != provenance.subscription_generation {
            return false;
        }
    }
    if let Some(current_health) =
        select_source_health(state, &provenance.profile_id, &provenance.source_id)
    {
        return current_health.can_satisfy_current_query()
            && current_health.subscription_generation == provenance.subscription_generation;
    }
    true
}

fn derived_input_satisfies_current_state(inputs: &IvDerivedInputSet, state: &IvQueryState) -> bool {
    if !inputs.source_health_state.can_satisfy_current_query() {
        return false;
    }
    if !state.current_subscription_generations.is_empty() {
        let Some(current_generation) = state
            .current_subscription_generations
            .get(&inputs.source_id)
        else {
            return false;
        };
        if *current_generation != inputs.subscription_generation {
            return false;
        }
    }
    if let Some(current_health) = select_source_health(state, &inputs.profile_id, &inputs.source_id)
    {
        return current_health.can_satisfy_current_query()
            && current_health.subscription_generation == inputs.subscription_generation;
    }
    true
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

fn truncate_front<T>(values: &mut Vec<T>, max_len: usize) {
    if values.len() > max_len {
        let retained_start = values.len() - max_len;
        values.drain(..retained_start);
    }
}

fn select_source_health<'a>(
    state: &'a IvQueryState,
    profile_id: &str,
    source_id: &str,
) -> Option<&'a IvSourceHealth> {
    if let Some(current_generation) = state.current_subscription_generations.get(source_id)
        && let Some(health) = state.source_health.iter().find(|health| {
            health.profile_id == profile_id
                && health.source_id == source_id
                && health.subscription_generation == *current_generation
        })
    {
        return Some(health);
    }

    state
        .source_health
        .iter()
        .filter(|health| health.profile_id == profile_id && health.source_id == source_id)
        .max_by_key(|health| health.subscription_generation)
}

fn merge_source_health_update(existing: &mut IvSourceHealth, mut incoming: IvSourceHealth) {
    if incoming.last_event_ts_ns.is_none() {
        incoming.last_event_ts_ns = existing.last_event_ts_ns;
    }
    if incoming.last_reject_reason.is_none() {
        incoming.last_reject_reason = existing.last_reject_reason;
    }
    for (reason, count) in &existing.reject_counts {
        *incoming
            .reject_counts
            .entry(*reason)
            .or_insert(INITIAL_REJECT_COUNT) += count;
    }
    incoming.stale_state |= existing.stale_state;
    incoming.retention_state |= existing.retention_state;
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
    if max_events == 0 {
        source_health.clear();
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

    if current.len() >= max_events {
        let retained_start = current.len() - max_events;
        *source_health = current.split_off(retained_start);
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
        collections::BTreeMap,
        panic::{AssertUnwindSafe, catch_unwind},
    };

    use crate::bolt_v3_iv::health::IvSourceHealthState;

    use super::*;

    #[test]
    fn query_state_handle_recovers_from_poisoned_lock() {
        let handle = IvQueryStateHandle::new(IvQueryState::new(IvStore::empty()));
        let inner = handle.inner.clone();
        let poison_result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = inner.write().unwrap();
            panic!("poison query state lock");
        }));
        assert!(poison_result.is_err());

        let recovered = catch_unwind(AssertUnwindSafe(|| handle.snapshot()));

        assert!(recovered.is_ok());
        assert_eq!(recovered.unwrap().store.raw_events().len(), 0);
    }

    #[test]
    fn only_derived_queries_require_snapshot_for_query_time_writes() {
        let point_query = IvProductQuery {
            strategy_id: "test-strategy".to_string(),
            profile_id: "test-profile".to_string(),
            product_kind: IvProductKind::IvPoint,
            selector: IvSelector::PointQuery {
                instrument_ids: vec!["test-instrument".to_string()],
                basis: IvBasis::Mark,
                as_of_ns: UnixNanos::new(1),
                source_filter: None,
            },
        };
        let derived_query = IvProductQuery {
            strategy_id: "test-strategy".to_string(),
            profile_id: "test-profile".to_string(),
            product_kind: IvProductKind::DerivedIv,
            selector: IvSelector::DerivedIvQuery {
                instrument_id: "test-instrument".to_string(),
                helper_policy_id: "test-helper-policy".to_string(),
                as_of_ns: UnixNanos::new(1),
                inputs: None,
            },
        };
        let projected_from_derived_query = IvProductQuery {
            strategy_id: "test-strategy".to_string(),
            profile_id: "test-profile".to_string(),
            product_kind: IvProductKind::ProjectedScalarIv,
            selector: IvSelector::ProjectedScalarIvQuery {
                input_selector: Box::new(derived_query.selector.clone()),
                projection_policy_id: "test-projection-policy".to_string(),
                as_of_ns: UnixNanos::new(1),
            },
        };

        assert!(!query_requires_snapshot(&point_query));
        assert!(query_requires_snapshot(&derived_query));
        assert!(query_requires_snapshot(&projected_from_derived_query));
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
