use rust_decimal::Decimal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossHaltReason {
    PerTradeLossLimit,
    DailyLossLimit,
    RollingLossLimit,
    MaxDrawdownLimit,
    StaleLossSnapshot,
}

impl LossHaltReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PerTradeLossLimit => "per_trade_loss_limit",
            Self::DailyLossLimit => "daily_loss_limit",
            Self::RollingLossLimit => "rolling_loss_limit",
            Self::MaxDrawdownLimit => "max_drawdown_limit",
            Self::StaleLossSnapshot => "stale_loss_snapshot",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossGovernorPolicy {
    pub max_snapshot_age_ns: u64,
    pub max_per_trade_loss: Option<Decimal>,
    pub max_daily_loss: Option<Decimal>,
    pub max_rolling_loss: Option<Decimal>,
    pub max_drawdown: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossSnapshot {
    pub source: String,
    pub observed_at_ns: u64,
    pub per_trade_pnl: Option<Decimal>,
    pub daily_pnl: Option<Decimal>,
    pub rolling_pnl: Option<Decimal>,
    pub current_equity: Option<Decimal>,
    pub peak_equity: Option<Decimal>,
    pub source_observations: LossSourceObservationTimestamps,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LossSourceObservationTimestamps {
    pub last_account_state_observed_at_ns: Option<u64>,
    pub last_portfolio_snapshot_observed_at_ns: Option<u64>,
    pub last_position_event_observed_at_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossSnapshotStaleReason {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossSnapshotDiagnostics {
    pub snapshot_present: bool,
    pub snapshot_observed_at_ns: Option<u64>,
    pub admission_now_ns: u64,
    pub snapshot_age_ns: Option<u64>,
    pub max_snapshot_age_ns: Option<u64>,
    pub snapshot_source: Option<String>,
    pub per_trade_pnl_present: bool,
    pub daily_pnl_present: bool,
    pub rolling_pnl_present: bool,
    pub current_equity_present: bool,
    pub peak_equity_present: bool,
    pub last_account_state_observed_at_ns: Option<u64>,
    pub last_portfolio_snapshot_observed_at_ns: Option<u64>,
    pub last_position_event_observed_at_ns: Option<u64>,
    pub stale_reason: Option<LossSnapshotStaleReason>,
}

impl LossSnapshotDiagnostics {
    pub fn not_evaluated(admission_now_ns: u64) -> Self {
        Self {
            snapshot_present: false,
            snapshot_observed_at_ns: None,
            admission_now_ns,
            snapshot_age_ns: None,
            max_snapshot_age_ns: None,
            snapshot_source: None,
            per_trade_pnl_present: false,
            daily_pnl_present: false,
            rolling_pnl_present: false,
            current_equity_present: false,
            peak_equity_present: false,
            last_account_state_observed_at_ns: None,
            last_portfolio_snapshot_observed_at_ns: None,
            last_position_event_observed_at_ns: None,
            stale_reason: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossAdmissionDecision {
    pub accepted: bool,
    pub halt_reasons: Vec<LossHaltReason>,
    pub diagnostics: LossSnapshotDiagnostics,
}

#[must_use]
pub fn evaluate_loss_admission(
    policy: &LossGovernorPolicy,
    snapshot: Option<&LossSnapshot>,
    now_ns: u64,
) -> LossAdmissionDecision {
    evaluate_loss_admission_with_observations(
        policy,
        snapshot,
        now_ns,
        snapshot.map_or_else(LossSourceObservationTimestamps::default, |snapshot| {
            snapshot.source_observations
        }),
    )
}

#[must_use]
pub fn evaluate_loss_admission_with_observations(
    policy: &LossGovernorPolicy,
    snapshot: Option<&LossSnapshot>,
    now_ns: u64,
    source_observations: LossSourceObservationTimestamps,
) -> LossAdmissionDecision {
    let mut halt_reasons = Vec::new();
    let diagnostics = loss_snapshot_diagnostics(policy, snapshot, now_ns, source_observations);

    let Some(snapshot) = snapshot else {
        return rejected(LossHaltReason::StaleLossSnapshot, diagnostics);
    };

    if diagnostics.stale_reason.is_some() {
        return rejected(LossHaltReason::StaleLossSnapshot, diagnostics);
    }

    match (policy.max_per_trade_loss, snapshot.per_trade_pnl) {
        (Some(limit), Some(pnl)) if loss_breaches(pnl, limit) => {
            halt_reasons.push(LossHaltReason::PerTradeLossLimit);
        }
        _ => {}
    }
    match (policy.max_daily_loss, snapshot.daily_pnl) {
        (Some(limit), Some(pnl)) if loss_breaches(pnl, limit) => {
            halt_reasons.push(LossHaltReason::DailyLossLimit);
        }
        _ => {}
    }
    match (policy.max_rolling_loss, snapshot.rolling_pnl) {
        (Some(limit), Some(pnl)) if loss_breaches(pnl, limit) => {
            halt_reasons.push(LossHaltReason::RollingLossLimit);
        }
        _ => {}
    }
    match (
        policy.max_drawdown,
        snapshot.current_equity,
        snapshot.peak_equity,
    ) {
        (Some(limit), Some(current), Some(peak)) if drawdown_breaches(current, peak, limit) => {
            halt_reasons.push(LossHaltReason::MaxDrawdownLimit);
        }
        _ => {}
    }

    LossAdmissionDecision {
        accepted: halt_reasons.is_empty(),
        halt_reasons,
        diagnostics,
    }
}

fn rejected(reason: LossHaltReason, diagnostics: LossSnapshotDiagnostics) -> LossAdmissionDecision {
    LossAdmissionDecision {
        accepted: false,
        halt_reasons: vec![reason],
        diagnostics,
    }
}

fn loss_snapshot_diagnostics(
    policy: &LossGovernorPolicy,
    snapshot: Option<&LossSnapshot>,
    now_ns: u64,
    source_observations: LossSourceObservationTimestamps,
) -> LossSnapshotDiagnostics {
    let Some(snapshot) = snapshot else {
        return LossSnapshotDiagnostics {
            snapshot_present: false,
            snapshot_observed_at_ns: None,
            admission_now_ns: now_ns,
            snapshot_age_ns: None,
            max_snapshot_age_ns: Some(policy.max_snapshot_age_ns),
            snapshot_source: None,
            per_trade_pnl_present: false,
            daily_pnl_present: false,
            rolling_pnl_present: false,
            current_equity_present: false,
            peak_equity_present: false,
            last_account_state_observed_at_ns: source_observations
                .last_account_state_observed_at_ns,
            last_portfolio_snapshot_observed_at_ns: source_observations
                .last_portfolio_snapshot_observed_at_ns,
            last_position_event_observed_at_ns: source_observations
                .last_position_event_observed_at_ns,
            stale_reason: Some(LossSnapshotStaleReason::MissingSnapshot),
        };
    };
    let source_empty = snapshot.source.trim().is_empty();
    let future_dated = snapshot.observed_at_ns > now_ns;
    let snapshot_age_ns = (!future_dated).then_some(now_ns - snapshot.observed_at_ns);
    let age_exceeded = snapshot_age_ns.is_some_and(|age| age > policy.max_snapshot_age_ns);
    let missing_required_field = (policy.max_per_trade_loss.is_some()
        && snapshot.per_trade_pnl.is_none())
        || (policy.max_daily_loss.is_some() && snapshot.daily_pnl.is_none())
        || (policy.max_rolling_loss.is_some() && snapshot.rolling_pnl.is_none())
        || (policy.max_drawdown.is_some()
            && (snapshot.current_equity.is_none() || snapshot.peak_equity.is_none()));
    let stale_reason = if source_empty {
        Some(LossSnapshotStaleReason::SourceEmpty)
    } else if future_dated {
        Some(LossSnapshotStaleReason::FutureDated)
    } else if age_exceeded {
        Some(LossSnapshotStaleReason::AgeExceeded)
    } else if missing_required_field {
        Some(LossSnapshotStaleReason::MissingRequiredField)
    } else {
        None
    };
    LossSnapshotDiagnostics {
        snapshot_present: true,
        snapshot_observed_at_ns: Some(snapshot.observed_at_ns),
        admission_now_ns: now_ns,
        snapshot_age_ns,
        max_snapshot_age_ns: Some(policy.max_snapshot_age_ns),
        snapshot_source: Some(snapshot.source.clone()),
        per_trade_pnl_present: snapshot.per_trade_pnl.is_some(),
        daily_pnl_present: snapshot.daily_pnl.is_some(),
        rolling_pnl_present: snapshot.rolling_pnl.is_some(),
        current_equity_present: snapshot.current_equity.is_some(),
        peak_equity_present: snapshot.peak_equity.is_some(),
        last_account_state_observed_at_ns: source_observations.last_account_state_observed_at_ns,
        last_portfolio_snapshot_observed_at_ns: source_observations
            .last_portfolio_snapshot_observed_at_ns,
        last_position_event_observed_at_ns: source_observations.last_position_event_observed_at_ns,
        stale_reason,
    }
}

fn loss_breaches(pnl: Decimal, limit: Decimal) -> bool {
    pnl < Decimal::ZERO && -pnl >= limit
}

fn drawdown_breaches(current_equity: Decimal, peak_equity: Decimal, limit: Decimal) -> bool {
    peak_equity > current_equity && peak_equity - current_equity >= limit
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::{
        LossGovernorPolicy, LossHaltReason, LossSnapshot, LossSourceObservationTimestamps,
        evaluate_loss_admission,
    };

    fn policy() -> LossGovernorPolicy {
        LossGovernorPolicy {
            max_snapshot_age_ns: 1_000,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_daily_loss: None,
            max_rolling_loss: None,
            max_drawdown: None,
        }
    }

    fn snapshot() -> LossSnapshot {
        LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 10_000,
            per_trade_pnl: Some(Decimal::new(-11, 0)),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::default(),
        }
    }

    #[test]
    fn per_trade_loss_breach_rejects_admission() {
        let decision = evaluate_loss_admission(&policy(), Some(&snapshot()), 10_100);

        assert!(!decision.accepted);
        assert_eq!(
            decision.halt_reasons,
            vec![LossHaltReason::PerTradeLossLimit]
        );
        assert_eq!(decision.halt_reasons[0].as_str(), "per_trade_loss_limit");
    }

    #[test]
    fn configured_per_trade_limit_with_missing_per_trade_pnl_fails_closed() {
        // A configured per-trade limit must fail closed when the snapshot has no
        // per-trade PnL, rather than admit on a missing field.
        let policy = policy();
        assert!(policy.max_per_trade_loss.is_some());

        let mut snapshot = snapshot();
        snapshot.per_trade_pnl = None;

        let decision = evaluate_loss_admission(&policy, Some(&snapshot), 10_100);

        assert!(!decision.accepted);
        assert_eq!(
            decision.halt_reasons,
            vec![LossHaltReason::StaleLossSnapshot]
        );
        assert_eq!(decision.halt_reasons[0].as_str(), "stale_loss_snapshot");
    }

    #[test]
    fn daily_loss_breach_rejects_admission() {
        let mut policy = policy();
        policy.max_per_trade_loss = None;
        policy.max_daily_loss = Some(Decimal::new(25, 0));

        let mut snapshot = snapshot();
        snapshot.per_trade_pnl = None;
        snapshot.daily_pnl = Some(Decimal::new(-26, 0));

        let decision = evaluate_loss_admission(&policy, Some(&snapshot), 10_100);

        assert!(!decision.accepted);
        assert_eq!(decision.halt_reasons, vec![LossHaltReason::DailyLossLimit]);
        assert_eq!(decision.halt_reasons[0].as_str(), "daily_loss_limit");
    }

    #[test]
    fn stale_missing_or_unattributed_snapshot_fails_closed() {
        let mut policy = policy();
        policy.max_per_trade_loss = None;
        policy.max_daily_loss = Some(Decimal::new(25, 0));

        let mut missing_daily = snapshot();
        missing_daily.per_trade_pnl = None;
        missing_daily.daily_pnl = None;

        let mut unattributed = missing_daily.clone();
        unattributed.source = " ".to_string();
        unattributed.daily_pnl = Some(Decimal::new(0, 0));

        let mut stale = unattributed.clone();
        stale.source = "nt_portfolio_snapshot".to_string();
        stale.observed_at_ns = 8_999;

        for candidate in [
            None,
            Some(&missing_daily),
            Some(&unattributed),
            Some(&stale),
        ] {
            let decision = evaluate_loss_admission(&policy, candidate, 10_100);

            assert!(!decision.accepted);
            assert_eq!(
                decision.halt_reasons,
                vec![LossHaltReason::StaleLossSnapshot]
            );
            assert_eq!(decision.halt_reasons[0].as_str(), "stale_loss_snapshot");
        }
    }

    #[test]
    fn fresh_below_limit_snapshot_accepts_admission() {
        let mut policy = policy();
        policy.max_daily_loss = Some(Decimal::new(25, 0));

        let mut snapshot = snapshot();
        snapshot.per_trade_pnl = Some(Decimal::new(-9, 0));
        snapshot.daily_pnl = Some(Decimal::new(-24, 0));

        let decision = evaluate_loss_admission(&policy, Some(&snapshot), 10_100);

        assert!(decision.accepted);
        assert!(decision.halt_reasons.is_empty());
    }

    #[test]
    fn rolling_loss_breach_rejects_admission() {
        let mut policy = policy();
        policy.max_per_trade_loss = None;
        policy.max_rolling_loss = Some(Decimal::new(30, 0));

        let mut snapshot = snapshot();
        snapshot.per_trade_pnl = None;
        snapshot.rolling_pnl = Some(Decimal::new(-31, 0));

        let decision = evaluate_loss_admission(&policy, Some(&snapshot), 10_100);

        assert!(!decision.accepted);
        assert_eq!(
            decision.halt_reasons,
            vec![LossHaltReason::RollingLossLimit]
        );
        assert_eq!(decision.halt_reasons[0].as_str(), "rolling_loss_limit");
    }

    #[test]
    fn max_drawdown_breach_rejects_admission() {
        let mut policy = policy();
        policy.max_per_trade_loss = None;
        policy.max_drawdown = Some(Decimal::new(25, 0));

        let mut snapshot = snapshot();
        snapshot.per_trade_pnl = None;
        snapshot.current_equity = Some(Decimal::new(974, 0));
        snapshot.peak_equity = Some(Decimal::new(1000, 0));

        let decision = evaluate_loss_admission(&policy, Some(&snapshot), 10_100);

        assert!(!decision.accepted);
        assert_eq!(
            decision.halt_reasons,
            vec![LossHaltReason::MaxDrawdownLimit]
        );
        assert_eq!(decision.halt_reasons[0].as_str(), "max_drawdown_limit");
    }

    #[test]
    fn multiple_breaches_return_deterministic_halt_evidence() {
        let policy = LossGovernorPolicy {
            max_snapshot_age_ns: 1_000,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_daily_loss: Some(Decimal::new(25, 0)),
            max_rolling_loss: Some(Decimal::new(30, 0)),
            max_drawdown: Some(Decimal::new(40, 0)),
        };
        let snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 10_000,
            per_trade_pnl: Some(Decimal::new(-11, 0)),
            daily_pnl: Some(Decimal::new(-26, 0)),
            rolling_pnl: Some(Decimal::new(-31, 0)),
            current_equity: Some(Decimal::new(959, 0)),
            peak_equity: Some(Decimal::new(1000, 0)),
            source_observations: LossSourceObservationTimestamps::default(),
        };

        let decision = evaluate_loss_admission(&policy, Some(&snapshot), 10_100);

        assert!(!decision.accepted);
        assert_eq!(
            decision.halt_reasons,
            vec![
                LossHaltReason::PerTradeLossLimit,
                LossHaltReason::DailyLossLimit,
                LossHaltReason::RollingLossLimit,
                LossHaltReason::MaxDrawdownLimit,
            ]
        );
        assert_eq!(
            decision
                .halt_reasons
                .iter()
                .map(|reason| reason.as_str())
                .collect::<Vec<_>>(),
            vec![
                "per_trade_loss_limit",
                "daily_loss_limit",
                "rolling_loss_limit",
                "max_drawdown_limit",
            ]
        );
    }

    #[test]
    fn loss_threshold_equality_rejects_admission() {
        let policy = LossGovernorPolicy {
            max_snapshot_age_ns: 1_000,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_daily_loss: Some(Decimal::new(25, 0)),
            max_rolling_loss: Some(Decimal::new(30, 0)),
            max_drawdown: Some(Decimal::new(40, 0)),
        };
        let snapshot = LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 10_000,
            per_trade_pnl: Some(Decimal::new(-10, 0)),
            daily_pnl: Some(Decimal::new(-25, 0)),
            rolling_pnl: Some(Decimal::new(-30, 0)),
            current_equity: Some(Decimal::new(960, 0)),
            peak_equity: Some(Decimal::new(1000, 0)),
            source_observations: LossSourceObservationTimestamps::default(),
        };

        let decision = evaluate_loss_admission(&policy, Some(&snapshot), 10_100);

        assert!(!decision.accepted);
        assert_eq!(
            decision.halt_reasons,
            vec![
                LossHaltReason::PerTradeLossLimit,
                LossHaltReason::DailyLossLimit,
                LossHaltReason::RollingLossLimit,
                LossHaltReason::MaxDrawdownLimit,
            ]
        );
    }
}
