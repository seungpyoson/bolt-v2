use super::{
    EconomicQuoteRequest, EconomicsUnavailable, SignedNativeEffect, ValuationEvidence,
    ValuationRequest, VenueQuoteEstimate,
};

pub trait VenueEconomicsAdapter: Send + Sync {
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
