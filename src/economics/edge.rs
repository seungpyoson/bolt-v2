use rust_decimal::Decimal;

use super::{EconomicQuote, EconomicsUnavailable, EdgeBasisEvidence, NetEdgeQuote};

pub fn fold_net_edge(
    gross_expected_value: Decimal,
    quote: &EconomicQuote,
    basis: EdgeBasisEvidence,
) -> Result<NetEdgeQuote, EconomicsUnavailable> {
    if basis.policy_id != quote.edge_basis_policy_id {
        return Err(EconomicsUnavailable::EdgeBasisPolicyMismatch);
    }
    if basis.valid_until_ns < quote.requested_at_ns {
        return Err(EconomicsUnavailable::StaleEdgeBasis {
            valid_until_ns: basis.valid_until_ns,
        });
    }

    let core_net_edge = gross_expected_value
        .checked_add(quote.core_total)
        .ok_or(EconomicsUnavailable::InvalidDecimal)?;
    let forecast_net_edge = gross_expected_value
        .checked_add(quote.forecast_total)
        .ok_or(EconomicsUnavailable::InvalidDecimal)?;
    let core_edge_ratio = core_net_edge
        .checked_div(basis.normalized_amount.amount())
        .ok_or(EconomicsUnavailable::InvalidDecimal)?;
    let forecast_edge_ratio = forecast_net_edge
        .checked_div(basis.normalized_amount.amount())
        .ok_or(EconomicsUnavailable::InvalidDecimal)?;
    Ok(NetEdgeQuote {
        gross_expected_value,
        core_net_edge,
        forecast_net_edge,
        core_edge_ratio,
        forecast_edge_ratio,
        basis,
    })
}
