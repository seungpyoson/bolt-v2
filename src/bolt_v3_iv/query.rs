use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
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
        IvPolicyOutput, IvProjectionPolicy, IvQuorumPolicy, interpolate_smile, project_scalar,
        resolve_fallback, resolve_quorum,
    },
    provenance::IvProvenance,
    selector::IvSelector,
    store::{
        IvAggregateGreeks, IvEvidence, IvGreeksPoint, IvPoint, IvRetentionPolicy, IvSmile, IvStore,
        IvStoreError, IvSurface,
    },
    time::UnixNanos,
    types::{IvConvention, IvProductKind},
};

const INITIAL_REJECT_COUNT: u64 = 0;
const REJECT_COUNT_INCREMENT: u64 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IvQuery {
    Product(IvProductQuery),
    RawPayload(IvRawPayloadQuery),
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

    pub fn snapshot(&self) -> IvQueryState {
        self.inner
            .read()
            .expect("IV query state lock poisoned")
            .clone()
    }

    pub fn ingest_event(&self, event: IvIngestEvent) -> Result<IvRawEvent, IvStoreError> {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .store
            .ingest_event(event)
    }

    pub fn raw_event_count(&self) -> usize {
        self.inner
            .read()
            .expect("IV query state lock poisoned")
            .store
            .raw_events()
            .len()
    }

    pub fn replace_source_health(&self, source_health: Vec<IvSourceHealth>) {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .source_health = source_health;
    }

    pub fn upsert_source_health(&self, source_health: IvSourceHealth) {
        let mut state = self.inner.write().expect("IV query state lock poisoned");
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
        }) {
            if existing.subscription_generation > source_health.subscription_generation {
                return;
            }
            *existing = source_health;
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
        let mut state = self.inner.write().expect("IV query state lock poisoned");
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
        state.source_health.push(IvSourceHealth {
            profile_id,
            source_id,
            subscription_state: super::health::IvSourceHealthState::Rejected,
            last_event_ts_ns: Some(last_event_ts_ns),
            last_reject_reason: Some(reject_reason),
            reject_counts,
            stale_state: false,
            retention_state: false,
            subscription_generation,
        });
    }

    pub fn source_health_for(&self, profile_id: &str, source_id: &str) -> Option<IvSourceHealth> {
        let state = self.inner.read().expect("IV query state lock poisoned");
        select_source_health(&state, profile_id, source_id).cloned()
    }

    pub fn enforce_retention(&self, policy: &IvRetentionPolicy) {
        let mut state = self.inner.write().expect("IV query state lock poisoned");
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
        self.inner
            .read()
            .expect("IV query state lock poisoned")
            .derived_outputs
            .clone()
    }

    pub fn record_derived_output(&self, output: IvDerivedOutput) {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .derived_outputs
            .push(output);
    }

    pub fn set_projection_policies(&self, projection_policies: Vec<IvProjectionPolicy>) {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .projection_policies = projection_policies;
    }

    pub fn set_helper_policies(&self, helper_policies: Vec<IvHelperPolicy>) {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .helper_policies = helper_policies;
    }

    pub fn set_derived_input_policies(&self, derived_input_policies: Vec<IvDerivedInputPolicy>) {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .derived_input_policies = derived_input_policies;
    }

    pub fn set_interpolation_policies(&self, interpolation_policies: Vec<IvInterpolationPolicy>) {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .interpolation_policies = interpolation_policies;
    }

    pub fn set_fallback_policies(&self, fallback_policies: Vec<IvFallbackPolicy>) {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .fallback_policies = fallback_policies;
    }

    pub fn set_quorum_policies(&self, quorum_policies: Vec<IvQuorumPolicy>) {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .quorum_policies = quorum_policies;
    }

    pub fn set_derived_inputs(&self, derived_inputs: Vec<IvDerivedInputSet>) {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .derived_inputs = derived_inputs;
    }

    pub fn set_current_subscription_generations(
        &self,
        current_subscription_generations: BTreeMap<String, u64>,
    ) {
        self.inner
            .write()
            .expect("IV query state lock poisoned")
            .current_subscription_generations = current_subscription_generations;
    }

    pub fn mark_sources_removed(
        &self,
        profile_id: &str,
        source_generations: &BTreeMap<String, u64>,
    ) {
        let mut state = self.inner.write().expect("IV query state lock poisoned");
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
            if let Some(existing) = state
                .source_health
                .iter_mut()
                .find(|health| health.profile_id == profile_id && health.source_id == *source_id)
            {
                if existing.subscription_generation <= *subscription_generation {
                    *existing = removed_health;
                }
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
        }
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

        let state = self.state.snapshot();
        let product = self.find_product(query, &state)?;
        if !product_satisfies_current_state(&product, &state) {
            return Err(IvQueryError::ProductNotFound);
        }
        let source_id = product.source_id();
        let selector_fingerprint = product.selector_fingerprint();
        if !self.authorization.authorizes(
            &query.strategy_id,
            query.product_kind,
            source_id,
            selector_fingerprint,
        ) {
            return Err(IvQueryError::StrategyNotAuthorized);
        }

        if let IvQueryProduct::DerivedIv(derived) = &product {
            self.state.record_derived_output((**derived).clone());
        }

        Ok(product)
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
            ) => state
                .store
                .smiles()
                .iter()
                .find(|smile| {
                    smile.profile_id == query.profile_id
                        && smile.series_id == *series_id
                        && side.as_ref().is_none_or(|side| smile.side == *side)
                        && smile.basis == *basis
                        && smile.ts_event_ns == *as_of_ns
                })
                .cloned()
                .map(IvQueryProduct::Smile)
                .ok_or(IvQueryError::ProductNotFound),
            (
                IvProductKind::Surface,
                IvSelector::SurfaceQuery {
                    series_selectors,
                    basis,
                    as_of_ns,
                },
            ) => state
                .store
                .smiles()
                .iter()
                .find_map(|smile| {
                    if smile.profile_id == query.profile_id
                        && series_selectors.contains(&smile.surface_selector)
                        && smile.basis == *basis
                        && smile.ts_event_ns == *as_of_ns
                    {
                        state.store.surface(
                            &smile.surface_selector,
                            &smile.source_id,
                            *basis,
                            *as_of_ns,
                        )
                    } else {
                        None
                    }
                })
                .map(IvQueryProduct::Surface)
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
            ) => state
                .source_health
                .iter()
                .find(|health| {
                    health.profile_id == query.profile_id
                        && source_matches(&health.source_id, source_filter)
                        && source_health_state_matches(health, state_filter)
                })
                .cloned()
                .map(IvQueryProduct::SourceHealth)
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
                },
            ) => self.derived_iv_query(query, state, instrument_id, helper_policy_id, *as_of_ns),
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
        let input_product = self.find_product(
            &IvProductQuery {
                strategy_id: query.strategy_id.clone(),
                profile_id: query.profile_id.clone(),
                product_kind: input_selector.product_kind(),
                selector: input_selector.clone(),
            },
            state,
        )?;
        let inputs = projection_inputs(&input_product)?;
        let mut policy_decisions = Vec::new();
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

        let output = if let Some(interpolation_policy_ref) = &policy.interpolation_policy_ref {
            if let Some(interpolation_output) =
                interpolate_projected_input(state, interpolation_policy_ref, &input_product)?
            {
                interpolation_output
            } else {
                project_or_fallback(policy, state, &inputs)?
            }
        } else {
            project_or_fallback(policy, state, &inputs)?
        };
        let mut provenance = input_product
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

    fn derived_iv_query(
        &self,
        query: &IvProductQuery,
        state: &IvQueryState,
        instrument_id: &str,
        helper_policy_id: &str,
        as_of_ns: UnixNanos,
    ) -> Result<IvQueryProduct, IvQueryError> {
        let policy = select_helper_policy(&state.helper_policies, helper_policy_id)
            .map_err(|_| IvQueryError::HelperPolicyNotFound)?;
        let inputs = state
            .derived_inputs
            .iter()
            .find(|inputs| {
                inputs.profile_id == query.profile_id
                    && inputs.instrument_id == instrument_id
                    && inputs.as_of_ns == as_of_ns
            })
            .cloned()
            .ok_or(IvQueryError::DerivedInputNotFound)?;
        let inputs = if let Some(input_policy) = state
            .derived_input_policies
            .iter()
            .find(|input_policy| input_policy.helper_policy_ref == helper_policy_id)
        {
            match resolve_derived_input_policy(input_policy, inputs.clone(), &state.derived_inputs)
            {
                Ok(inputs) => inputs,
                Err(error) => {
                    self.record_derived_rejection(&inputs, derive_reject_reason(&error));
                    return Err(IvQueryError::DerivationRejected);
                }
            }
        } else {
            inputs
        };
        match derive_iv(policy, inputs.clone()) {
            Ok(output) => Ok(IvQueryProduct::DerivedIv(Box::new(output))),
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
            true,
        );
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
            Self::SourceHealth(health) => &health.source_id,
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
                convention: smile.provenance.nt_symbol.clone(),
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
                    convention: smile.provenance.nt_symbol.clone(),
                    value: point.iv,
                    ts_event_ns: smile.ts_event_ns,
                })
            })
            .collect(),
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
        IvQueryProduct::AggregateGreeks(_)
        | IvQueryProduct::ProjectedScalarIv(_)
        | IvQueryProduct::SourceHealth(_) => return Err(IvQueryError::UnsupportedProductKind),
    };

    if inputs.is_empty() {
        Err(IvQueryError::ProductNotFound)
    } else {
        Ok(inputs)
    }
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

fn interpolate_projected_input(
    state: &IvQueryState,
    interpolation_policy_id: &str,
    input_product: &IvQueryProduct,
) -> Result<Option<IvPolicyOutput>, IvQueryError> {
    let policy = state
        .interpolation_policies
        .iter()
        .find(|policy| policy.policy_id == interpolation_policy_id)
        .ok_or(IvQueryError::ProjectionRejected)?;
    match input_product {
        IvQueryProduct::Smile(smile) => {
            let strike = smile
                .atm_strike
                .or_else(|| smile.points_by_strike.first().map(|point| point.strike))
                .ok_or(IvQueryError::ProjectionRejected)?;
            interpolate_smile(policy, &smile.points_by_strike, strike)
                .map(Some)
                .map_err(|_| IvQueryError::ProjectionRejected)
        }
        IvQueryProduct::Surface(surface) => {
            let smile = surface
                .smiles
                .iter()
                .find(|smile| !smile.points_by_strike.is_empty())
                .ok_or(IvQueryError::ProjectionRejected)?;
            let strike = smile
                .atm_strike
                .or_else(|| smile.points_by_strike.first().map(|point| point.strike))
                .ok_or(IvQueryError::ProjectionRejected)?;
            interpolate_smile(policy, &smile.points_by_strike, strike)
                .map(Some)
                .map_err(|_| IvQueryError::ProjectionRejected)
        }
        _ => Ok(None),
    }
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
        || state_filter
            .iter()
            .any(|expected| expected == health.subscription_state.as_str())
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
