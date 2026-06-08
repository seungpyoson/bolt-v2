use serde::{Deserialize, Serialize};

use super::{
    authz::IvSelectorAuthorization,
    derive::{IvDerivedInputSet, IvDerivedOutput, IvHelperPolicy, derive_iv, select_helper_policy},
    health::IvSourceHealth,
    ingest::IvRawEvent,
    policy::{IvPolicyInput, IvProjectionPolicy, project_scalar},
    provenance::{IvPolicyDecision, IvProvenance},
    selector::IvSelector,
    store::{IvAggregateGreeks, IvEvidence, IvGreeksPoint, IvPoint, IvSmile, IvStore, IvSurface},
    time::UnixNanos,
    types::IvProductKind,
};

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
pub struct IvQueryHandle {
    profile_id: String,
    authorization: IvSelectorAuthorization,
    store: IvStore,
    source_health: Vec<IvSourceHealth>,
    projection_policies: Vec<IvProjectionPolicy>,
    helper_policies: Vec<IvHelperPolicy>,
    derived_inputs: Vec<IvDerivedInputSet>,
}

impl IvQueryHandle {
    pub fn new(
        profile_id: impl Into<String>,
        authorization: IvSelectorAuthorization,
        store: IvStore,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            authorization,
            store,
            source_health: Vec::new(),
            projection_policies: Vec::new(),
            helper_policies: Vec::new(),
            derived_inputs: Vec::new(),
        }
    }

    pub fn with_source_health(mut self, source_health: Vec<IvSourceHealth>) -> Self {
        self.source_health = source_health;
        self
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

    pub fn with_derived_inputs(mut self, derived_inputs: Vec<IvDerivedInputSet>) -> Self {
        self.derived_inputs = derived_inputs;
        self
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

        let product = self.find_product(query)?;
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

        Ok(product)
    }

    fn find_product(&self, query: &IvProductQuery) -> Result<IvQueryProduct, IvQueryError> {
        match (&query.product_kind, &query.selector) {
            (
                IvProductKind::IvPoint,
                IvSelector::PointQuery {
                    instrument_ids,
                    basis,
                    as_of_ns,
                    source_filter,
                },
            ) => self
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
            ) => self
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
            ) => self
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
            ) => self
                .store
                .smiles()
                .iter()
                .find_map(|smile| {
                    if smile.profile_id == query.profile_id
                        && series_selectors.contains(&smile.surface_selector)
                        && smile.basis == *basis
                        && smile.ts_event_ns == *as_of_ns
                    {
                        self.store.surface(
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
            ) => self
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
            ) => self
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
            (IvProductKind::SourceHealth, IvSelector::SourceHealthQuery { source_filter, .. }) => {
                self.source_health
                    .iter()
                    .find(|health| {
                        health.profile_id == query.profile_id
                            && source_matches(&health.source_id, source_filter)
                    })
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
            ) => self.project_scalar_query(query, input_selector, projection_policy_id, *as_of_ns),
            (
                IvProductKind::DerivedIv,
                IvSelector::DerivedIvQuery {
                    instrument_id,
                    helper_policy_id,
                    as_of_ns,
                },
            ) => self.derived_iv_query(query, instrument_id, helper_policy_id, *as_of_ns),
            _ => Err(IvQueryError::ProductKindMismatch),
        }
    }

    fn project_scalar_query(
        &self,
        query: &IvProductQuery,
        input_selector: &IvSelector,
        projection_policy_id: &str,
        as_of_ns: UnixNanos,
    ) -> Result<IvQueryProduct, IvQueryError> {
        if matches!(input_selector, IvSelector::ProjectedScalarIvQuery { .. }) {
            return Err(IvQueryError::UnsupportedProductKind);
        }
        let policy = self
            .projection_policies
            .iter()
            .find(|policy| policy.policy_id == projection_policy_id)
            .ok_or(IvQueryError::ProjectionPolicyNotFound)?;
        let input_product = self.find_product(&IvProductQuery {
            strategy_id: query.strategy_id.clone(),
            profile_id: query.profile_id.clone(),
            product_kind: input_selector.product_kind(),
            selector: input_selector.clone(),
        })?;
        let inputs = projection_inputs(&input_product)?;
        let output =
            project_scalar(policy, &inputs).map_err(|_| IvQueryError::ProjectionRejected)?;
        let mut provenance = input_product
            .provenance()
            .cloned()
            .ok_or(IvQueryError::UnsupportedProductKind)?;
        provenance
            .policy_decisions
            .push(IvPolicyDecision::Projection);
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
        instrument_id: &str,
        helper_policy_id: &str,
        as_of_ns: UnixNanos,
    ) -> Result<IvQueryProduct, IvQueryError> {
        let policy = select_helper_policy(&self.helper_policies, helper_policy_id)
            .map_err(|_| IvQueryError::HelperPolicyNotFound)?;
        let inputs = self
            .derived_inputs
            .iter()
            .find(|inputs| {
                inputs.profile_id == query.profile_id
                    && inputs.instrument_id == instrument_id
                    && inputs.as_of_ns == as_of_ns
            })
            .cloned()
            .ok_or(IvQueryError::DerivedInputNotFound)?;
        derive_iv(policy, inputs)
            .map(Box::new)
            .map(IvQueryProduct::DerivedIv)
            .map_err(|_| IvQueryError::DerivationRejected)
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
            value: point.iv,
            ts_event_ns: point.ts_event_ns,
        }],
        IvQueryProduct::IvGreeksPoint(point) => vec![IvPolicyInput {
            product_id: point.point.instrument_id.clone(),
            value: point.point.iv,
            ts_event_ns: point.point.ts_event_ns,
        }],
        IvQueryProduct::Smile(smile) => smile
            .points_by_strike
            .iter()
            .map(|point| IvPolicyInput {
                product_id: smile.series_id.clone(),
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
                    value: point.iv,
                    ts_event_ns: smile.ts_event_ns,
                })
            })
            .collect(),
        IvQueryProduct::CustomIvEvidence(evidence) => vec![IvPolicyInput {
            product_id: evidence.iv_evidence_kind.clone(),
            value: evidence.value,
            ts_event_ns: evidence.ts_event_ns,
        }],
        IvQueryProduct::DerivedIv(derived) => vec![IvPolicyInput {
            product_id: derived.point.instrument_id.clone(),
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

fn source_matches(actual: &str, filter: &Option<String>) -> bool {
    filter.as_ref().is_none_or(|expected| actual == expected)
}

fn selector_supports_product_kind(selector: &IvSelector, product_kind: IvProductKind) -> bool {
    selector.product_kind() == product_kind
        || (product_kind == IvProductKind::IvGreeksPoint
            && matches!(selector, IvSelector::PointQuery { .. }))
}
