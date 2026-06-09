use serde::{Deserialize, Serialize};

use super::{
    derive::IvDerivedInputSet,
    time::UnixNanos,
    types::{IvBasis, IvProductKind},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "selector_kind", rename_all = "snake_case")]
pub enum IvSelector {
    SourceOptionGreeks {
        instrument_ids: Vec<String>,
        nt_params: toml::Value,
    },
    SourceOptionChain {
        series_ids: Vec<String>,
        strike_range_policy: String,
        nt_params: toml::Value,
    },
    SourceAggregateGreeks {
        aggregate_key: String,
        underlying_selectors: Vec<String>,
        delta_field: String,
        gamma_field: String,
        vega_field: String,
        theta_field: String,
        rho_field: String,
        nt_params: toml::Value,
    },
    SourceCustomImpliedVolatility {
        custom_iv_data_type: String,
        custom_iv_data_fields: Vec<String>,
        nt_params: toml::Value,
    },
    PointQuery {
        instrument_ids: Vec<String>,
        basis: IvBasis,
        as_of_ns: UnixNanos,
        source_filter: Option<String>,
    },
    SmileQuery {
        series_id: String,
        side: Option<String>,
        basis: IvBasis,
        as_of_ns: UnixNanos,
    },
    SurfaceQuery {
        series_selectors: Vec<String>,
        basis: IvBasis,
        as_of_ns: UnixNanos,
    },
    AggregateGreeksQuery {
        aggregate_key: String,
        underlying_selectors: Vec<String>,
        as_of_ns: UnixNanos,
    },
    IvEvidenceQuery {
        iv_evidence_kind: String,
        source_filter: Option<String>,
        as_of_ns: UnixNanos,
    },
    ProjectedScalarIvQuery {
        input_selector: Box<IvSelector>,
        projection_policy_id: String,
        as_of_ns: UnixNanos,
    },
    DerivedIvQuery {
        instrument_id: String,
        helper_policy_id: String,
        as_of_ns: UnixNanos,
        inputs: Option<Box<IvDerivedInputSet>>,
    },
    SourceHealthQuery {
        source_filter: Option<String>,
        state_filter: Vec<String>,
    },
}

impl IvSelector {
    pub fn product_kind(&self) -> IvProductKind {
        match self {
            Self::PointQuery { .. } => IvProductKind::IvPoint,
            Self::SmileQuery { .. } => IvProductKind::Smile,
            Self::SurfaceQuery { .. } => IvProductKind::Surface,
            Self::AggregateGreeksQuery { .. } => IvProductKind::AggregateGreeks,
            Self::IvEvidenceQuery { .. } => IvProductKind::CustomIvEvidence,
            Self::ProjectedScalarIvQuery { .. } => IvProductKind::ProjectedScalarIv,
            Self::DerivedIvQuery { .. } => IvProductKind::DerivedIv,
            Self::SourceHealthQuery { .. } => IvProductKind::SourceHealth,
            Self::SourceOptionGreeks { .. } => IvProductKind::IvGreeksPoint,
            Self::SourceOptionChain { .. } => IvProductKind::Smile,
            Self::SourceAggregateGreeks { .. } => IvProductKind::AggregateGreeks,
            Self::SourceCustomImpliedVolatility { .. } => IvProductKind::CustomIvEvidence,
        }
    }
}
