use rust_decimal::Decimal;

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
    pub liability: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationRejectionReason {
    MissingEvidence,
    StaleRequest,
    PoolMismatch,
    OverBudget,
    InvalidRequest,
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

    pub fn reserve(
        &mut self,
        pool: &CapitalPoolSnapshot,
        request: &ReservationRequest,
        now_ns: u64,
        max_snapshot_age_ns: u64,
        min_remaining_pool_balance: Option<Decimal>,
    ) -> ReservationDecision {
        let available_before = pool.max_pool_liability
            - pool.committed_liability
            - self.live_reserved_liability(&pool.pool_id);

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
        if stale(pool.observed_at_ns, now_ns, max_snapshot_age_ns)
            || stale(request.observed_at_ns, now_ns, max_snapshot_age_ns)
            || request.observed_at_ns < pool.observed_at_ns
        {
            return rejected(
                ReservationRejectionReason::StaleRequest,
                request.liability,
                available_before,
            );
        }
        if self
            .live_reservations
            .iter()
            .any(|reservation| reservation.request_id == request.request_id)
        {
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
        if let Some(min_remaining_pool_balance) = min_remaining_pool_balance
            && available_before - request.liability < min_remaining_pool_balance
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
            available_after: Some(available_before - request.liability),
        }
    }

    pub fn live_reserved_liability(&self, pool_id: &str) -> Decimal {
        self.live_reservations
            .iter()
            .filter(|reservation| reservation.pool_id == pool_id)
            .map(|reservation| reservation.liability)
            .sum()
    }

    pub fn release(
        &mut self,
        request_id: &str,
        evidence_label: &str,
    ) -> ReservationReleaseDecision {
        if request_id.trim().is_empty() {
            return rejected_release(ReservationRejectionReason::InvalidRequest);
        }
        if evidence_label.trim().is_empty() {
            return rejected_release(ReservationRejectionReason::MissingEvidence);
        }
        let Some(index) = self
            .live_reservations
            .iter()
            .position(|reservation| reservation.request_id == request_id)
        else {
            return rejected_release(ReservationRejectionReason::UnknownRelease);
        };
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
        max_snapshot_age_ns: u64,
        min_remaining_pool_balance: Option<Decimal>,
    ) -> ReservationRevalueDecision {
        if !self.reconciliation_complete {
            return rejected_revalue(ReservationRejectionReason::ReconciliationRequired);
        }
        if pool.pool_id != request.pool_id {
            return rejected_revalue(ReservationRejectionReason::PoolMismatch);
        }
        if request.request_id.trim().is_empty()
            || request.pool_id.trim().is_empty()
            || request.liability <= Decimal::ZERO
        {
            return rejected_revalue(ReservationRejectionReason::InvalidRequest);
        }
        if pool.source.trim().is_empty() || request.evidence_label.trim().is_empty() {
            return rejected_revalue(ReservationRejectionReason::MissingEvidence);
        }
        if stale(pool.observed_at_ns, now_ns, max_snapshot_age_ns)
            || stale(request.observed_at_ns, now_ns, max_snapshot_age_ns)
            || request.observed_at_ns < pool.observed_at_ns
        {
            return rejected_revalue(ReservationRejectionReason::StaleRequest);
        }
        let Some(index) = self
            .live_reservations
            .iter()
            .position(|reservation| reservation.request_id == request.request_id)
        else {
            return rejected_revalue(ReservationRejectionReason::UnknownReservation);
        };
        if self.live_reservations[index].pool_id != pool.pool_id {
            return rejected_revalue(ReservationRejectionReason::PoolMismatch);
        }
        let previous_liability = self.live_reservations[index].liability;
        let available_before = pool.max_pool_liability
            - pool.committed_liability
            - self.live_reserved_liability(&pool.pool_id)
            + previous_liability;
        if request.liability > available_before {
            return rejected_revalue(ReservationRejectionReason::OverBudget);
        }
        if let Some(min_remaining_pool_balance) = min_remaining_pool_balance
            && available_before - request.liability < min_remaining_pool_balance
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

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::{
        CapitalPoolSnapshot, ReservationLedger, ReservationRequest, ReservationRevalueRequest,
    };

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
        let decision = ledger.reserve(&pool, &request, 1_020, 100, None);

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
        assert!(ledger.reserve(&pool, &request, 1_020, 100, None).accepted);

        let release = ledger.release("request-2", "nt-order-terminal");

        assert!(release.accepted);
        assert_eq!(release.reason, None);
        assert_eq!(release.released_liability, Some(Decimal::new(40, 0)));
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::ZERO
        );

        let duplicate = ledger.release("request-2", "nt-order-terminal");

        assert!(!duplicate.accepted);
        assert_eq!(
            duplicate.reason,
            Some(super::ReservationRejectionReason::UnknownRelease)
        );
        assert_eq!(duplicate.released_liability, None);
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

        let decision = ledger.reserve(&pool, &request, 1_020, 100, Some(Decimal::new(96, 0)));

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
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-partial-fill-snapshot".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, 100, None).accepted);

        let decision = ledger.revalue(&pool, &revalue, 1_040, 100, None);

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
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-open-order-revalue".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(ledger.reserve(&pool, &request, 1_020, 100, None).accepted);

        let decision = ledger.revalue(&pool, &revalue, 1_040, 100, None);

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
    fn revalue_rejects_when_existing_reservation_belongs_to_another_pool() {
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
            liability: Decimal::new(25, 0),
            observed_at_ns: 1_030,
            evidence_label: "nt-open-order-revalue".to_string(),
        };
        let mut ledger = ReservationLedger::reconciled();
        assert!(
            ledger
                .reserve(&first_pool, &request, 1_020, 100, None)
                .accepted
        );

        let decision = ledger.revalue(&second_pool, &revalue, 1_040, 100, None);

        assert!(!decision.accepted);
        assert_eq!(
            decision.reason,
            Some(super::ReservationRejectionReason::PoolMismatch)
        );
        assert_eq!(
            ledger.live_reserved_liability("polymarket-live"),
            Decimal::new(40, 0)
        );
        assert_eq!(ledger.live_reserved_liability("kalshi-live"), Decimal::ZERO);
    }
}
