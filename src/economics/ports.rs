use super::{
    EconomicQuoteRequest, EconomicsUnavailable, ResolvedEdgeBasis, SignedNativeEffect,
    ValuationEvidence, ValuationRequest, VenueQuoteEstimate,
};

pub trait VenueEconomicsAdapter: Send + Sync {
    fn resolve_edge_basis(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<ResolvedEdgeBasis, EconomicsUnavailable>;

    fn quote(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<VenueQuoteEstimate, EconomicsUnavailable>;
}

pub trait ValuationProvider: Send + Sync {
    fn value(
        &self,
        effect: &SignedNativeEffect,
        request: &ValuationRequest,
    ) -> Result<ValuationEvidence, EconomicsUnavailable>;
}
