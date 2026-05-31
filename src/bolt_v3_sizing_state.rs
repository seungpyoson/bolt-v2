use rust_decimal::Decimal;

use crate::bolt_v3_loss_governor::LossSnapshot;
use crate::bolt_v3_position_sizer::ProductSizingSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizingStateError {
    MissingNtState,
    StaleNtState(SizingStateEvidenceKind),
    UnattributedState(SizingStateEvidenceKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizingStateEvidenceKind {
    State,
    Portfolio,
    OrderLifecycle,
    ProductState,
    ReservationLedger,
    LossSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtDerivedSizingState {
    pub source: String,
    pub observed_at_ns: u64,
    pub portfolio: PortfolioSizingSnapshot,
    pub order_lifecycle: OrderLifecycleSizingSnapshot,
    pub product_state: ProductSizingSnapshot,
    pub reservation_snapshot: ReservationLedgerSnapshot,
    pub loss_snapshot: Option<LossSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortfolioSizingSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub venue_id: String,
    pub account_id: String,
    pub collateral_currency: String,
    pub free_collateral: Decimal,
    pub total_equity: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLifecycleSizingSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub open_order_count: usize,
    pub all_open_orders_attributed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationLedgerSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub all_live_reservations_attributed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizingStateEvidenceSource {
    pub kind: SizingStateEvidenceKind,
    pub source: String,
    pub observed_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizingStateEvidence {
    pub sources: Vec<SizingStateEvidenceSource>,
}

pub fn validate_nt_derived_sizing_state(
    state: Option<&NtDerivedSizingState>,
    now_ns: u64,
    max_snapshot_age_ns: u64,
) -> Result<SizingStateEvidence, SizingStateError> {
    let Some(state) = state else {
        return Err(SizingStateError::MissingNtState);
    };

    validate_source(&state.source, SizingStateEvidenceKind::State)?;
    validate_freshness(
        state.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        SizingStateEvidenceKind::State,
    )?;
    validate_source(&state.portfolio.source, SizingStateEvidenceKind::Portfolio)?;
    validate_freshness(
        state.portfolio.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        SizingStateEvidenceKind::Portfolio,
    )?;
    validate_source(
        &state.order_lifecycle.source,
        SizingStateEvidenceKind::OrderLifecycle,
    )?;
    validate_freshness(
        state.order_lifecycle.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        SizingStateEvidenceKind::OrderLifecycle,
    )?;
    if !state.order_lifecycle.all_open_orders_attributed {
        return Err(SizingStateError::UnattributedState(
            SizingStateEvidenceKind::OrderLifecycle,
        ));
    }

    let (product_source, product_observed_at_ns) = match &state.product_state {
        ProductSizingSnapshot::PredictionMarketBinary(snapshot) => {
            (snapshot.source.as_str(), snapshot.observed_at_ns)
        }
    };
    validate_source(product_source, SizingStateEvidenceKind::ProductState)?;
    validate_freshness(
        product_observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        SizingStateEvidenceKind::ProductState,
    )?;

    validate_source(
        &state.reservation_snapshot.source,
        SizingStateEvidenceKind::ReservationLedger,
    )?;
    validate_freshness(
        state.reservation_snapshot.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        SizingStateEvidenceKind::ReservationLedger,
    )?;
    if !state.reservation_snapshot.all_live_reservations_attributed {
        return Err(SizingStateError::UnattributedState(
            SizingStateEvidenceKind::ReservationLedger,
        ));
    }

    if let Some(loss_snapshot) = &state.loss_snapshot {
        validate_source(&loss_snapshot.source, SizingStateEvidenceKind::LossSnapshot)?;
        validate_freshness(
            loss_snapshot.observed_at_ns,
            now_ns,
            max_snapshot_age_ns,
            SizingStateEvidenceKind::LossSnapshot,
        )?;
    }

    Ok(SizingStateEvidence {
        sources: evidence_sources(state),
    })
}

fn validate_source(source: &str, kind: SizingStateEvidenceKind) -> Result<(), SizingStateError> {
    if source.trim().is_empty() {
        return Err(SizingStateError::UnattributedState(kind));
    }
    Ok(())
}

fn validate_freshness(
    observed_at_ns: u64,
    now_ns: u64,
    max_snapshot_age_ns: u64,
    kind: SizingStateEvidenceKind,
) -> Result<(), SizingStateError> {
    if observed_at_ns > now_ns || now_ns - observed_at_ns > max_snapshot_age_ns {
        return Err(SizingStateError::StaleNtState(kind));
    }
    Ok(())
}

fn evidence_sources(state: &NtDerivedSizingState) -> Vec<SizingStateEvidenceSource> {
    let (product_source, product_observed_at_ns) = match &state.product_state {
        ProductSizingSnapshot::PredictionMarketBinary(snapshot) => {
            (snapshot.source.clone(), snapshot.observed_at_ns)
        }
    };
    let mut sources = vec![
        SizingStateEvidenceSource {
            kind: SizingStateEvidenceKind::State,
            source: state.source.clone(),
            observed_at_ns: state.observed_at_ns,
        },
        SizingStateEvidenceSource {
            kind: SizingStateEvidenceKind::Portfolio,
            source: state.portfolio.source.clone(),
            observed_at_ns: state.portfolio.observed_at_ns,
        },
        SizingStateEvidenceSource {
            kind: SizingStateEvidenceKind::OrderLifecycle,
            source: state.order_lifecycle.source.clone(),
            observed_at_ns: state.order_lifecycle.observed_at_ns,
        },
        SizingStateEvidenceSource {
            kind: SizingStateEvidenceKind::ProductState,
            source: product_source,
            observed_at_ns: product_observed_at_ns,
        },
        SizingStateEvidenceSource {
            kind: SizingStateEvidenceKind::ReservationLedger,
            source: state.reservation_snapshot.source.clone(),
            observed_at_ns: state.reservation_snapshot.observed_at_ns,
        },
    ];
    if let Some(loss_snapshot) = &state.loss_snapshot {
        sources.push(SizingStateEvidenceSource {
            kind: SizingStateEvidenceKind::LossSnapshot,
            source: loss_snapshot.source.clone(),
            observed_at_ns: loss_snapshot.observed_at_ns,
        });
    }
    sources
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use crate::bolt_v3_position_sizer::{PredictionMarketSizingSnapshot, ProductSizingSnapshot};

    use super::{
        NtDerivedSizingState, OrderLifecycleSizingSnapshot, PortfolioSizingSnapshot,
        ReservationLedgerSnapshot, SizingStateError, SizingStateEvidenceKind,
        validate_nt_derived_sizing_state,
    };

    #[test]
    fn sizing_state_missing_nt_snapshot_fails_closed() {
        let decision = validate_nt_derived_sizing_state(None, 1_000, 100)
            .expect_err("missing NT-derived sizing state must fail closed");

        assert_eq!(decision, SizingStateError::MissingNtState);
    }

    fn state() -> NtDerivedSizingState {
        NtDerivedSizingState {
            source: "nt_sizing_state".to_string(),
            observed_at_ns: 1_000,
            portfolio: PortfolioSizingSnapshot {
                source: "nt_portfolio_snapshot".to_string(),
                observed_at_ns: 1_000,
                venue_id: "polymarket-clob".to_string(),
                account_id: "account-1".to_string(),
                collateral_currency: "PUSD".to_string(),
                free_collateral: Decimal::new(100, 0),
                total_equity: Decimal::new(100, 0),
            },
            order_lifecycle: OrderLifecycleSizingSnapshot {
                source: "nt_open_order_cache".to_string(),
                observed_at_ns: 1_000,
                open_order_count: 1,
                all_open_orders_attributed: true,
            },
            product_state: ProductSizingSnapshot::PredictionMarketBinary(
                PredictionMarketSizingSnapshot {
                    source: "nt_prediction_market_snapshot".to_string(),
                    observed_at_ns: 1_000,
                    yes_position: Decimal::new(10, 0),
                    no_position: Decimal::ZERO,
                    pusd_allowance: Decimal::new(100, 0),
                    conditional_token_allowance: Decimal::new(10, 0),
                    collateral_coupled_group_id: "group-1".to_string(),
                },
            ),
            reservation_snapshot: ReservationLedgerSnapshot {
                source: "bolt_reservation_ledger".to_string(),
                observed_at_ns: 1_000,
                all_live_reservations_attributed: true,
            },
            loss_snapshot: None,
        }
    }

    #[test]
    fn unattributed_state_transition_fails_closed() {
        let cases = [
            {
                let mut candidate = state();
                candidate.portfolio.source = " ".to_string();
                (candidate, SizingStateEvidenceKind::Portfolio)
            },
            {
                let mut candidate = state();
                candidate.order_lifecycle.all_open_orders_attributed = false;
                (candidate, SizingStateEvidenceKind::OrderLifecycle)
            },
            {
                let mut candidate = state();
                let ProductSizingSnapshot::PredictionMarketBinary(snapshot) =
                    &mut candidate.product_state;
                snapshot.source.clear();
                (candidate, SizingStateEvidenceKind::ProductState)
            },
            {
                let mut candidate = state();
                candidate
                    .reservation_snapshot
                    .all_live_reservations_attributed = false;
                (candidate, SizingStateEvidenceKind::ReservationLedger)
            },
        ];

        for (candidate, expected_kind) in cases {
            let decision = validate_nt_derived_sizing_state(Some(&candidate), 1_000, 100)
                .expect_err("unattributed NT-derived state must fail closed");

            assert_eq!(decision, SizingStateError::UnattributedState(expected_kind));
        }
    }
}
