use rust_decimal::Decimal;

use crate::{
    EconomicScope, EconomicsError, EconomicsQuote, EdgeBasisPolicyId, FormulaId, SnapshotId,
    SourceIdentity,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeBasisAmount(Decimal);

impl EdgeBasisAmount {
    pub fn try_new(amount: Decimal) -> Result<Self, EconomicsError> {
        if amount <= Decimal::ZERO {
            return Err(EconomicsError::NonPositiveValue {
                field: "edge_basis_amount",
            });
        }
        Ok(Self(amount))
    }

    pub const fn amount(self) -> Decimal {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeBasisEvidence {
    pub policy_id: EdgeBasisPolicyId,
    pub resolver_id: FormulaId,
    pub product_metadata_source: SourceIdentity,
    pub policy_version: u64,
    pub normalized_amount: EdgeBasisAmount,
    pub scope: EconomicScope,
    pub source_snapshot_ids: Vec<SnapshotId>,
    pub valid_until_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetEdgeQuote {
    pub gross_expected_value: Decimal,
    pub core_net_edge: Decimal,
    pub forecast_net_edge: Decimal,
    pub core_edge_ratio: Decimal,
    pub forecast_edge_ratio: Decimal,
    pub basis: EdgeBasisEvidence,
}

pub fn fold_net_edge(
    gross_expected_value: Decimal,
    quote: &EconomicsQuote,
    basis: EdgeBasisEvidence,
) -> Result<NetEdgeQuote, EconomicsError> {
    if basis.policy_id != *quote.edge_basis_policy_id() {
        return Err(EconomicsError::EdgeBasisPolicyMismatch);
    }
    if !matches!(
        &basis.scope,
        EconomicScope::Decision {
            decision_correlation_id,
        } if decision_correlation_id == quote.decision_correlation_id()
    ) {
        return Err(EconomicsError::EdgeBasisScopeMismatch);
    }
    if basis.valid_until_ns < quote.requested_at_ns() {
        return Err(EconomicsError::StaleEdgeBasis);
    }
    let core_net_edge = gross_expected_value
        .checked_add(quote.core_total())
        .ok_or(EconomicsError::ArithmeticOverflow)?;
    let forecast_net_edge = gross_expected_value
        .checked_add(quote.forecast_total())
        .ok_or(EconomicsError::ArithmeticOverflow)?;
    let core_edge_ratio = core_net_edge
        .checked_div(basis.normalized_amount.amount())
        .ok_or(EconomicsError::ArithmeticOverflow)?;
    let forecast_edge_ratio = forecast_net_edge
        .checked_div(basis.normalized_amount.amount())
        .ok_or(EconomicsError::ArithmeticOverflow)?;
    Ok(NetEdgeQuote {
        gross_expected_value,
        core_net_edge,
        forecast_net_edge,
        core_edge_ratio,
        forecast_edge_ratio,
        basis,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccountId, DecisionCorrelationId, EconomicsInstrumentId, EconomicsQuoteRequest,
        ExecutionClientId, LifecyclePath, LiquidityRole, OrderSide, PlannedFillLeg,
        ProductSurfaceId, ReportingPolicyId, RoutingContext, SourceValidity, VenueQuoteEstimate,
        validate_and_aggregate_quote,
    };

    fn id<T>(value: &str, constructor: impl FnOnce(String) -> Result<T, EconomicsError>) -> T {
        constructor(value.to_owned()).expect("fixture identifier should be canonical")
    }

    #[test]
    fn fold_uses_evidence_backed_basis_and_rejects_policy_mismatch() {
        let policy_id = id("basis", EdgeBasisPolicyId::try_new);
        let request = EconomicsQuoteRequest {
            execution_client_id: id("execution", ExecutionClientId::try_new),
            account_id: id("account", AccountId::try_new),
            instrument_id: id("instrument", EconomicsInstrumentId::try_new),
            product_surface_id: id("surface", ProductSurfaceId::try_new),
            order_side: OrderSide::Buy,
            liquidity_role: LiquidityRole::Taker,
            planned_fill_legs: vec![PlannedFillLeg {
                price: Decimal::ONE,
                quantity: Decimal::ONE,
            }],
            routing: RoutingContext {
                attached_charge: None,
            },
            position: None,
            lifecycle_path: LifecyclePath::PlannedExit,
            reporting_policy_id: id("reporting", ReportingPolicyId::try_new),
            reporting_currency: id("USD", crate::CurrencyId::try_new),
            edge_basis_policy_id: policy_id.clone(),
            requested_at_ns: 1_000,
            decision_correlation_id: id("decision", DecisionCorrelationId::try_new),
        };
        let quote = validate_and_aggregate_quote(
            &request,
            VenueQuoteEstimate {
                authority: SourceValidity {
                    source: id("schedule", SourceIdentity::try_new),
                    snapshot_id: id("schedule-1", SnapshotId::try_new),
                    source_at_ns: 900,
                    fetched_at_ns: 950,
                    valid_until_ns: 1_100,
                },
                dependency_sources: Vec::new(),
                components: Vec::new(),
            },
            &[],
        )
        .expect("empty fee-free quote should aggregate");
        let basis = EdgeBasisEvidence {
            policy_id,
            resolver_id: id("resolver", FormulaId::try_new),
            product_metadata_source: id("product", SourceIdentity::try_new),
            policy_version: 1,
            normalized_amount: EdgeBasisAmount::try_new(Decimal::new(100, 0))
                .expect("positive basis should construct"),
            scope: EconomicScope::Decision {
                decision_correlation_id: id("decision", DecisionCorrelationId::try_new),
            },
            source_snapshot_ids: vec![id("product-1", SnapshotId::try_new)],
            valid_until_ns: 1_100,
        };
        let edge = fold_net_edge(Decimal::new(5, 0), &quote, basis.clone())
            .expect("matching basis should fold");
        assert_eq!(edge.core_net_edge, Decimal::new(5, 0));
        assert_eq!(edge.core_edge_ratio, Decimal::new(5, 2));

        let mismatched = EdgeBasisEvidence {
            policy_id: id("other", EdgeBasisPolicyId::try_new),
            ..basis.clone()
        };
        assert_eq!(
            fold_net_edge(Decimal::ONE, &quote, mismatched),
            Err(EconomicsError::EdgeBasisPolicyMismatch)
        );

        let foreign_scope = EdgeBasisEvidence {
            scope: EconomicScope::Decision {
                decision_correlation_id: id("other-decision", DecisionCorrelationId::try_new),
            },
            ..basis
        };
        assert_eq!(
            fold_net_edge(Decimal::ONE, &quote, foreign_scope),
            Err(EconomicsError::EdgeBasisScopeMismatch)
        );
    }
}
