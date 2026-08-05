//! Product-agnostic reservation ledger for committed-but-unfilled capital.
//!
//! New reservations require request evidence no older than the pool snapshot so submit admission
//! cannot spend against stale account state. Terminal release and residual revalue events may arrive
//! from an async order stream that lags a newer pool snapshot; those paths still enforce each event's
//! freshness window and monotonic lifecycle timestamp against the stored reservation.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalPoolSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub pool_id: String,
    pub max_pool_liability: Decimal,
    pub committed_liability: Decimal,
    pub max_snapshot_age_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRequest {
    pub request_id: String,
    pub pool_id: String,
    pub collateral_group_id: String,
    pub liability: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRevalueRequest {
    pub request_id: String,
    pub pool_id: String,
    pub collateral_group_id: String,
    pub liability: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationReleaseRequest {
    pub request_id: String,
    pub pool_id: String,
    pub collateral_group_id: String,
    pub observed_at_ns: u64,
    pub evidence_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationRejectionReason {
    MissingEvidence,
    StaleRequest,
    PoolMismatch,
    OverBudget,
    InvalidRequest,
    CollateralGroupMismatch,
    DuplicateReservation,
    UnknownReservation,
    UnknownRelease,
    ReconciliationRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationDecision {
    pub accepted: bool,
    pub reason: Option<ReservationRejectionReason>,
    pub requested_liability: Decimal,
    pub available_before: Decimal,
    pub available_after: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationReleaseDecision {
    pub accepted: bool,
    pub reason: Option<ReservationRejectionReason>,
    pub released_liability: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRevalueDecision {
    pub accepted: bool,
    pub reason: Option<ReservationRejectionReason>,
    pub previous_liability: Option<Decimal>,
    pub revalued_liability: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationLedger {
    live_reservations: Vec<LiveReservation>,
    reconciliation_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveReservation {
    request_id: String,
    pool_id: String,
    collateral_group_id: String,
    liability: Decimal,
    observed_at_ns: u64,
}

impl ReservationLedger {
    pub fn unreconciled() -> Self {
        Self {
            live_reservations: Vec::new(),
            reconciliation_complete: false,
        }
    }

    pub fn reconciled() -> Self {
        Self {
            live_reservations: Vec::new(),
            reconciliation_complete: true,
        }
    }

    pub fn is_reconciled(&self) -> bool {
        self.reconciliation_complete
    }

    pub fn invalidate_reconciliation(&mut self) {
        self.reconciliation_complete = false;
    }

    pub fn reserve(
        &mut self,
        pool: &CapitalPoolSnapshot,
        request: &ReservationRequest,
        now_ns: u64,
        min_remaining_pool_balance: Option<Decimal>,
    ) -> ReservationDecision {
        let Some(available_before) =
            available_pool_liability(pool, self.live_reserved_liability(&pool.pool_id))
        else {
            return rejected(
                ReservationRejectionReason::OverBudget,
                request.liability,
                Decimal::ZERO,
            );
        };

        if !self.reconciliation_complete {
            return rejected(
                ReservationRejectionReason::ReconciliationRequired,
                request.liability,
                available_before,
            );
        }
        if pool.pool_id != request.pool_id {
            return rejected(
                ReservationRejectionReason::PoolMismatch,
                request.liability,
                available_before,
            );
        }
        if missing_identity(request) || request.liability <= Decimal::ZERO {
            return rejected(
                ReservationRejectionReason::InvalidRequest,
                request.liability,
                available_before,
            );
        }
        if pool.source.trim().is_empty() || request.evidence_label.trim().is_empty() {
            return rejected(
                ReservationRejectionReason::MissingEvidence,
                request.liability,
                available_before,
            );
        }
        if stale(pool.observed_at_ns, now_ns, pool.max_snapshot_age_ns)
            || stale(request.observed_at_ns, now_ns, pool.max_snapshot_age_ns)
            || request.observed_at_ns < pool.observed_at_ns
        {
            return rejected(
                ReservationRejectionReason::StaleRequest,
                request.liability,
                available_before,
            );
        }
        if self.live_reservations.iter().any(|reservation| {
            reservation.pool_id == request.pool_id && reservation.request_id == request.request_id
        }) {
            return rejected(
                ReservationRejectionReason::DuplicateReservation,
                request.liability,
                available_before,
            );
        }
        if request.liability > available_before {
            return rejected(
                ReservationRejectionReason::OverBudget,
                request.liability,
                available_before,
            );
        }
        let Some(available_after) = available_before.checked_sub(request.liability) else {
            return rejected(
                ReservationRejectionReason::OverBudget,
                request.liability,
                available_before,
            );
        };
        if let Some(min_remaining_pool_balance) = min_remaining_pool_balance
            && available_after < min_remaining_pool_balance
        {
            return rejected(
                ReservationRejectionReason::OverBudget,
                request.liability,
                available_before,
            );
        }

        self.live_reservations.push(LiveReservation {
            request_id: request.request_id.clone(),
            pool_id: request.pool_id.clone(),
            collateral_group_id: request.collateral_group_id.clone(),
            liability: request.liability,
            observed_at_ns: request.observed_at_ns,
        });

        ReservationDecision {
            accepted: true,
            reason: None,
            requested_liability: request.liability,
            available_before,
            available_after: Some(available_after),
        }
    }

    pub fn live_reserved_liability(&self, pool_id: &str) -> Decimal {
        self.live_reservations
            .iter()
            .filter(|reservation| reservation.pool_id == pool_id)
            .try_fold(Decimal::ZERO, |sum, reservation| {
                sum.checked_add(reservation.liability)
            })
            .unwrap_or(Decimal::MAX)
    }

    pub fn rollback_uncommitted(&mut self, pool_id: &str, request_id: &str) -> Option<Decimal> {
        if !self.reconciliation_complete {
            return None;
        }
        let index = self.live_reservations.iter().position(|reservation| {
            reservation.pool_id == pool_id && reservation.request_id == request_id
        })?;
        Some(self.live_reservations.remove(index).liability)
    }

    pub fn release(
        &mut self,
        pool: &CapitalPoolSnapshot,
        request: &ReservationReleaseRequest,
        now_ns: u64,
    ) -> ReservationReleaseDecision {
        if !self.reconciliation_complete {
            return rejected_release(ReservationRejectionReason::ReconciliationRequired);
        }
        if pool.pool_id != request.pool_id {
            return rejected_release(ReservationRejectionReason::PoolMismatch);
        }
        if request.request_id.trim().is_empty()
            || request.pool_id.trim().is_empty()
            || request.collateral_group_id.trim().is_empty()
        {
            return rejected_release(ReservationRejectionReason::InvalidRequest);
        }
        if pool.source.trim().is_empty() || request.evidence_label.trim().is_empty() {
            return rejected_release(ReservationRejectionReason::MissingEvidence);
        }
        if stale(pool.observed_at_ns, now_ns, pool.max_snapshot_age_ns)
            || stale(request.observed_at_ns, now_ns, pool.max_snapshot_age_ns)
        {
            return rejected_release(ReservationRejectionReason::StaleRequest);
        }
        let Some(index) = self.live_reservations.iter().position(|reservation| {
            reservation.pool_id == request.pool_id && reservation.request_id == request.request_id
        }) else {
            return rejected_release(ReservationRejectionReason::UnknownRelease);
        };
        if self.live_reservations[index].pool_id != pool.pool_id {
            return rejected_release(ReservationRejectionReason::PoolMismatch);
        }
        if self.live_reservations[index].collateral_group_id != request.collateral_group_id {
            return rejected_release(ReservationRejectionReason::CollateralGroupMismatch);
        }
        if request.observed_at_ns <= self.live_reservations[index].observed_at_ns {
            return rejected_release(ReservationRejectionReason::StaleRequest);
        }
        let reservation = self.live_reservations.remove(index);

        ReservationReleaseDecision {
            accepted: true,
            reason: None,
            released_liability: Some(reservation.liability),
        }
    }

    pub fn revalue(
        &mut self,
        pool: &CapitalPoolSnapshot,
        request: &ReservationRevalueRequest,
        now_ns: u64,
        min_remaining_pool_balance: Option<Decimal>,
    ) -> ReservationRevalueDecision {
        if !self.reconciliation_complete {
            return rejected_revalue(ReservationRejectionReason::ReconciliationRequired);
        }
        if pool.pool_id != request.pool_id {
            return rejected_revalue(ReservationRejectionReason::PoolMismatch);
        }
        // Terminal zero-liability order events release reservations; revalue tracks live residual liability.
        if request.request_id.trim().is_empty()
            || request.pool_id.trim().is_empty()
            || request.collateral_group_id.trim().is_empty()
            || request.liability <= Decimal::ZERO
        {
            return rejected_revalue(ReservationRejectionReason::InvalidRequest);
        }
        if pool.source.trim().is_empty() || request.evidence_label.trim().is_empty() {
            return rejected_revalue(ReservationRejectionReason::MissingEvidence);
        }
        if stale(pool.observed_at_ns, now_ns, pool.max_snapshot_age_ns)
            || stale(request.observed_at_ns, now_ns, pool.max_snapshot_age_ns)
        {
            return rejected_revalue(ReservationRejectionReason::StaleRequest);
        }
        let Some(index) = self.live_reservations.iter().position(|reservation| {
            reservation.pool_id == request.pool_id && reservation.request_id == request.request_id
        }) else {
            return rejected_revalue(ReservationRejectionReason::UnknownReservation);
        };
        if self.live_reservations[index].pool_id != pool.pool_id {
            return rejected_revalue(ReservationRejectionReason::PoolMismatch);
        }
        if self.live_reservations[index].collateral_group_id != request.collateral_group_id {
            return rejected_revalue(ReservationRejectionReason::CollateralGroupMismatch);
        }
        if request.observed_at_ns <= self.live_reservations[index].observed_at_ns {
            return rejected_revalue(ReservationRejectionReason::StaleRequest);
        }
        let previous_liability = self.live_reservations[index].liability;
        let Some(available_before) =
            available_pool_liability(pool, self.live_reserved_liability(&pool.pool_id))
                .and_then(|available| available.checked_add(previous_liability))
        else {
            return rejected_revalue(ReservationRejectionReason::OverBudget);
        };
        if request.liability > available_before {
            return rejected_revalue(ReservationRejectionReason::OverBudget);
        }
        let Some(available_after) = available_before.checked_sub(request.liability) else {
            return rejected_revalue(ReservationRejectionReason::OverBudget);
        };
        if let Some(min_remaining_pool_balance) = min_remaining_pool_balance
            && available_after < min_remaining_pool_balance
        {
            return rejected_revalue(ReservationRejectionReason::OverBudget);
        }

        self.live_reservations[index].liability = request.liability;
        self.live_reservations[index].observed_at_ns = request.observed_at_ns;

        ReservationRevalueDecision {
            accepted: true,
            reason: None,
            previous_liability: Some(previous_liability),
            revalued_liability: Some(request.liability),
        }
    }
}

fn rejected(
    reason: ReservationRejectionReason,
    requested_liability: Decimal,
    available_before: Decimal,
) -> ReservationDecision {
    ReservationDecision {
        accepted: false,
        reason: Some(reason),
        requested_liability,
        available_before,
        available_after: None,
    }
}

fn missing_identity(request: &ReservationRequest) -> bool {
    request.request_id.trim().is_empty()
        || request.pool_id.trim().is_empty()
        || request.collateral_group_id.trim().is_empty()
}

fn rejected_release(reason: ReservationRejectionReason) -> ReservationReleaseDecision {
    ReservationReleaseDecision {
        accepted: false,
        reason: Some(reason),
        released_liability: None,
    }
}

fn rejected_revalue(reason: ReservationRejectionReason) -> ReservationRevalueDecision {
    ReservationRevalueDecision {
        accepted: false,
        reason: Some(reason),
        previous_liability: None,
        revalued_liability: None,
    }
}

fn stale(observed_at_ns: u64, now_ns: u64, max_snapshot_age_ns: u64) -> bool {
    observed_at_ns > now_ns || now_ns - observed_at_ns > max_snapshot_age_ns
}

fn available_pool_liability(
    pool: &CapitalPoolSnapshot,
    live_reserved_liability: Decimal,
) -> Option<Decimal> {
    pool.max_pool_liability
        .checked_sub(pool.committed_liability)?
        .checked_sub(live_reserved_liability)
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::{
        CapitalPoolSnapshot, LiveReservation, ReservationLedger, ReservationReleaseRequest,
        ReservationRequest, ReservationRevalueRequest,
    };

    fn pool() -> CapitalPoolSnapshot {
        CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        }
    }

    fn reservation_request(request_id: &str) -> ReservationRequest {
        ReservationRequest {
            request_id: request_id.to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(40, 0),
            observed_at_ns: 1_010,
            evidence_label: "nt-account-and-allowance-snapshot".to_string(),
        }
    }

    fn release_request(request_id: &str, observed_at_ns: u64) -> ReservationReleaseRequest {
        ReservationReleaseRequest {
            request_id: request_id.to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            observed_at_ns,
            evidence_label: "nt-order-terminal".to_string(),
        }
    }

    fn revalue_request(
        request_id: &str,
        liability: Decimal,
        observed_at_ns: u64,
    ) -> ReservationRevalueRequest {
        ReservationRevalueRequest {
            request_id: request_id.to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability,
            observed_at_ns,
            evidence_label: "nt-order-live-residual".to_string(),
        }
    }

    #[test]
    fn fitting_reservation_is_accepted_and_recorded_with_remaining_budget_evidence() {
        let pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::new(25, 0),
            max_snapshot_age_ns: 100,
        };
        let request = ReservationRequest {
            request_id: "request-1".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(40, 0),
            observed_at_ns: 1_010,
            evidence_label: "nt-account-and-allowance-snapshot".to_string(),
        };

        let mut ledger = ReservationLedger::reconciled();
        let decision = ledger.reserve(&pool, &request, 1_020, None);

        assert!(decision.accepted);
        assert_eq!(decision.reason, None);
        assert_eq!(decision.requested_liability, Decimal::new(40, 0));
        assert_eq!(decision.available_before, Decimal::new(75, 0));
        assert_eq!(decision.available_after, Some(Decimal::new(35, 0)));
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(40, 0)
        );
    }

    #[test]
    fn reserve_rejects_fail_closed_contract_violations_without_mutating() {
        let base_pool = pool();
        let base_request = reservation_request("request-reserve-contract");
        let cases = [
            (
                "unreconciled",
                false,
                base_pool.clone(),
                base_request.clone(),
                1_020,
                super::ReservationRejectionReason::ReconciliationRequired,
            ),
            (
                "pool mismatch",
                true,
                base_pool.clone(),
                ReservationRequest {
                    pool_id: "other-capital-pool".to_string(),
                    ..base_request.clone()
                },
                1_020,
                super::ReservationRejectionReason::PoolMismatch,
            ),
            (
                "invalid empty identity",
                true,
                base_pool.clone(),
                ReservationRequest {
                    request_id: " ".to_string(),
                    ..base_request.clone()
                },
                1_020,
                super::ReservationRejectionReason::InvalidRequest,
            ),
            (
                "invalid non-positive liability",
                true,
                base_pool.clone(),
                ReservationRequest {
                    liability: Decimal::ZERO,
                    ..base_request.clone()
                },
                1_020,
                super::ReservationRejectionReason::InvalidRequest,
            ),
            (
                "missing pool source",
                true,
                CapitalPoolSnapshot {
                    source: " ".to_string(),
                    ..base_pool.clone()
                },
                base_request.clone(),
                1_020,
                super::ReservationRejectionReason::MissingEvidence,
            ),
            (
                "missing request evidence",
                true,
                base_pool.clone(),
                ReservationRequest {
                    evidence_label: " ".to_string(),
                    ..base_request.clone()
                },
                1_020,
                super::ReservationRejectionReason::MissingEvidence,
            ),
            (
                "pool snapshot freshness bound",
                true,
                CapitalPoolSnapshot {
                    max_snapshot_age_ns: 5,
                    ..base_pool.clone()
                },
                base_request.clone(),
                1_020,
                super::ReservationRejectionReason::StaleRequest,
            ),
            (
                "request older than pool",
                true,
                base_pool.clone(),
                ReservationRequest {
                    observed_at_ns: 999,
                    ..base_request.clone()
                },
                1_020,
                super::ReservationRejectionReason::StaleRequest,
            ),
            (
                "direct over budget",
                true,
                CapitalPoolSnapshot {
                    committed_liability: Decimal::new(90, 0),
                    ..base_pool.clone()
                },
                ReservationRequest {
                    liability: Decimal::new(20, 0),
                    ..base_request
                },
                1_020,
                super::ReservationRejectionReason::OverBudget,
            ),
        ];

        for (case_name, reconciled, pool, request, now_ns, expected_reason) in cases {
            let mut ledger = if reconciled {
                ReservationLedger::reconciled()
            } else {
                ReservationLedger::unreconciled()
            };

            let decision = ledger.reserve(&pool, &request, now_ns, None);

            assert!(!decision.accepted, "{case_name}");
            assert_eq!(decision.reason, Some(expected_reason), "{case_name}");
            assert_eq!(
                ledger.live_reserved_liability(&pool.pool_id),
                Decimal::ZERO,
                "{case_name}"
            );
        }
    }

    #[test]
    fn reserve_rejects_duplicate_reservation_without_mutating() {
        let pool = pool();
        let request = reservation_request("request-duplicate");
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);

        let duplicate = ledger.reserve(&pool, &request, 1_030, None);

        assert!(!duplicate.accepted);
        assert_eq!(
            duplicate.reason,
            Some(super::ReservationRejectionReason::DuplicateReservation)
        );
        assert_eq!(
            ledger.live_reserved_liability(&pool.pool_id),
            Decimal::new(40, 0)
        );
    }

    #[test]
    fn same_request_id_can_reserve_in_different_pool() {
        let first_pool = pool();
        let second_pool = CapitalPoolSnapshot {
            pool_id: "kalshi-live".to_string(),
            ..first_pool.clone()
        };
        let first_request = reservation_request("request-cross-pool");
        let second_request = ReservationRequest {
            pool_id: second_pool.pool_id.clone(),
            ..first_request.clone()
        };
        let mut ledger = ReservationLedger::reconciled();

        let first = ledger.reserve(&first_pool, &first_request, 1_020, None);
        let second = ledger.reserve(&second_pool, &second_request, 1_030, None);

        assert!(first.accepted);
        assert!(second.accepted);
        assert_eq!(
            ledger.live_reserved_liability(&first_pool.pool_id),
            Decimal::new(40, 0)
        );
        assert_eq!(
            ledger.live_reserved_liability(&second_pool.pool_id),
            Decimal::new(40, 0)
        );
    }

    #[test]
    fn rollback_uncommitted_removes_reservation_and_frees_capacity() {
        let pool = pool();
        let request = reservation_request("request-rollback");
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);

        let rolled_back = ledger.rollback_uncommitted(&pool.pool_id, &request.request_id);

        assert_eq!(rolled_back, Some(Decimal::new(40, 0)));
        assert_eq!(ledger.live_reserved_liability(&pool.pool_id), Decimal::ZERO);
        let replacement = ReservationRequest {
            request_id: "request-after-rollback".to_string(),
            liability: Decimal::new(100, 0),
            ..reservation_request("request-after-rollback")
        };
        assert!(ledger.reserve(&pool, &replacement, 1_030, None).accepted);
    }

    #[test]
    fn rollback_uncommitted_wrong_identity_does_not_mutate() {
        let pool = pool();
        let request = reservation_request("request-rollback-identity");
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);

        assert_eq!(
            ledger.rollback_uncommitted("other-capital-pool", &request.request_id),
            None
        );
        assert_eq!(
            ledger.rollback_uncommitted(&pool.pool_id, "other-request"),
            None
        );
        assert_eq!(
            ledger.live_reserved_liability(&pool.pool_id),
            Decimal::new(40, 0)
        );
    }

    #[test]
    fn rollback_uncommitted_rejects_unreconciled_ledger_without_mutating() {
        let mut ledger = ReservationLedger {
            live_reservations: vec![LiveReservation {
                request_id: "request-unreconciled-rollback".to_string(),
                pool_id: "capital-pool-1".to_string(),
                collateral_group_id: "collateral-group-1".to_string(),
                liability: Decimal::new(40, 0),
                observed_at_ns: 1_010,
            }],
            reconciliation_complete: false,
        };

        let rolled_back =
            ledger.rollback_uncommitted("capital-pool-1", "request-unreconciled-rollback");

        assert_eq!(rolled_back, None);
        assert_eq!(
            ledger.live_reserved_liability("capital-pool-1"),
            Decimal::new(40, 0)
        );
    }

    #[test]
    fn release_removes_live_reservation_and_duplicate_release_rejects() {
        let pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        };
        let request = ReservationRequest {
            request_id: "request-2".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(40, 0),
            observed_at_ns: 1_010,
            evidence_label: "nt-account-and-allowance-snapshot".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);

        let release_request = ReservationReleaseRequest {
            request_id: "request-2".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            observed_at_ns: 1_030,
            evidence_label: "nt-order-terminal".to_string(),
        };

        let release = ledger.release(&pool, &release_request, 1_040);

        assert!(release.accepted);
        assert_eq!(release.reason, None);
        assert_eq!(release.released_liability, Some(Decimal::new(40, 0)));
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::ZERO
        );

        let duplicate = ledger.release(&pool, &release_request, 1_040);

        assert!(!duplicate.accepted);
        assert_eq!(
            duplicate.reason,
            Some(super::ReservationRejectionReason::UnknownRelease)
        );
        assert_eq!(duplicate.released_liability, None);
    }

    #[test]
    fn release_rejects_unreconciled_ledger_before_unknown_release() {
        let pool = pool();
        let mut ledger = ReservationLedger::unreconciled();
        let release = ledger.release(&pool, &release_request("request-unknown", 1_030), 1_040);

        assert!(!release.accepted);
        assert_eq!(
            release.reason,
            Some(super::ReservationRejectionReason::ReconciliationRequired)
        );
        assert_eq!(release.released_liability, None);
    }

    #[test]
    fn release_rejects_contract_violations_without_mutating_live_reservation() {
        let base_pool = pool();
        let base_request = reservation_request("request-release-contract");
        let base_release = release_request("request-release-contract", 1_030);
        let cases = [
            (
                "request pool mismatch",
                base_pool.clone(),
                ReservationReleaseRequest {
                    pool_id: "other-capital-pool".to_string(),
                    ..base_release.clone()
                },
                1_040,
                super::ReservationRejectionReason::PoolMismatch,
            ),
            (
                "invalid request identity",
                base_pool.clone(),
                ReservationReleaseRequest {
                    request_id: " ".to_string(),
                    ..base_release.clone()
                },
                1_040,
                super::ReservationRejectionReason::InvalidRequest,
            ),
            (
                "missing pool source",
                CapitalPoolSnapshot {
                    source: " ".to_string(),
                    ..base_pool.clone()
                },
                base_release.clone(),
                1_040,
                super::ReservationRejectionReason::MissingEvidence,
            ),
            (
                "missing release evidence",
                base_pool.clone(),
                ReservationReleaseRequest {
                    evidence_label: " ".to_string(),
                    ..base_release.clone()
                },
                1_040,
                super::ReservationRejectionReason::MissingEvidence,
            ),
            (
                "stale pool snapshot",
                CapitalPoolSnapshot {
                    max_snapshot_age_ns: 5,
                    ..base_pool.clone()
                },
                base_release,
                1_040,
                super::ReservationRejectionReason::StaleRequest,
            ),
        ];

        for (case_name, pool, release_request, now_ns, expected_reason) in cases {
            let mut ledger = ReservationLedger::reconciled();
            assert!(
                ledger
                    .reserve(&base_pool, &base_request, 1_020, None)
                    .accepted,
                "{case_name}"
            );

            let release = ledger.release(&pool, &release_request, now_ns);

            assert!(!release.accepted, "{case_name}");
            assert_eq!(release.reason, Some(expected_reason), "{case_name}");
            assert_eq!(release.released_liability, None, "{case_name}");
            assert_eq!(
                ledger.live_reserved_liability(&base_pool.pool_id),
                Decimal::new(40, 0),
                "{case_name}"
            );
        }
    }

    #[test]
    fn stale_or_equal_timestamp_release_rejects_without_mutating_live_reservation() {
        let pool = pool();
        let request = reservation_request("request-stale-release");
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);

        let release = ledger.release(
            &pool,
            &release_request("request-stale-release", 1_010),
            1_040,
        );

        assert!(!release.accepted);
        assert_eq!(
            release.reason,
            Some(super::ReservationRejectionReason::StaleRequest)
        );
        assert_eq!(release.released_liability, None);
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(40, 0)
        );
    }

    #[test]
    fn release_accepts_terminal_evidence_older_than_newer_pool_snapshot() {
        let pool = pool();
        let request = reservation_request("request-async-release");
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);
        let newer_pool = CapitalPoolSnapshot {
            observed_at_ns: 1_060,
            ..pool
        };

        let release = ledger.release(
            &newer_pool,
            &release_request("request-async-release", 1_050),
            1_070,
        );

        assert!(release.accepted);
        assert_eq!(release.released_liability, Some(Decimal::new(40, 0)));
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::ZERO
        );
    }

    #[test]
    fn revalue_accepts_live_residual_evidence_older_than_newer_pool_snapshot() {
        let pool = pool();
        let request = reservation_request("request-async-revalue");
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);
        let newer_pool = CapitalPoolSnapshot {
            observed_at_ns: 1_060,
            ..pool
        };

        let revalue = ledger.revalue(
            &newer_pool,
            &revalue_request("request-async-revalue", Decimal::new(25, 0), 1_050),
            1_070,
            None,
        );

        assert!(revalue.accepted);
        assert_eq!(revalue.previous_liability, Some(Decimal::new(40, 0)));
        assert_eq!(revalue.revalued_liability, Some(Decimal::new(25, 0)));
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(25, 0)
        );
    }

    #[test]
    fn revalue_rejects_unreconciled_and_unknown_reservation_without_mutating() {
        let pool = pool();
        let request = revalue_request("request-unknown-revalue", Decimal::new(25, 0), 1_030);
        let mut unreconciled_ledger = ReservationLedger::unreconciled();
        let unreconciled = unreconciled_ledger.revalue(&pool, &request, 1_040, None);

        assert!(!unreconciled.accepted);
        assert_eq!(
            unreconciled.reason,
            Some(super::ReservationRejectionReason::ReconciliationRequired)
        );

        let mut reconciled_ledger = ReservationLedger::reconciled();
        let unknown = reconciled_ledger.revalue(&pool, &request, 1_040, None);

        assert!(!unknown.accepted);
        assert_eq!(
            unknown.reason,
            Some(super::ReservationRejectionReason::UnknownReservation)
        );
        assert_eq!(
            reconciled_ledger.live_reserved_liability(&pool.pool_id),
            Decimal::ZERO
        );
    }

    #[test]
    fn revalue_rejects_contract_violations_without_mutating_live_reservation() {
        let base_pool = pool();
        let base_request = reservation_request("request-revalue-contract");
        let base_revalue = revalue_request("request-revalue-contract", Decimal::new(25, 0), 1_030);
        let cases = [
            (
                "request pool mismatch",
                base_pool.clone(),
                ReservationRevalueRequest {
                    pool_id: "other-capital-pool".to_string(),
                    ..base_revalue.clone()
                },
                1_040,
                super::ReservationRejectionReason::PoolMismatch,
            ),
            (
                "invalid request identity",
                base_pool.clone(),
                ReservationRevalueRequest {
                    request_id: " ".to_string(),
                    ..base_revalue.clone()
                },
                1_040,
                super::ReservationRejectionReason::InvalidRequest,
            ),
            (
                "invalid non-positive liability",
                base_pool.clone(),
                ReservationRevalueRequest {
                    liability: Decimal::ZERO,
                    ..base_revalue.clone()
                },
                1_040,
                super::ReservationRejectionReason::InvalidRequest,
            ),
            (
                "missing pool source",
                CapitalPoolSnapshot {
                    source: " ".to_string(),
                    ..base_pool.clone()
                },
                base_revalue.clone(),
                1_040,
                super::ReservationRejectionReason::MissingEvidence,
            ),
            (
                "missing revalue evidence",
                base_pool.clone(),
                ReservationRevalueRequest {
                    evidence_label: " ".to_string(),
                    ..base_revalue.clone()
                },
                1_040,
                super::ReservationRejectionReason::MissingEvidence,
            ),
            (
                "stale pool snapshot",
                CapitalPoolSnapshot {
                    max_snapshot_age_ns: 5,
                    ..base_pool.clone()
                },
                base_revalue.clone(),
                1_040,
                super::ReservationRejectionReason::StaleRequest,
            ),
            (
                "future revalue evidence",
                base_pool.clone(),
                ReservationRevalueRequest {
                    observed_at_ns: 1_050,
                    ..base_revalue
                },
                1_040,
                super::ReservationRejectionReason::StaleRequest,
            ),
        ];

        for (case_name, pool, revalue_request, now_ns, expected_reason) in cases {
            let mut ledger = ReservationLedger::reconciled();
            assert!(
                ledger
                    .reserve(&base_pool, &base_request, 1_020, None)
                    .accepted,
                "{case_name}"
            );

            let revalue = ledger.revalue(&pool, &revalue_request, now_ns, None);

            assert!(!revalue.accepted, "{case_name}");
            assert_eq!(revalue.reason, Some(expected_reason), "{case_name}");
            assert_eq!(revalue.previous_liability, None, "{case_name}");
            assert_eq!(revalue.revalued_liability, None, "{case_name}");
            assert_eq!(
                ledger.live_reserved_liability(&base_pool.pool_id),
                Decimal::new(40, 0),
                "{case_name}"
            );
        }
    }

    #[test]
    fn release_rejects_collateral_group_mismatch_without_mutating_live_reservation() {
        let pool = pool();
        let request = reservation_request("request-collateral-release");
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);
        let release_request = ReservationReleaseRequest {
            collateral_group_id: "wrong-group".to_string(),
            ..release_request("request-collateral-release", 1_030)
        };

        let release = ledger.release(&pool, &release_request, 1_040);

        assert!(!release.accepted);
        assert_eq!(
            release.reason,
            Some(super::ReservationRejectionReason::CollateralGroupMismatch)
        );
        assert_eq!(release.released_liability, None);
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(40, 0)
        );
    }

    #[test]
    fn release_rejects_unknown_when_request_exists_only_in_another_pool() {
        let first_pool = pool();
        let second_pool = CapitalPoolSnapshot {
            pool_id: "other-capital-pool".to_string(),
            ..first_pool.clone()
        };
        let request = reservation_request("request-release-pool-mismatch");
        let release = ReservationReleaseRequest {
            request_id: request.request_id.clone(),
            pool_id: second_pool.pool_id.clone(),
            collateral_group_id: request.collateral_group_id.clone(),
            observed_at_ns: 1_030,
            evidence_label: "nt-order-terminal".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&first_pool, &request, 1_020, None).accepted);

        let decision = ledger.release(&second_pool, &release, 1_040);

        assert!(!decision.accepted);
        assert_eq!(
            decision.reason,
            Some(super::ReservationRejectionReason::UnknownRelease)
        );
        assert_eq!(
            ledger.live_reserved_liability(&first_pool.pool_id),
            Decimal::new(40, 0)
        );
        assert_eq!(
            ledger.live_reserved_liability(&second_pool.pool_id),
            Decimal::ZERO
        );
    }

    #[test]
    fn reservation_enforces_min_remaining_pool_balance() {
        let pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        };
        let request = ReservationRequest {
            request_id: "request-3".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(5, 0),
            observed_at_ns: 1_010,
            evidence_label: "nt-account-and-allowance-snapshot".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();

        let decision = ledger.reserve(&pool, &request, 1_020, Some(Decimal::new(96, 0)));

        assert!(!decision.accepted);
        assert_eq!(
            decision.reason,
            Some(super::ReservationRejectionReason::OverBudget)
        );
        assert_eq!(decision.available_before, Decimal::new(100, 0));
        assert_eq!(decision.available_after, None);
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::ZERO
        );
    }

    #[test]
    fn revalue_live_reservation_replaces_liability_from_fresh_lifecycle_evidence() {
        let pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        };
        let request = ReservationRequest {
            request_id: "request-4".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(40, 0),
            observed_at_ns: 1_010,
            evidence_label: "nt-open-order-snapshot".to_string(),
        };
        let revalue = ReservationRevalueRequest {
            request_id: "request-4".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-partial-fill-snapshot".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);

        let decision = ledger.revalue(&pool, &revalue, 1_040, None);

        assert!(decision.accepted);
        assert_eq!(decision.reason, None);
        assert_eq!(decision.previous_liability, Some(Decimal::new(40, 0)));
        assert_eq!(decision.revalued_liability, Some(Decimal::new(25, 0)));
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(25, 0)
        );
    }

    #[test]
    fn over_budget_revalue_rejects_without_mutating_live_reservation() {
        let pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::new(80, 0),
            max_snapshot_age_ns: 100,
        };
        let request = ReservationRequest {
            request_id: "request-5".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(10, 0),
            observed_at_ns: 1_010,
            evidence_label: "nt-open-order-snapshot".to_string(),
        };
        let revalue = ReservationRevalueRequest {
            request_id: "request-5".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-open-order-revalue".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);

        let decision = ledger.revalue(&pool, &revalue, 1_040, None);

        assert!(!decision.accepted);
        assert_eq!(
            decision.reason,
            Some(super::ReservationRejectionReason::OverBudget)
        );
        assert_eq!(decision.previous_liability, None);
        assert_eq!(decision.revalued_liability, None);
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(10, 0)
        );
    }

    #[test]
    fn upward_revalue_within_budget_increases_live_reservation() {
        let pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        };
        let request = ReservationRequest {
            request_id: "request-6".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(10, 0),
            observed_at_ns: 1_010,
            evidence_label: "nt-open-order-snapshot".to_string(),
        };
        let revalue = ReservationRevalueRequest {
            request_id: "request-6".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-open-order-revalue".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);

        let decision = ledger.revalue(&pool, &revalue, 1_040, None);

        assert!(decision.accepted);
        assert_eq!(decision.previous_liability, Some(Decimal::new(10, 0)));
        assert_eq!(decision.revalued_liability, Some(Decimal::new(25, 0)));
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(25, 0)
        );
    }

    #[test]
    fn revalue_rejects_when_min_remaining_pool_balance_would_be_breached() {
        let pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        };
        let request = ReservationRequest {
            request_id: "request-7".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(10, 0),
            observed_at_ns: 1_010,
            evidence_label: "nt-open-order-snapshot".to_string(),
        };
        let revalue = ReservationRevalueRequest {
            request_id: "request-7".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-open-order-revalue".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);

        let decision = ledger.revalue(&pool, &revalue, 1_040, Some(Decimal::new(80, 0)));

        assert!(!decision.accepted);
        assert_eq!(
            decision.reason,
            Some(super::ReservationRejectionReason::OverBudget)
        );
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(10, 0)
        );
    }

    #[test]
    fn out_of_order_revalue_rejects_without_mutating_live_reservation() {
        let pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        };
        let request = ReservationRequest {
            request_id: "request-6".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(40, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-open-order-snapshot".to_string(),
        };
        let revalue = ReservationRevalueRequest {
            request_id: "request-6".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_020,
            evidence_label: "nt-delayed-partial-fill-snapshot".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_040, None).accepted);

        let decision = ledger.revalue(&pool, &revalue, 1_050, None);

        assert!(!decision.accepted);
        assert_eq!(
            decision.reason,
            Some(super::ReservationRejectionReason::StaleRequest)
        );
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(40, 0)
        );
    }

    #[test]
    fn equal_timestamp_revalue_rejects_without_mutating_live_reservation() {
        let pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        };
        let request = ReservationRequest {
            request_id: "request-8".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(40, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-open-order-snapshot".to_string(),
        };
        let revalue = ReservationRevalueRequest {
            request_id: "request-8".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-duplicate-order-snapshot".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_040, None).accepted);

        let decision = ledger.revalue(&pool, &revalue, 1_050, None);

        assert!(!decision.accepted);
        assert_eq!(
            decision.reason,
            Some(super::ReservationRejectionReason::StaleRequest)
        );
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(40, 0)
        );
    }

    #[test]
    fn revalue_rejects_collateral_group_mismatch_without_mutating_live_reservation() {
        let pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        };
        let request = ReservationRequest {
            request_id: "request-9".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(40, 0),
            observed_at_ns: 1_010,
            evidence_label: "nt-open-order-snapshot".to_string(),
        };
        let revalue = ReservationRevalueRequest {
            request_id: "request-9".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "eth-updown-15m".to_string(),
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-open-order-revalue".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);

        let decision = ledger.revalue(&pool, &revalue, 1_040, None);

        assert!(!decision.accepted);
        assert_eq!(
            decision.reason,
            Some(super::ReservationRejectionReason::CollateralGroupMismatch)
        );
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(40, 0)
        );
    }

    #[test]
    fn revalue_rejects_unknown_when_request_exists_only_in_another_pool() {
        let first_pool = CapitalPoolSnapshot {
            source: "nt_account_snapshot".to_string(),
            observed_at_ns: 1_000,
            pool_id: "polymarket-live".to_string(),
            max_pool_liability: Decimal::new(100, 0),
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: 100,
        };
        let second_pool = CapitalPoolSnapshot {
            pool_id: "kalshi-live".to_string(),
            ..first_pool.clone()
        };
        let request = ReservationRequest {
            request_id: "request-6".to_string(),
            pool_id: "polymarket-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(40, 0),
            observed_at_ns: 1_010,
            evidence_label: "nt-open-order-snapshot".to_string(),
        };
        let revalue = ReservationRevalueRequest {
            request_id: "request-6".to_string(),
            pool_id: "kalshi-live".to_string(),
            collateral_group_id: "btc-updown-15m".to_string(),
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-open-order-revalue".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&first_pool, &request, 1_020, None).accepted);

        let decision = ledger.revalue(&second_pool, &revalue, 1_040, None);

        assert!(!decision.accepted);
        assert_eq!(
            decision.reason,
            Some(super::ReservationRejectionReason::UnknownReservation)
        );
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(40, 0)
        );
        assert_eq!(ledger.live_reserved_liability("kalshi-live"), Decimal::ZERO);
    }
}
