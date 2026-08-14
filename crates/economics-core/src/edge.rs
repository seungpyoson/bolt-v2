use rust_decimal::Decimal;

use crate::{
    CurrencyId, EconomicScope, EconomicsError, EconomicsQuote, EdgeBasisPolicyId, FormulaId,
    SnapshotId, SourceIdentity,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrossExpectedValue {
    amount: Decimal,
    currency_id: CurrencyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitVsHoldDecision {
    Hold,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeAdjustedLegValue {
    gross_value: Decimal,
    execution_economics: Decimal,
}

impl FeeAdjustedLegValue {
    pub const fn new(gross_value: Decimal, execution_economics: Decimal) -> Self {
        Self {
            gross_value,
            execution_economics,
        }
    }

    pub const fn proven_zero_execution_economics(gross_value: Decimal) -> Self {
        Self::new(gross_value, Decimal::ZERO)
    }

    pub fn net_value(self) -> Result<Decimal, EconomicsError> {
        self.gross_value
            .checked_add(self.execution_economics)
            .ok_or(EconomicsError::ArithmeticOverflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeAdjustedExitVsHoldComparison {
    decision: ExitVsHoldDecision,
    hold_net_value: Decimal,
    exit_net_value: Decimal,
}

impl FeeAdjustedExitVsHoldComparison {
    pub const fn decision(self) -> ExitVsHoldDecision {
        self.decision
    }

    pub const fn hold_net_value(self) -> Decimal {
        self.hold_net_value
    }

    pub const fn exit_net_value(self) -> Decimal {
        self.exit_net_value
    }
}

pub fn compare_fee_adjusted_exit_vs_hold(
    hold: FeeAdjustedLegValue,
    exit: FeeAdjustedLegValue,
    hysteresis: Decimal,
) -> Result<FeeAdjustedExitVsHoldComparison, EconomicsError> {
    if hysteresis.is_sign_negative() {
        return Err(EconomicsError::InvalidExitVsHoldHysteresis);
    }
    let hold_net_value = hold.net_value()?;
    let exit_net_value = exit.net_value()?;
    let exit_threshold = hold_net_value
        .checked_add(hysteresis)
        .ok_or(EconomicsError::ArithmeticOverflow)?;
    let decision = if exit_net_value > exit_threshold {
        ExitVsHoldDecision::Exit
    } else {
        ExitVsHoldDecision::Hold
    };
    Ok(FeeAdjustedExitVsHoldComparison {
        decision,
        hold_net_value,
        exit_net_value,
    })
}

impl GrossExpectedValue {
    pub fn new(amount: Decimal, currency_id: CurrencyId) -> Self {
        Self {
            amount,
            currency_id,
        }
    }

    pub const fn amount(&self) -> Decimal {
        self.amount
    }

    pub const fn currency_id(&self) -> &CurrencyId {
        &self.currency_id
    }
}

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
    gross_expected_value: GrossExpectedValue,
    quote: &EconomicsQuote,
    basis: EdgeBasisEvidence,
) -> Result<NetEdgeQuote, EconomicsError> {
    if gross_expected_value.currency_id() != quote.reporting_currency() {
        return Err(EconomicsError::GrossCurrencyMismatch {
            gross_currency: gross_expected_value.currency_id().clone(),
            reporting_currency: quote.reporting_currency().clone(),
        });
    }
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
    let gross_expected_value = gross_expected_value.amount();
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
    fn fee_adjusted_exit_comparison_holds_when_fees_reverse_the_gross_preference() {
        let hold = FeeAdjustedLegValue::proven_zero_execution_economics(Decimal::new(495, 3));
        let gross_favorable_exit =
            FeeAdjustedLegValue::new(Decimal::new(500, 3), Decimal::new(-75, 4));
        let zero_fee_exit =
            FeeAdjustedLegValue::proven_zero_execution_economics(Decimal::new(500, 3));

        assert_eq!(
            compare_fee_adjusted_exit_vs_hold(hold, gross_favorable_exit, Decimal::ZERO)
                .expect("fee-adjusted comparison should succeed")
                .decision(),
            ExitVsHoldDecision::Hold
        );
        assert_eq!(
            compare_fee_adjusted_exit_vs_hold(hold, zero_fee_exit, Decimal::ZERO)
                .expect("zero-fee comparison should succeed")
                .decision(),
            ExitVsHoldDecision::Exit
        );
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
        let edge = fold_net_edge(
            GrossExpectedValue::new(Decimal::new(5, 0), id("USD", crate::CurrencyId::try_new)),
            &quote,
            basis.clone(),
        )
        .expect("matching basis should fold");
        assert_eq!(edge.core_net_edge, Decimal::new(5, 0));
        assert_eq!(edge.core_edge_ratio, Decimal::new(5, 2));

        let mismatched = EdgeBasisEvidence {
            policy_id: id("other", EdgeBasisPolicyId::try_new),
            ..basis.clone()
        };
        assert_eq!(
            fold_net_edge(
                GrossExpectedValue::new(Decimal::ONE, id("USD", crate::CurrencyId::try_new),),
                &quote,
                mismatched,
            ),
            Err(EconomicsError::EdgeBasisPolicyMismatch)
        );

        let foreign_scope = EdgeBasisEvidence {
            scope: EconomicScope::Decision {
                decision_correlation_id: id("other-decision", DecisionCorrelationId::try_new),
            },
            ..basis
        };
        assert_eq!(
            fold_net_edge(
                GrossExpectedValue::new(Decimal::ONE, id("USD", crate::CurrencyId::try_new),),
                &quote,
                foreign_scope,
            ),
            Err(EconomicsError::EdgeBasisScopeMismatch)
        );
    }

    #[test]
    fn fold_rejects_gross_value_in_a_different_currency() {
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
            normalized_amount: EdgeBasisAmount::try_new(Decimal::ONE)
                .expect("positive basis should construct"),
            scope: EconomicScope::Decision {
                decision_correlation_id: id("decision", DecisionCorrelationId::try_new),
            },
            source_snapshot_ids: vec![id("product-1", SnapshotId::try_new)],
            valid_until_ns: 1_100,
        };

        assert_eq!(
            fold_net_edge(
                GrossExpectedValue::new(Decimal::ONE, id("EUR", crate::CurrencyId::try_new),),
                &quote,
                basis,
            ),
            Err(EconomicsError::GrossCurrencyMismatch {
                gross_currency: id("EUR", crate::CurrencyId::try_new),
                reporting_currency: id("USD", crate::CurrencyId::try_new),
            })
        );
    }
}
