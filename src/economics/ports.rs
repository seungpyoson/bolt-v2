use super::{
    EconomicQuoteRequest, EconomicsUnavailable, EstimatedEconomicComponent, SignedNativeEffect,
    ValuationEvidence, ValuationRequest,
};

pub trait VenueEconomicsAdapter: Send + Sync {
    fn quote(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<Vec<EstimatedEconomicComponent>, EconomicsUnavailable>;
}

pub trait ValuationProvider: Send + Sync {
    fn value(
        &self,
        effect: &SignedNativeEffect,
        request: &ValuationRequest,
    ) -> Result<ValuationEvidence, EconomicsUnavailable>;
}
