use crate::{
    EconomicsError, EconomicsQuoteRequest, FormulaId, PlannedFillNotional, SnapshotId,
    SourceIdentity, VenueQuoteEstimate,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueEdgeBasisEstimate {
    pub resolver_id: FormulaId,
    pub product_metadata_source: SourceIdentity,
    pub policy_version: u64,
    pub normalized_amount: crate::EdgeBasisAmount,
    pub source_snapshot_ids: Vec<SnapshotId>,
    pub valid_until_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VenueEconomicsUnavailable {
    MissingAuthoritativeSnapshot,
    UnsupportedProductEconomics,
    InvalidAuthoritativeSnapshot,
    RequestScopeMismatch,
    InvalidQuote(EconomicsError),
}

impl std::fmt::Display for VenueEconomicsUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingAuthoritativeSnapshot => {
                f.write_str("venue economics authoritative snapshot is unavailable")
            }
            Self::UnsupportedProductEconomics => {
                f.write_str("venue economics does not support the requested product")
            }
            Self::InvalidAuthoritativeSnapshot => {
                f.write_str("venue economics authoritative snapshot is invalid")
            }
            Self::RequestScopeMismatch => {
                f.write_str("venue economics request does not match its configured authority")
            }
            Self::InvalidQuote(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for VenueEconomicsUnavailable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidQuote(error) => Some(error),
            Self::MissingAuthoritativeSnapshot
            | Self::UnsupportedProductEconomics
            | Self::InvalidAuthoritativeSnapshot
            | Self::RequestScopeMismatch => None,
        }
    }
}

impl From<EconomicsError> for VenueEconomicsUnavailable {
    fn from(value: EconomicsError) -> Self {
        Self::InvalidQuote(value)
    }
}

pub trait VenueEconomicsAdapter: Send + Sync {
    fn provider_key(&self) -> &str;

    fn resolve_edge_basis(
        &self,
        request: &EconomicsQuoteRequest,
        planned_fill_notional: PlannedFillNotional,
    ) -> Result<VenueEdgeBasisEstimate, VenueEconomicsUnavailable>;

    fn quote(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<VenueQuoteEstimate, VenueEconomicsUnavailable>;
}
