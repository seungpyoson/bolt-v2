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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossAdmissionDecision {
    pub accepted: bool,
    pub halt_reasons: Vec<LossHaltReason>,
}

#[must_use]
pub fn evaluate_loss_admission(
    policy: &LossGovernorPolicy,
    snapshot: Option<&LossSnapshot>,
    now_ns: u64,
) -> LossAdmissionDecision {
    let mut halt_reasons = Vec::new();

    let Some(snapshot) = snapshot else {
        return rejected(LossHaltReason::StaleLossSnapshot);
    };

    if snapshot_is_stale(policy, snapshot, now_ns) {
        return rejected(LossHaltReason::StaleLossSnapshot);
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
    }
}

fn rejected(reason: LossHaltReason) -> LossAdmissionDecision {
    LossAdmissionDecision {
        accepted: false,
        halt_reasons: vec![reason],
    }
}

fn snapshot_is_stale(policy: &LossGovernorPolicy, snapshot: &LossSnapshot, now_ns: u64) -> bool {
    if snapshot.source.trim().is_empty() || snapshot.observed_at_ns > now_ns {
        return true;
    }

    let age = now_ns - snapshot.observed_at_ns;
    if age > policy.max_snapshot_age_ns {
        return true;
    }

    (policy.max_per_trade_loss.is_some() && snapshot.per_trade_pnl.is_none())
        || (policy.max_daily_loss.is_some() && snapshot.daily_pnl.is_none())
        || (policy.max_rolling_loss.is_some() && snapshot.rolling_pnl.is_none())
        || (policy.max_drawdown.is_some()
            && (snapshot.current_equity.is_none() || snapshot.peak_equity.is_none()))
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

    use super::{LossGovernorPolicy, LossHaltReason, LossSnapshot, evaluate_loss_admission};

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
