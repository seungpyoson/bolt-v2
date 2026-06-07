use serde::{Deserialize, Serialize};

use super::{
    health::IvSourceHealthState,
    provenance::{IvProvenance, IvProvenanceSeed},
    time::UnixNanos,
    types::{IvBasis, IvConvention, IvSourceKind},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvPayloadKind {
    OptionGreeks,
    OptionChainSlice,
    AggregateGreeks,
    CustomImpliedVolatility,
}

impl IvPayloadKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OptionGreeks => "option_greeks",
            Self::OptionChainSlice => "option_chain_slice",
            Self::AggregateGreeks => "aggregate_greeks",
            Self::CustomImpliedVolatility => "custom_implied_volatility",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IvGreekValues {
    pub delta: Option<f64>,
    pub gamma: Option<f64>,
    pub vega: Option<f64>,
    pub theta: Option<f64>,
    pub rho: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IvBasisValue {
    pub basis: IvBasis,
    pub iv: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvOptionGreeksPayload {
    pub instrument_id: String,
    pub convention: IvConvention,
    pub basis_values: Vec<IvBasisValue>,
    pub greeks: IvGreekValues,
    pub underlying_price: Option<f64>,
    pub open_interest: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IvOptionChainPoint {
    pub strike: f64,
    pub iv: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvOptionChainSlicePayload {
    pub series_id: String,
    pub surface_selector: String,
    pub side: String,
    pub basis: IvBasis,
    pub points: Vec<IvOptionChainPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvAggregateGreeksPayload {
    pub aggregate_key: String,
    pub underlying_selectors: Vec<String>,
    pub greeks: IvGreekValues,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvCustomIvPayload {
    pub iv_evidence_kind: String,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "payload_kind", rename_all = "snake_case")]
pub enum IvRawPayload {
    OptionGreeks(IvOptionGreeksPayload),
    OptionChainSlice(IvOptionChainSlicePayload),
    AggregateGreeks(IvAggregateGreeksPayload),
    CustomImpliedVolatility(IvCustomIvPayload),
}

impl IvRawPayload {
    pub fn payload_kind(&self) -> IvPayloadKind {
        match self {
            Self::OptionGreeks(_) => IvPayloadKind::OptionGreeks,
            Self::OptionChainSlice(_) => IvPayloadKind::OptionChainSlice,
            Self::AggregateGreeks(_) => IvPayloadKind::AggregateGreeks,
            Self::CustomImpliedVolatility(_) => IvPayloadKind::CustomImpliedVolatility,
        }
    }

    pub fn matches_source_kind(&self, source_kind: IvSourceKind) -> bool {
        matches!(
            (source_kind, self),
            (IvSourceKind::OptionGreeks, Self::OptionGreeks(_))
                | (IvSourceKind::OptionChain, Self::OptionChainSlice(_))
                | (IvSourceKind::AggregateGreeks, Self::AggregateGreeks(_))
                | (
                    IvSourceKind::CustomImpliedVolatility,
                    Self::CustomImpliedVolatility(_)
                )
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvIngestEvent {
    pub profile_id: String,
    pub source_id: String,
    pub source_kind: IvSourceKind,
    pub selector_fingerprint: String,
    pub nt_revision: String,
    pub nt_evidence_path: String,
    pub nt_symbol: String,
    pub ts_event_ns: UnixNanos,
    pub ts_init_ns: Option<UnixNanos>,
    pub received_ts_ns: UnixNanos,
    pub subscription_generation: u64,
    pub source_health_state: IvSourceHealthState,
    pub payload: IvRawPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvRawEvent {
    pub raw_event_id: String,
    pub profile_id: String,
    pub source_id: String,
    pub received_ts_ns: UnixNanos,
    pub payload_kind: String,
    pub payload: IvRawPayload,
    pub provenance: IvProvenance,
}

pub fn preserve_raw_event(event: IvIngestEvent, ingest_sequence: u64) -> IvRawEvent {
    let payload_kind = event.payload.payload_kind().as_str().to_string();
    let raw_event_id = format!(
        "{}:{}:{}",
        event.profile_id, event.source_id, ingest_sequence
    );
    let provenance = IvProvenance::from_raw_event(
        IvProvenanceSeed {
            profile_id: event.profile_id.clone(),
            source_id: event.source_id.clone(),
            source_kind: event.source_kind,
            selector_fingerprint: event.selector_fingerprint,
            nt_revision: event.nt_revision,
            nt_evidence_path: event.nt_evidence_path,
            nt_symbol: event.nt_symbol,
            ts_event_ns: event.ts_event_ns,
            ts_init_ns: event.ts_init_ns,
            received_ts_ns: event.received_ts_ns,
            ingest_sequence,
            subscription_generation: event.subscription_generation,
            source_health_state: event.source_health_state,
        },
        raw_event_id.clone(),
        payload_kind.clone(),
    );

    IvRawEvent {
        raw_event_id,
        profile_id: event.profile_id,
        source_id: event.source_id,
        received_ts_ns: event.received_ts_ns,
        payload_kind,
        payload: event.payload,
        provenance,
    }
}
