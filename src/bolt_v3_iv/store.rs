use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    error::IvRejectReason,
    ingest::{
        IvAggregateGreeksPayload, IvCustomIvPayload, IvIngestEvent, IvOptionChainSlicePayload,
        IvOptionChainStrikePayload, IvOptionGreeksPayload, IvRawEvent, IvRawPayload,
        preserve_raw_event,
    },
    provenance::{IvProvenance, validate_iv_provenance},
    time::UnixNanos,
    types::{IvBasis, IvConvention},
};

const OPTION_CHAIN_CALL_SIDE_LABEL: &str = "call";
const OPTION_CHAIN_PUT_SIDE_LABEL: &str = "put";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvPoint {
    pub profile_id: String,
    pub source_id: String,
    pub instrument_id: String,
    pub basis: IvBasis,
    pub iv: f64,
    pub convention: IvConvention,
    pub ts_event_ns: UnixNanos,
    pub ts_init_ns: Option<UnixNanos>,
    pub provenance: IvProvenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvGreeksPoint {
    pub point: IvPoint,
    pub greeks: super::ingest::IvGreekValues,
    pub underlying_price: Option<f64>,
    pub open_interest: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IvSmilePoint {
    pub strike: f64,
    pub iv: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvSmile {
    pub profile_id: String,
    pub source_id: String,
    pub surface_selector: String,
    pub series_id: String,
    pub side: String,
    pub basis: IvBasis,
    pub points_by_strike: Vec<IvSmilePoint>,
    pub atm_strike: Option<f64>,
    pub ts_event_ns: UnixNanos,
    pub provenance: IvProvenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvSurface {
    pub profile_id: String,
    pub surface_selector: String,
    pub source_id: String,
    pub basis: IvBasis,
    pub smiles: Vec<IvSmile>,
    pub as_of_ns: UnixNanos,
    pub provenance: IvProvenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvAggregateGreeks {
    pub profile_id: String,
    pub source_id: String,
    pub aggregate_key: String,
    pub underlying_selectors: Vec<String>,
    pub greeks: super::ingest::IvGreekValues,
    pub ts_event_ns: UnixNanos,
    pub ts_init_ns: Option<UnixNanos>,
    pub provenance: IvProvenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvEvidence {
    pub profile_id: String,
    pub source_id: String,
    pub iv_evidence_kind: String,
    pub value: f64,
    pub ts_event_ns: UnixNanos,
    pub ts_init_ns: Option<UnixNanos>,
    pub provenance: IvProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IvRetentionPolicy {
    pub max_raw_events: usize,
    pub max_indexed_points: usize,
    pub max_smiles: usize,
    pub max_surfaces: usize,
    pub max_source_health_events: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IvStoreError {
    PayloadKindMismatch,
    MissingIvBasis,
    InvalidIvValue,
    ProvenanceIncomplete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvStore {
    raw_events: Vec<IvRawEvent>,
    iv_points: Vec<IvPoint>,
    greeks_points: Vec<IvGreeksPoint>,
    smiles: Vec<IvSmile>,
    aggregate_greeks: Vec<IvAggregateGreeks>,
    iv_evidence: Vec<IvEvidence>,
    next_ingest_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IvStoreIndexCheckpoint {
    iv_points: usize,
    greeks_points: usize,
    smiles: usize,
    aggregate_greeks: usize,
    iv_evidence: usize,
}

impl IvStore {
    pub fn empty() -> Self {
        Self {
            raw_events: Vec::new(),
            iv_points: Vec::new(),
            greeks_points: Vec::new(),
            smiles: Vec::new(),
            aggregate_greeks: Vec::new(),
            iv_evidence: Vec::new(),
            next_ingest_sequence: u64::MIN,
        }
    }

    pub fn ingest_event(&mut self, event: IvIngestEvent) -> Result<IvRawEvent, IvStoreError> {
        if !event.payload.matches_source_kind(event.source_kind) {
            return Err(IvStoreError::PayloadKindMismatch);
        }

        self.next_ingest_sequence += 1;
        let raw_event = preserve_raw_event(event, self.next_ingest_sequence);
        self.raw_events.push(raw_event.clone());
        let checkpoint = self.index_checkpoint();
        if let Err(error) = self.index_raw_event(&raw_event) {
            self.rollback_indexes(checkpoint);
            return Err(error);
        }
        Ok(raw_event)
    }

    pub fn raw_events(&self) -> &[IvRawEvent] {
        &self.raw_events
    }

    pub fn raw_event(&self, raw_event_id: &str) -> Option<&IvRawEvent> {
        self.raw_events
            .iter()
            .find(|event| event.raw_event_id == raw_event_id)
    }

    pub fn iv_points(&self) -> &[IvPoint] {
        &self.iv_points
    }

    pub fn greeks_points(&self) -> &[IvGreeksPoint] {
        &self.greeks_points
    }

    pub fn smiles(&self) -> &[IvSmile] {
        &self.smiles
    }

    pub fn aggregate_greeks(&self) -> &[IvAggregateGreeks] {
        &self.aggregate_greeks
    }

    pub fn iv_evidence(&self) -> &[IvEvidence] {
        &self.iv_evidence
    }

    pub fn surface(
        &self,
        surface_selector: &str,
        source_id: &str,
        basis: IvBasis,
        as_of_ns: UnixNanos,
    ) -> Option<IvSurface> {
        let smiles = self
            .smiles
            .iter()
            .filter(|smile| {
                smile.surface_selector == surface_selector
                    && smile.source_id == source_id
                    && smile.basis == basis
                    && smile.ts_event_ns == as_of_ns
            })
            .cloned()
            .collect::<Vec<_>>();
        let provenance = smiles.first().map(|smile| smile.provenance.clone())?;

        Some(IvSurface {
            profile_id: provenance.profile_id.clone(),
            surface_selector: surface_selector.to_string(),
            source_id: source_id.to_string(),
            basis,
            smiles,
            as_of_ns,
            provenance,
        })
    }

    pub fn all_product_provenance(&self) -> Vec<&IvProvenance> {
        let mut provenance = Vec::new();
        provenance.extend(self.iv_points.iter().map(|point| &point.provenance));
        provenance.extend(
            self.greeks_points
                .iter()
                .map(|point| &point.point.provenance),
        );
        provenance.extend(self.smiles.iter().map(|smile| &smile.provenance));
        provenance.extend(
            self.aggregate_greeks
                .iter()
                .map(|aggregate| &aggregate.provenance),
        );
        provenance.extend(self.iv_evidence.iter().map(|evidence| &evidence.provenance));
        provenance
    }

    pub fn enforce_retention(&mut self, policy: &IvRetentionPolicy) {
        truncate_front(&mut self.raw_events, policy.max_raw_events);
        truncate_front(&mut self.iv_points, policy.max_indexed_points);
        truncate_front(&mut self.greeks_points, policy.max_indexed_points);
        truncate_front(&mut self.smiles, policy.max_smiles);
        truncate_front(&mut self.aggregate_greeks, policy.max_indexed_points);
        truncate_front(&mut self.iv_evidence, policy.max_indexed_points);
    }

    fn index_raw_event(&mut self, raw_event: &IvRawEvent) -> Result<(), IvStoreError> {
        validate_iv_provenance(&raw_event.provenance)
            .map_err(|_| IvStoreError::ProvenanceIncomplete)?;

        match &raw_event.payload {
            IvRawPayload::OptionGreeks(payload) => self.index_option_greeks(raw_event, payload),
            IvRawPayload::OptionChainSlice(payload) => {
                self.index_option_chain_slice(raw_event, payload)
            }
            IvRawPayload::AggregateGreeks(payload) => {
                self.index_aggregate_greeks(raw_event, payload);
                Ok(())
            }
            IvRawPayload::CustomImpliedVolatility(payload) => {
                self.index_custom_iv(raw_event, payload)
            }
        }
    }

    fn index_checkpoint(&self) -> IvStoreIndexCheckpoint {
        IvStoreIndexCheckpoint {
            iv_points: self.iv_points.len(),
            greeks_points: self.greeks_points.len(),
            smiles: self.smiles.len(),
            aggregate_greeks: self.aggregate_greeks.len(),
            iv_evidence: self.iv_evidence.len(),
        }
    }

    fn rollback_indexes(&mut self, checkpoint: IvStoreIndexCheckpoint) {
        self.iv_points.truncate(checkpoint.iv_points);
        self.greeks_points.truncate(checkpoint.greeks_points);
        self.smiles.truncate(checkpoint.smiles);
        self.aggregate_greeks.truncate(checkpoint.aggregate_greeks);
        self.iv_evidence.truncate(checkpoint.iv_evidence);
    }

    fn index_option_greeks(
        &mut self,
        raw_event: &IvRawEvent,
        payload: &IvOptionGreeksPayload,
    ) -> Result<(), IvStoreError> {
        if payload.basis_values.is_empty() {
            return Err(IvStoreError::MissingIvBasis);
        }
        for basis_value in &payload.basis_values {
            if !valid_iv(basis_value.iv) {
                return Err(IvStoreError::InvalidIvValue);
            }

            let point = IvPoint {
                profile_id: raw_event.profile_id.clone(),
                source_id: raw_event.source_id.clone(),
                instrument_id: payload.instrument_id.clone(),
                basis: basis_value.basis,
                iv: basis_value.iv,
                convention: payload.convention.clone(),
                ts_event_ns: raw_event.provenance.ts_event_ns,
                ts_init_ns: raw_event.provenance.ts_init_ns,
                provenance: raw_event.provenance.clone(),
            };
            self.iv_points.push(point.clone());
            self.greeks_points.push(IvGreeksPoint {
                point,
                greeks: payload.greeks,
                underlying_price: payload.underlying_price,
                open_interest: payload.open_interest,
            });
        }

        Ok(())
    }

    fn index_option_chain_slice(
        &mut self,
        raw_event: &IvRawEvent,
        payload: &IvOptionChainSlicePayload,
    ) -> Result<(), IvStoreError> {
        let indexed_calls = self.index_option_chain_side(
            raw_event,
            payload,
            OPTION_CHAIN_CALL_SIDE_LABEL,
            &payload.calls,
        )?;
        let indexed_puts = self.index_option_chain_side(
            raw_event,
            payload,
            OPTION_CHAIN_PUT_SIDE_LABEL,
            &payload.puts,
        )?;
        if !indexed_calls && !indexed_puts {
            return Err(IvStoreError::MissingIvBasis);
        }
        Ok(())
    }

    fn index_option_chain_side(
        &mut self,
        raw_event: &IvRawEvent,
        payload: &IvOptionChainSlicePayload,
        side: &str,
        strikes: &[IvOptionChainStrikePayload],
    ) -> Result<bool, IvStoreError> {
        let mut points_by_basis = BTreeMap::<IvBasis, Vec<IvSmilePoint>>::new();
        for strike in strikes {
            let Some(greeks) = &strike.greeks else {
                continue;
            };
            for basis_value in &greeks.basis_values {
                if !valid_iv(basis_value.iv) {
                    return Err(IvStoreError::InvalidIvValue);
                }
                points_by_basis
                    .entry(basis_value.basis)
                    .or_insert_with(empty_iv_smile_points)
                    .push(IvSmilePoint {
                        strike: strike.strike,
                        iv: basis_value.iv,
                    });
            }
        }

        let indexed_side = !points_by_basis.is_empty();
        for (basis, mut points_by_strike) in points_by_basis {
            points_by_strike.sort_by(|left, right| left.strike.total_cmp(&right.strike));
            self.smiles.push(IvSmile {
                profile_id: raw_event.profile_id.clone(),
                source_id: raw_event.source_id.clone(),
                surface_selector: payload.surface_selector.clone(),
                series_id: payload.series_id.clone(),
                side: side.to_string(),
                basis,
                points_by_strike,
                atm_strike: payload.atm_strike,
                ts_event_ns: raw_event.provenance.ts_event_ns,
                provenance: raw_event.provenance.clone(),
            });
        }

        Ok(indexed_side)
    }

    fn index_aggregate_greeks(
        &mut self,
        raw_event: &IvRawEvent,
        payload: &IvAggregateGreeksPayload,
    ) {
        self.aggregate_greeks.push(IvAggregateGreeks {
            profile_id: raw_event.profile_id.clone(),
            source_id: raw_event.source_id.clone(),
            aggregate_key: payload.aggregate_key.clone(),
            underlying_selectors: payload.underlying_selectors.clone(),
            greeks: payload.greeks,
            ts_event_ns: raw_event.provenance.ts_event_ns,
            ts_init_ns: raw_event.provenance.ts_init_ns,
            provenance: raw_event.provenance.clone(),
        });
    }

    fn index_custom_iv(
        &mut self,
        raw_event: &IvRawEvent,
        payload: &IvCustomIvPayload,
    ) -> Result<(), IvStoreError> {
        if !valid_iv(payload.value) {
            return Err(IvStoreError::InvalidIvValue);
        }

        self.iv_evidence.push(IvEvidence {
            profile_id: raw_event.profile_id.clone(),
            source_id: raw_event.source_id.clone(),
            iv_evidence_kind: payload.iv_evidence_kind.clone(),
            value: payload.value,
            ts_event_ns: raw_event.provenance.ts_event_ns,
            ts_init_ns: raw_event.provenance.ts_init_ns,
            provenance: raw_event.provenance.clone(),
        });

        Ok(())
    }
}

fn valid_iv(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn empty_iv_smile_points() -> Vec<IvSmilePoint> {
    Vec::new()
}

fn truncate_front<T>(items: &mut Vec<T>, max_len: usize) {
    if items.len() > max_len {
        let remove_count = items.len() - max_len;
        items.drain(0..remove_count);
    }
}

impl From<IvRejectReason> for IvStoreError {
    fn from(reason: IvRejectReason) -> Self {
        match reason {
            IvRejectReason::ProvenanceIncomplete => Self::ProvenanceIncomplete,
            IvRejectReason::MissingIvBasis => Self::MissingIvBasis,
            _ => Self::InvalidIvValue,
        }
    }
}

impl IvStoreError {
    pub fn reject_reason(&self) -> IvRejectReason {
        match self {
            Self::PayloadKindMismatch => IvRejectReason::PayloadKindMismatch,
            Self::MissingIvBasis => IvRejectReason::MissingIvBasis,
            Self::InvalidIvValue => IvRejectReason::InvalidIvValue,
            Self::ProvenanceIncomplete => IvRejectReason::ProvenanceIncomplete,
        }
    }
}
