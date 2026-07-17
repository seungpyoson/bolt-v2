use std::sync::Arc;

use rust_decimal::Decimal;

use crate::economics::{
    EconomicQuote, EconomicQuoteRequest, EconomicsUnavailable, EdgeBasisEvidence, NetEdgeQuote,
    SnapshotId, ValuationEvidence, VenueEconomicsAdapter, fold_net_edge,
    validate_and_aggregate_quote,
};

pub struct EconomicsAdmissionIntent {
    pub request: EconomicQuoteRequest,
    pub gross_expected_value: Decimal,
    pub edge_basis: EdgeBasisEvidence,
    pub valuations: Vec<ValuationEvidence>,
    pub base_reservation_notional: Decimal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EconomicsAdmission {
    request: EconomicQuoteRequest,
    quote: EconomicQuote,
    net_edge: NetEdgeQuote,
    base_reservation_notional: Decimal,
    reservation_notional: Decimal,
    source_snapshot_ids: Vec<SnapshotId>,
}

impl EconomicsAdmission {
    pub fn request(&self) -> &EconomicQuoteRequest {
        &self.request
    }

    pub fn quote(&self) -> &EconomicQuote {
        &self.quote
    }

    pub fn net_edge(&self) -> &NetEdgeQuote {
        &self.net_edge
    }

    pub fn reservation_notional(&self) -> Decimal {
        self.reservation_notional
    }

    pub fn base_reservation_notional(&self) -> Decimal {
        self.base_reservation_notional
    }

    pub fn source_snapshot_ids(&self) -> &[SnapshotId] {
        &self.source_snapshot_ids
    }
}

pub struct BoltV3EconomicsRuntime {
    adapter: Arc<dyn VenueEconomicsAdapter>,
}

impl BoltV3EconomicsRuntime {
    pub fn from_offline_adapter(adapter: Arc<dyn VenueEconomicsAdapter>) -> Self {
        Self { adapter }
    }

    pub fn quote_admission(
        &self,
        intent: EconomicsAdmissionIntent,
    ) -> Result<EconomicsAdmission, EconomicsUnavailable> {
        if intent.base_reservation_notional <= Decimal::ZERO {
            return Err(EconomicsUnavailable::InvalidPlannedFill);
        }
        let estimate = self.adapter.quote(&intent.request)?;
        let authority_snapshot_id = estimate.authority.snapshot_id.clone();
        let quote =
            validate_and_aggregate_quote(&intent.request, estimate, intent.valuations.as_slice())?;
        let net_edge = fold_net_edge(intent.gross_expected_value, &quote, intent.edge_basis)?;
        let debit_reservation = (-quote.core_total()).max(Decimal::ZERO);
        let reservation_notional = intent.base_reservation_notional + debit_reservation;
        let mut source_snapshot_ids = vec![authority_snapshot_id];
        source_snapshot_ids.extend(
            quote
                .components()
                .iter()
                .map(|component| component.source.snapshot_id.clone()),
        );
        source_snapshot_ids.sort();
        source_snapshot_ids.dedup();
        Ok(EconomicsAdmission {
            request: intent.request,
            quote,
            net_edge,
            base_reservation_notional: intent.base_reservation_notional,
            reservation_notional,
            source_snapshot_ids,
        })
    }
}
