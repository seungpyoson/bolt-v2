use rust_decimal::Decimal;

use crate::bolt_v3_capital_admission::ProductAdmissionSnapshot;
use crate::bolt_v3_loss_governor::LossSnapshot;

pub const POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE: &str =
    "polymarket_provider_collateral_allowance_rest";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapitalAdmissionStateError {
    MissingNtState,
    StaleNtState(CapitalAdmissionStateEvidenceKind),
    UnattributedState(CapitalAdmissionStateEvidenceKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapitalAdmissionStateEvidenceKind {
    State,
    Portfolio,
    ProviderCollateralAllowance,
    OrderLifecycle,
    ProductState,
    ReservationLedger,
    LossSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtDerivedCapitalAdmissionState {
    pub source: String,
    pub observed_at_ns: u64,
    pub portfolio: PortfolioCapitalAdmissionSnapshot,
    pub provider_collateral_allowance: ProviderCollateralAllowanceSnapshot,
    pub order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot,
    pub product_state: ProductAdmissionSnapshot,
    pub reservation_snapshot: ReservationLedgerSnapshot,
    pub loss_snapshot: Option<LossSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortfolioCapitalAdmissionSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub venue_id: String,
    pub account_id: String,
    pub collateral_currency: String,
    pub free_collateral: Decimal,
    pub total_equity: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCollateralAllowanceSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub venue_id: String,
    pub account_id: String,
    pub collateral_currency: String,
    pub collateral_allowance: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLifecycleCapitalAdmissionSnapshot {
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
pub struct CapitalAdmissionStateEvidenceSource {
    pub kind: CapitalAdmissionStateEvidenceKind,
    pub source: String,
    pub observed_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionStateEvidence {
    pub sources: Vec<CapitalAdmissionStateEvidenceSource>,
}

pub fn validate_nt_derived_capital_admission_state(
    state: Option<&NtDerivedCapitalAdmissionState>,
    now_ns: u64,
    max_snapshot_age_ns: u64,
) -> Result<CapitalAdmissionStateEvidence, CapitalAdmissionStateError> {
    let Some(state) = state else {
        return Err(CapitalAdmissionStateError::MissingNtState);
    };

    validate_source(&state.source, CapitalAdmissionStateEvidenceKind::State)?;
    validate_freshness(
        state.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::State,
    )?;
    validate_source(
        &state.portfolio.source,
        CapitalAdmissionStateEvidenceKind::Portfolio,
    )?;
    validate_freshness(
        state.portfolio.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::Portfolio,
    )?;
    validate_source(
        &state.provider_collateral_allowance.source,
        CapitalAdmissionStateEvidenceKind::ProviderCollateralAllowance,
    )?;
    validate_freshness(
        state.provider_collateral_allowance.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::ProviderCollateralAllowance,
    )?;
    validate_source(
        &state.order_lifecycle.source,
        CapitalAdmissionStateEvidenceKind::OrderLifecycle,
    )?;
    validate_freshness(
        state.order_lifecycle.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::OrderLifecycle,
    )?;
    if !state.order_lifecycle.all_open_orders_attributed {
        return Err(CapitalAdmissionStateError::UnattributedState(
            CapitalAdmissionStateEvidenceKind::OrderLifecycle,
        ));
    }

    let (product_source, product_observed_at_ns) = match &state.product_state {
        ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) => {
            (snapshot.source.as_str(), snapshot.observed_at_ns)
        }
    };
    validate_source(
        product_source,
        CapitalAdmissionStateEvidenceKind::ProductState,
    )?;
    validate_freshness(
        product_observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::ProductState,
    )?;

    validate_source(
        &state.reservation_snapshot.source,
        CapitalAdmissionStateEvidenceKind::ReservationLedger,
    )?;
    validate_freshness(
        state.reservation_snapshot.observed_at_ns,
        now_ns,
        max_snapshot_age_ns,
        CapitalAdmissionStateEvidenceKind::ReservationLedger,
    )?;
    if !state.reservation_snapshot.all_live_reservations_attributed {
        return Err(CapitalAdmissionStateError::UnattributedState(
            CapitalAdmissionStateEvidenceKind::ReservationLedger,
        ));
    }

    if let Some(loss_snapshot) = &state.loss_snapshot {
        if loss_snapshot.source.is_none() {
            return Err(CapitalAdmissionStateError::UnattributedState(
                CapitalAdmissionStateEvidenceKind::LossSnapshot,
            ));
        }
        validate_freshness(
            loss_snapshot.observed_at_ns,
            now_ns,
            max_snapshot_age_ns,
            CapitalAdmissionStateEvidenceKind::LossSnapshot,
        )?;
    }

    Ok(CapitalAdmissionStateEvidence {
        sources: evidence_sources(state),
    })
}

fn validate_source(
    source: &str,
    kind: CapitalAdmissionStateEvidenceKind,
) -> Result<(), CapitalAdmissionStateError> {
    if source.trim().is_empty() {
        return Err(CapitalAdmissionStateError::UnattributedState(kind));
    }
    Ok(())
}

fn validate_freshness(
    observed_at_ns: u64,
    now_ns: u64,
    max_snapshot_age_ns: u64,
    kind: CapitalAdmissionStateEvidenceKind,
) -> Result<(), CapitalAdmissionStateError> {
    if observed_at_ns > now_ns || now_ns - observed_at_ns > max_snapshot_age_ns {
        return Err(CapitalAdmissionStateError::StaleNtState(kind));
    }
    Ok(())
}

fn evidence_sources(
    state: &NtDerivedCapitalAdmissionState,
) -> Vec<CapitalAdmissionStateEvidenceSource> {
    let (product_source, product_observed_at_ns) = match &state.product_state {
        ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) => {
            (snapshot.source.clone(), snapshot.observed_at_ns)
        }
    };
    let mut sources = vec![
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::State,
            source: state.source.clone(),
            observed_at_ns: state.observed_at_ns,
        },
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::Portfolio,
            source: state.portfolio.source.clone(),
            observed_at_ns: state.portfolio.observed_at_ns,
        },
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::ProviderCollateralAllowance,
            source: state.provider_collateral_allowance.source.clone(),
            observed_at_ns: state.provider_collateral_allowance.observed_at_ns,
        },
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::OrderLifecycle,
            source: state.order_lifecycle.source.clone(),
            observed_at_ns: state.order_lifecycle.observed_at_ns,
        },
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::ProductState,
            source: product_source,
            observed_at_ns: product_observed_at_ns,
        },
        CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::ReservationLedger,
            source: state.reservation_snapshot.source.clone(),
            observed_at_ns: state.reservation_snapshot.observed_at_ns,
        },
    ];
    if let Some(loss_snapshot) = &state.loss_snapshot {
        sources.push(CapitalAdmissionStateEvidenceSource {
            kind: CapitalAdmissionStateEvidenceKind::LossSnapshot,
            source: loss_snapshot
                .source
                .expect("validated loss snapshot source must exist")
                .as_str()
                .to_string(),
            observed_at_ns: loss_snapshot.observed_at_ns,
        });
    }
    sources
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use crate::bolt_v3_capital_admission::{
        PredictionMarketAdmissionSnapshot, ProductAdmissionSnapshot,
    };
    use crate::bolt_v3_loss_governor::{
        LossSnapshot, LossSnapshotSource, LossSourceObservationTimestamps,
    };

    use super::{
        CapitalAdmissionStateError, CapitalAdmissionStateEvidenceKind,
        NtDerivedCapitalAdmissionState, OrderLifecycleCapitalAdmissionSnapshot,
        PortfolioCapitalAdmissionSnapshot, ProviderCollateralAllowanceSnapshot,
        ReservationLedgerSnapshot, validate_nt_derived_capital_admission_state,
    };

    #[test]
    fn capital_admission_state_missing_nt_snapshot_fails_closed() {
        let decision = validate_nt_derived_capital_admission_state(None, 1_000, 100)
            .expect_err("missing NT-derived sizing state must fail closed");

        assert_eq!(decision, CapitalAdmissionStateError::MissingNtState);
    }

    fn state() -> NtDerivedCapitalAdmissionState {
        NtDerivedCapitalAdmissionState {
            source: "nt_capital_admission_state".to_string(),
            observed_at_ns: 1_000,
            portfolio: PortfolioCapitalAdmissionSnapshot {
                source: "nt_portfolio_snapshot".to_string(),
                observed_at_ns: 1_000,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-A".to_string(),
                collateral_currency: "USD".to_string(),
                free_collateral: Decimal::new(100, 0),
                total_equity: Decimal::new(100, 0),
            },
            provider_collateral_allowance: ProviderCollateralAllowanceSnapshot {
                source: "operator-venue-allowance".to_string(),
                observed_at_ns: 1_000,
                venue_id: "VENUE-A".to_string(),
                account_id: "ACCOUNT-A".to_string(),
                collateral_currency: "USD".to_string(),
                collateral_allowance: Decimal::new(100, 0),
            },
            order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
                source: "nt_open_order_cache".to_string(),
                observed_at_ns: 1_000,
                open_order_count: 1,
                all_open_orders_attributed: true,
            },
            product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
                PredictionMarketAdmissionSnapshot {
                    source: "nt_prediction_market_snapshot".to_string(),
                    observed_at_ns: 1_000,
                    yes_instrument_id: "instrument-1".to_string(),
                    no_instrument_id: "instrument-1-no".to_string(),
                    yes_position: Decimal::new(10, 0),
                    no_position: Decimal::ZERO,
                    collateral_allowance: Decimal::new(100, 0),
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

    fn loss_snapshot() -> LossSnapshot {
        LossSnapshot {
            source: Some(LossSnapshotSource::BoltLossSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::ZERO),
            daily_pnl: Some(Decimal::ZERO),
            rolling_pnl: Some(Decimal::ZERO),
            current_equity: Some(Decimal::new(100, 0)),
            peak_equity: Some(Decimal::new(100, 0)),
            source_observations: LossSourceObservationTimestamps::unobserved(),
        }
    }

    fn state_with_observed_at(
        kind: CapitalAdmissionStateEvidenceKind,
        observed_at_ns: u64,
    ) -> NtDerivedCapitalAdmissionState {
        let mut candidate = state();
        match kind {
            CapitalAdmissionStateEvidenceKind::State => candidate.observed_at_ns = observed_at_ns,
            CapitalAdmissionStateEvidenceKind::Portfolio => {
                candidate.portfolio.observed_at_ns = observed_at_ns;
            }
            CapitalAdmissionStateEvidenceKind::ProviderCollateralAllowance => {
                candidate.provider_collateral_allowance.observed_at_ns = observed_at_ns;
            }
            CapitalAdmissionStateEvidenceKind::OrderLifecycle => {
                candidate.order_lifecycle.observed_at_ns = observed_at_ns;
            }
            CapitalAdmissionStateEvidenceKind::ProductState => {
                let ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) =
                    &mut candidate.product_state;
                snapshot.observed_at_ns = observed_at_ns;
            }
            CapitalAdmissionStateEvidenceKind::ReservationLedger => {
                candidate.reservation_snapshot.observed_at_ns = observed_at_ns;
            }
            CapitalAdmissionStateEvidenceKind::LossSnapshot => {
                let mut snapshot = loss_snapshot();
                snapshot.observed_at_ns = observed_at_ns;
                candidate.loss_snapshot = Some(snapshot);
            }
        }
        candidate
    }

    #[test]
    fn capital_admission_state_valid_snapshot_returns_expected_evidence_sources() {
        let evidence = validate_nt_derived_capital_admission_state(Some(&state()), 1_000, 100)
            .expect("fresh attributed NT-derived sizing state should be accepted");

        let kinds = evidence
            .sources
            .iter()
            .map(|source| source.kind)
            .collect::<Vec<_>>();

        assert_eq!(
            kinds,
            vec![
                CapitalAdmissionStateEvidenceKind::State,
                CapitalAdmissionStateEvidenceKind::Portfolio,
                CapitalAdmissionStateEvidenceKind::ProviderCollateralAllowance,
                CapitalAdmissionStateEvidenceKind::OrderLifecycle,
                CapitalAdmissionStateEvidenceKind::ProductState,
                CapitalAdmissionStateEvidenceKind::ReservationLedger,
            ]
        );
    }

    #[test]
    fn capital_admission_state_valid_loss_snapshot_is_included_in_evidence() {
        let mut candidate = state();
        candidate.loss_snapshot = Some(loss_snapshot());

        let evidence = validate_nt_derived_capital_admission_state(Some(&candidate), 1_000, 100)
            .expect("fresh attributed loss snapshot should be accepted");

        assert_eq!(evidence.sources.len(), 7);
        assert_eq!(
            evidence.sources.last().map(|source| source.kind),
            Some(CapitalAdmissionStateEvidenceKind::LossSnapshot)
        );
    }

    #[test]
    fn unattributed_state_transition_fails_closed() {
        let cases = [
            {
                let mut candidate = state();
                candidate.source = " ".to_string();
                (candidate, CapitalAdmissionStateEvidenceKind::State)
            },
            {
                let mut candidate = state();
                candidate.portfolio.source = " ".to_string();
                (candidate, CapitalAdmissionStateEvidenceKind::Portfolio)
            },
            {
                let mut candidate = state();
                candidate.provider_collateral_allowance.source = " ".to_string();
                (
                    candidate,
                    CapitalAdmissionStateEvidenceKind::ProviderCollateralAllowance,
                )
            },
            {
                let mut candidate = state();
                candidate.order_lifecycle.all_open_orders_attributed = false;
                (candidate, CapitalAdmissionStateEvidenceKind::OrderLifecycle)
            },
            {
                let mut candidate = state();
                let ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) =
                    &mut candidate.product_state;
                snapshot.source.clear();
                (candidate, CapitalAdmissionStateEvidenceKind::ProductState)
            },
            {
                let mut candidate = state();
                candidate
                    .reservation_snapshot
                    .all_live_reservations_attributed = false;
                (
                    candidate,
                    CapitalAdmissionStateEvidenceKind::ReservationLedger,
                )
            },
            {
                let mut candidate = state();
                let mut snapshot = loss_snapshot();
                snapshot.source = None;
                candidate.loss_snapshot = Some(snapshot);
                (candidate, CapitalAdmissionStateEvidenceKind::LossSnapshot)
            },
        ];

        for (candidate, expected_kind) in cases {
            let decision =
                validate_nt_derived_capital_admission_state(Some(&candidate), 1_000, 100)
                    .expect_err("unattributed NT-derived state must fail closed");

            assert_eq!(
                decision,
                CapitalAdmissionStateError::UnattributedState(expected_kind)
            );
        }
    }

    #[test]
    fn stale_capital_admission_state_evidence_fails_closed_for_each_kind() {
        let kinds = [
            CapitalAdmissionStateEvidenceKind::State,
            CapitalAdmissionStateEvidenceKind::Portfolio,
            CapitalAdmissionStateEvidenceKind::ProviderCollateralAllowance,
            CapitalAdmissionStateEvidenceKind::OrderLifecycle,
            CapitalAdmissionStateEvidenceKind::ProductState,
            CapitalAdmissionStateEvidenceKind::ReservationLedger,
            CapitalAdmissionStateEvidenceKind::LossSnapshot,
        ];

        for kind in kinds {
            let stale = state_with_observed_at(kind, 899);
            let decision = validate_nt_derived_capital_admission_state(Some(&stale), 1_000, 100)
                .expect_err("stale NT-derived sizing state must fail closed");

            assert_eq!(decision, CapitalAdmissionStateError::StaleNtState(kind));
        }
    }

    #[test]
    fn future_capital_admission_state_evidence_fails_closed_for_each_kind() {
        let kinds = [
            CapitalAdmissionStateEvidenceKind::State,
            CapitalAdmissionStateEvidenceKind::Portfolio,
            CapitalAdmissionStateEvidenceKind::ProviderCollateralAllowance,
            CapitalAdmissionStateEvidenceKind::OrderLifecycle,
            CapitalAdmissionStateEvidenceKind::ProductState,
            CapitalAdmissionStateEvidenceKind::ReservationLedger,
            CapitalAdmissionStateEvidenceKind::LossSnapshot,
        ];

        for kind in kinds {
            let future = state_with_observed_at(kind, 1_001);
            let decision = validate_nt_derived_capital_admission_state(Some(&future), 1_000, 100)
                .expect_err("future NT-derived sizing state must fail closed");

            assert_eq!(decision, CapitalAdmissionStateError::StaleNtState(kind));
        }
    }
}
