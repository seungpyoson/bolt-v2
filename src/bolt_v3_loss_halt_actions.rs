use std::{
    path::{Component, Path},
    rc::Rc,
};

use nautilus_model::enums::TradingState;
use serde::Deserialize;

use crate::bolt_v3_loss_governor::{
    LossAdmissionDecision, LossHaltReason, LossSnapshot, LossSourceObservationTimestamps,
};
use crate::bolt_v3_numeric::is_sha256_hex_digest;

pub type LossGovernorHaltActionHandler =
    Rc<dyn Fn(Option<&LossSnapshot>, u64, LossSourceObservationTimestamps)>;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LossGovernorTradingStateAction {
    None,
    Reducing,
    Halted,
}

impl LossGovernorTradingStateAction {
    #[must_use]
    pub const fn as_trading_state(self) -> Option<TradingState> {
        match self {
            Self::None => None,
            Self::Reducing => Some(TradingState::Reducing),
            Self::Halted => Some(TradingState::Halted),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LossGovernorRecoveryMode {
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LossGovernorHaltActionPolicy {
    pub on_loss_breach_trading_state: LossGovernorTradingStateAction,
    pub on_untrusted_snapshot_trading_state: LossGovernorTradingStateAction,
    pub recovery_mode: LossGovernorRecoveryMode,
    pub manual_recovery_evidence_max_path_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossGovernorManualRecoveryEvidence {
    operator_id: String,
    evidence_path: String,
    evidence_sha256: String,
    observed_at_ns: u64,
}

impl LossGovernorManualRecoveryEvidence {
    pub fn new(
        operator_id: impl Into<String>,
        evidence_path: impl Into<String>,
        evidence_sha256: impl Into<String>,
        observed_at_ns: u64,
        max_evidence_path_bytes: usize,
    ) -> Result<Self, LossGovernorManualRecoveryEvidenceError> {
        let evidence = Self {
            operator_id: operator_id.into(),
            evidence_path: evidence_path.into(),
            evidence_sha256: evidence_sha256.into(),
            observed_at_ns,
        };
        evidence.validate(max_evidence_path_bytes)?;
        Ok(evidence)
    }

    #[must_use]
    pub fn operator_id(&self) -> &str {
        &self.operator_id
    }

    #[must_use]
    pub fn evidence_path(&self) -> &str {
        &self.evidence_path
    }

    #[must_use]
    pub fn evidence_sha256(&self) -> &str {
        &self.evidence_sha256
    }

    #[must_use]
    pub const fn observed_at_ns(&self) -> u64 {
        self.observed_at_ns
    }

    fn validate(
        &self,
        max_evidence_path_bytes: usize,
    ) -> Result<(), LossGovernorManualRecoveryEvidenceError> {
        if self.operator_id.trim().is_empty() {
            return Err(LossGovernorManualRecoveryEvidenceError::MissingOperatorId);
        }
        if !is_sha256_hex_digest(&self.evidence_sha256) {
            return Err(LossGovernorManualRecoveryEvidenceError::InvalidEvidenceSha256);
        }
        if self.observed_at_ns == 0 {
            return Err(LossGovernorManualRecoveryEvidenceError::MissingObservedAt);
        }
        validate_recovery_evidence_path(&self.evidence_path, max_evidence_path_bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossGovernorManualRecoveryEvidenceError {
    MissingOperatorId,
    InvalidEvidencePathLimit,
    EmptyEvidencePath,
    EvidencePathTooLong,
    AbsoluteEvidencePath,
    ParentEvidencePath,
    InvalidEvidenceSha256,
    MissingObservedAt,
}

fn validate_recovery_evidence_path(
    path: &str,
    max_evidence_path_bytes: usize,
) -> Result<(), LossGovernorManualRecoveryEvidenceError> {
    if max_evidence_path_bytes == 0 {
        return Err(LossGovernorManualRecoveryEvidenceError::InvalidEvidencePathLimit);
    }
    if path.is_empty() {
        return Err(LossGovernorManualRecoveryEvidenceError::EmptyEvidencePath);
    }
    if path.len() > max_evidence_path_bytes {
        return Err(LossGovernorManualRecoveryEvidenceError::EvidencePathTooLong);
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(LossGovernorManualRecoveryEvidenceError::AbsoluteEvidencePath);
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(LossGovernorManualRecoveryEvidenceError::ParentEvidencePath);
    }
    Ok(())
}

#[must_use]
pub fn loss_governor_trading_state_action_for_reasons(
    policy: &LossGovernorHaltActionPolicy,
    halt_reasons: &[LossHaltReason],
) -> LossGovernorTradingStateAction {
    if halt_reasons.is_empty() {
        return LossGovernorTradingStateAction::None;
    }

    halt_reasons
        .iter()
        .fold(LossGovernorTradingStateAction::None, |action, reason| {
            let reason_action = match reason {
                LossHaltReason::StaleLossSnapshot => policy.on_untrusted_snapshot_trading_state,
                LossHaltReason::PerTradeLossLimit
                | LossHaltReason::DailyLossLimit
                | LossHaltReason::RollingLossLimit
                | LossHaltReason::MaxDrawdownLimit => policy.on_loss_breach_trading_state,
            };
            strongest_trading_state_action(action, reason_action)
        })
}

#[must_use]
pub fn next_loss_governor_halt_action(
    policy: &LossGovernorHaltActionPolicy,
    current_state: TradingState,
    decision: &LossAdmissionDecision,
) -> Option<TradingState> {
    if decision.accepted {
        return None;
    }

    match policy.recovery_mode {
        LossGovernorRecoveryMode::Manual => {}
    }

    loss_governor_trading_state_action_for_reasons(policy, &decision.halt_reasons)
        .as_trading_state()
        .filter(|target| trading_state_severity(*target) > trading_state_severity(current_state))
}

#[must_use]
pub fn next_loss_governor_trading_state(
    policy: &LossGovernorHaltActionPolicy,
    current_state: TradingState,
    decision: &LossAdmissionDecision,
) -> Option<TradingState> {
    next_loss_governor_halt_action(policy, current_state, decision)
}

pub struct LossGovernorManualRecoveryRequest<'a> {
    pub policy: &'a LossGovernorHaltActionPolicy,
    pub current_state: TradingState,
    pub decision: &'a LossAdmissionDecision,
    pub snapshot: Option<&'a LossSnapshot>,
    pub now_ns: u64,
    pub max_snapshot_age_ns: u64,
    pub evidence: Option<&'a LossGovernorManualRecoveryEvidence>,
    pub max_evidence_path_bytes: usize,
}

#[must_use]
pub fn next_loss_governor_manual_recovery_trading_state(
    request: LossGovernorManualRecoveryRequest<'_>,
) -> Option<TradingState> {
    let LossGovernorManualRecoveryRequest {
        policy,
        current_state,
        decision,
        snapshot,
        now_ns,
        max_snapshot_age_ns,
        evidence,
        max_evidence_path_bytes,
    } = request;

    match policy.recovery_mode {
        LossGovernorRecoveryMode::Manual => {}
    }
    if !matches!(current_state, TradingState::Halted | TradingState::Reducing) {
        return None;
    }
    if !decision.accepted || max_snapshot_age_ns == 0 {
        return None;
    }
    let snapshot = snapshot?;
    if snapshot.source.trim().is_empty() || snapshot.observed_at_ns > now_ns {
        return None;
    }
    if now_ns - snapshot.observed_at_ns > max_snapshot_age_ns {
        return None;
    }
    let evidence = evidence?;
    if evidence.validate(max_evidence_path_bytes).is_err() {
        return None;
    }
    if evidence.observed_at_ns > now_ns {
        return None;
    }
    if evidence.observed_at_ns < snapshot.observed_at_ns {
        return None;
    }
    Some(TradingState::Active)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TradingStateSeverity {
    Active,
    Reducing,
    Halted,
}

const fn trading_state_severity(state: TradingState) -> TradingStateSeverity {
    match state {
        TradingState::Active => TradingStateSeverity::Active,
        TradingState::Reducing => TradingStateSeverity::Reducing,
        TradingState::Halted => TradingStateSeverity::Halted,
    }
}

const fn trading_state_action_severity(
    action: LossGovernorTradingStateAction,
) -> TradingStateSeverity {
    match action {
        LossGovernorTradingStateAction::None => TradingStateSeverity::Active,
        LossGovernorTradingStateAction::Reducing => TradingStateSeverity::Reducing,
        LossGovernorTradingStateAction::Halted => TradingStateSeverity::Halted,
    }
}

fn strongest_trading_state_action(
    current: LossGovernorTradingStateAction,
    candidate: LossGovernorTradingStateAction,
) -> LossGovernorTradingStateAction {
    if trading_state_action_severity(candidate) > trading_state_action_severity(current) {
        candidate
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LossGovernorHaltActionPolicy, LossGovernorManualRecoveryEvidence,
        LossGovernorManualRecoveryEvidenceError, LossGovernorManualRecoveryRequest,
        LossGovernorRecoveryMode, LossGovernorTradingStateAction,
        next_loss_governor_manual_recovery_trading_state, next_loss_governor_trading_state,
    };
    use crate::bolt_v3_loss_governor::{
        LossAdmissionDecision, LossHaltReason, LossSnapshot, LossSnapshotDiagnostics,
        LossSourceObservationTimestamps,
    };
    use nautilus_model::enums::TradingState;
    use rust_decimal::Decimal;

    fn policy(
        on_loss_breach_trading_state: LossGovernorTradingStateAction,
        on_untrusted_snapshot_trading_state: LossGovernorTradingStateAction,
    ) -> LossGovernorHaltActionPolicy {
        LossGovernorHaltActionPolicy {
            on_loss_breach_trading_state,
            on_untrusted_snapshot_trading_state,
            recovery_mode: LossGovernorRecoveryMode::Manual,
            manual_recovery_evidence_max_path_bytes: 128,
        }
    }

    fn rejected(halt_reasons: Vec<LossHaltReason>) -> LossAdmissionDecision {
        LossAdmissionDecision {
            accepted: false,
            halt_reasons,
            diagnostics: LossSnapshotDiagnostics::not_evaluated(1),
        }
    }

    fn accepted() -> LossAdmissionDecision {
        LossAdmissionDecision {
            accepted: true,
            halt_reasons: Vec::new(),
            diagnostics: LossSnapshotDiagnostics::not_evaluated(1),
        }
    }

    fn fresh_snapshot() -> LossSnapshot {
        LossSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::ZERO),
            daily_pnl: Some(Decimal::ZERO),
            rolling_pnl: Some(Decimal::ZERO),
            current_equity: Some(Decimal::new(100, 0)),
            peak_equity: Some(Decimal::new(100, 0)),
            source_observations: LossSourceObservationTimestamps::unobserved(),
        }
    }

    fn valid_recovery_evidence() -> LossGovernorManualRecoveryEvidence {
        LossGovernorManualRecoveryEvidence::new(
            "operator-1",
            "manual-recovery/evidence.json",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            1_100,
            128,
        )
        .expect("valid manual recovery evidence should build")
    }

    fn manual_recovery_target(
        current_state: TradingState,
        decision: &LossAdmissionDecision,
        snapshot: Option<&LossSnapshot>,
        evidence: Option<&LossGovernorManualRecoveryEvidence>,
    ) -> Option<TradingState> {
        next_loss_governor_manual_recovery_trading_state(LossGovernorManualRecoveryRequest {
            policy: &policy(
                LossGovernorTradingStateAction::Halted,
                LossGovernorTradingStateAction::Reducing,
            ),
            current_state,
            decision,
            snapshot,
            now_ns: 1_100,
            max_snapshot_age_ns: 1_000,
            evidence,
            max_evidence_path_bytes: 128,
        })
    }

    #[test]
    fn loss_breach_requests_configured_nt_state() {
        let decision = rejected(vec![LossHaltReason::DailyLossLimit]);

        let target = next_loss_governor_trading_state(
            &policy(
                LossGovernorTradingStateAction::Reducing,
                LossGovernorTradingStateAction::None,
            ),
            TradingState::Active,
            &decision,
        );

        assert_eq!(target, Some(TradingState::Reducing));
    }

    #[test]
    fn untrusted_snapshot_requests_configured_untrusted_state() {
        let decision = rejected(vec![LossHaltReason::StaleLossSnapshot]);

        let target = next_loss_governor_trading_state(
            &policy(
                LossGovernorTradingStateAction::None,
                LossGovernorTradingStateAction::Halted,
            ),
            TradingState::Active,
            &decision,
        );

        assert_eq!(target, Some(TradingState::Halted));
    }

    #[test]
    fn explicit_none_action_keeps_nt_state_unchanged() {
        let decision = rejected(vec![LossHaltReason::MaxDrawdownLimit]);

        let target = next_loss_governor_trading_state(
            &policy(
                LossGovernorTradingStateAction::None,
                LossGovernorTradingStateAction::None,
            ),
            TradingState::Active,
            &decision,
        );

        assert_eq!(target, None);
    }

    #[test]
    fn mixed_halt_reasons_use_strongest_configured_action() {
        let mixed_reasons = vec![
            LossHaltReason::StaleLossSnapshot,
            LossHaltReason::DailyLossLimit,
        ];

        let target = next_loss_governor_trading_state(
            &policy(
                LossGovernorTradingStateAction::Halted,
                LossGovernorTradingStateAction::None,
            ),
            TradingState::Active,
            &rejected(mixed_reasons.clone()),
        );

        assert_eq!(target, Some(TradingState::Halted));

        let target = next_loss_governor_trading_state(
            &policy(
                LossGovernorTradingStateAction::Reducing,
                LossGovernorTradingStateAction::Halted,
            ),
            TradingState::Active,
            &rejected(mixed_reasons),
        );

        assert_eq!(target, Some(TradingState::Halted));
    }

    #[test]
    fn manual_recovery_never_auto_activates_after_below_limit_snapshot() {
        let target = next_loss_governor_trading_state(
            &policy(
                LossGovernorTradingStateAction::Reducing,
                LossGovernorTradingStateAction::Halted,
            ),
            TradingState::Halted,
            &accepted(),
        );

        assert_eq!(target, None);
    }

    #[test]
    fn trading_state_changes_are_monotonic_and_idempotent() {
        let policy = policy(
            LossGovernorTradingStateAction::Halted,
            LossGovernorTradingStateAction::Reducing,
        );
        let breach = rejected(vec![LossHaltReason::DailyLossLimit]);
        let stale = rejected(vec![LossHaltReason::StaleLossSnapshot]);

        assert_eq!(
            next_loss_governor_trading_state(&policy, TradingState::Reducing, &breach),
            Some(TradingState::Halted)
        );
        assert_eq!(
            next_loss_governor_trading_state(&policy, TradingState::Halted, &stale),
            None
        );
        assert_eq!(
            next_loss_governor_trading_state(&policy, TradingState::Halted, &breach),
            None
        );
    }

    #[test]
    fn manual_recovery_evidence_clears_halted_state_after_accepted_fresh_snapshot() {
        let target = manual_recovery_target(
            TradingState::Halted,
            &accepted(),
            Some(&fresh_snapshot()),
            Some(&valid_recovery_evidence()),
        );

        assert_eq!(target, Some(TradingState::Active));
    }

    #[test]
    fn manual_recovery_evidence_clears_reducing_state_after_accepted_fresh_snapshot() {
        let target = manual_recovery_target(
            TradingState::Reducing,
            &accepted(),
            Some(&fresh_snapshot()),
            Some(&valid_recovery_evidence()),
        );

        assert_eq!(target, Some(TradingState::Active));
    }

    #[test]
    fn manual_recovery_without_evidence_or_when_already_active_is_noop() {
        assert_eq!(
            manual_recovery_target(
                TradingState::Halted,
                &accepted(),
                Some(&fresh_snapshot()),
                None
            ),
            None
        );
        assert_eq!(
            manual_recovery_target(
                TradingState::Active,
                &accepted(),
                Some(&fresh_snapshot()),
                Some(&valid_recovery_evidence()),
            ),
            None
        );
    }

    #[test]
    fn manual_recovery_rejects_loss_breach_missing_stale_future_or_unattributed_snapshot() {
        let rejected = rejected(vec![LossHaltReason::DailyLossLimit]);
        let evidence = valid_recovery_evidence();
        assert_eq!(
            manual_recovery_target(
                TradingState::Halted,
                &rejected,
                Some(&fresh_snapshot()),
                Some(&evidence),
            ),
            None
        );
        assert_eq!(
            manual_recovery_target(TradingState::Halted, &accepted(), None, Some(&evidence)),
            None
        );

        let mut stale = fresh_snapshot();
        stale.observed_at_ns = 99;
        assert_eq!(
            manual_recovery_target(
                TradingState::Halted,
                &accepted(),
                Some(&stale),
                Some(&evidence)
            ),
            None
        );

        let mut future = fresh_snapshot();
        future.observed_at_ns = 1_101;
        assert_eq!(
            manual_recovery_target(
                TradingState::Halted,
                &accepted(),
                Some(&future),
                Some(&evidence)
            ),
            None
        );

        let mut unattributed = fresh_snapshot();
        unattributed.source = " ".to_string();
        assert_eq!(
            manual_recovery_target(
                TradingState::Halted,
                &accepted(),
                Some(&unattributed),
                Some(&evidence),
            ),
            None
        );
    }

    #[test]
    fn manual_recovery_rejects_stale_future_or_pre_snapshot_evidence() {
        let stale_evidence = LossGovernorManualRecoveryEvidence::new(
            "operator-1",
            "manual-recovery/evidence.json",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            99,
            128,
        )
        .expect("structurally valid stale evidence should build");
        assert_eq!(
            manual_recovery_target(
                TradingState::Halted,
                &accepted(),
                Some(&fresh_snapshot()),
                Some(&stale_evidence),
            ),
            None
        );

        let future_evidence = LossGovernorManualRecoveryEvidence::new(
            "operator-1",
            "manual-recovery/evidence.json",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            1_101,
            128,
        )
        .expect("structurally valid future evidence should build");
        assert_eq!(
            manual_recovery_target(
                TradingState::Halted,
                &accepted(),
                Some(&fresh_snapshot()),
                Some(&future_evidence),
            ),
            None
        );

        let pre_snapshot_evidence = LossGovernorManualRecoveryEvidence::new(
            "operator-1",
            "manual-recovery/evidence.json",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            999,
            128,
        )
        .expect("structurally valid pre-snapshot evidence should build");
        assert_eq!(
            manual_recovery_target(
                TradingState::Halted,
                &accepted(),
                Some(&fresh_snapshot()),
                Some(&pre_snapshot_evidence),
            ),
            None
        );
    }

    #[test]
    fn manual_recovery_zero_caps_fail_closed() {
        let evidence = valid_recovery_evidence();
        assert_eq!(
            next_loss_governor_manual_recovery_trading_state(LossGovernorManualRecoveryRequest {
                policy: &policy(
                    LossGovernorTradingStateAction::Halted,
                    LossGovernorTradingStateAction::Reducing,
                ),
                current_state: TradingState::Halted,
                decision: &accepted(),
                snapshot: Some(&fresh_snapshot()),
                now_ns: 1_100,
                max_snapshot_age_ns: 0,
                evidence: Some(&evidence),
                max_evidence_path_bytes: 128,
            }),
            None
        );
        assert_eq!(
            LossGovernorManualRecoveryEvidence::new(
                "operator-1",
                "manual-recovery/evidence.json",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1_100,
                0,
            ),
            Err(LossGovernorManualRecoveryEvidenceError::InvalidEvidencePathLimit)
        );
    }

    #[test]
    fn manual_recovery_evidence_rejects_structural_failures() {
        assert_eq!(
            LossGovernorManualRecoveryEvidence::new(
                " ",
                "manual-recovery/evidence.json",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1_100,
                128,
            ),
            Err(LossGovernorManualRecoveryEvidenceError::MissingOperatorId)
        );
        assert_eq!(
            LossGovernorManualRecoveryEvidence::new(
                "operator-1",
                "manual-recovery/evidence.json",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1_100,
                128,
            ),
            Err(LossGovernorManualRecoveryEvidenceError::InvalidEvidenceSha256)
        );
        assert_eq!(
            LossGovernorManualRecoveryEvidence::new(
                "operator-1",
                "manual-recovery/evidence.json",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                1_100,
                128,
            ),
            Err(LossGovernorManualRecoveryEvidenceError::InvalidEvidenceSha256)
        );
        assert_eq!(
            LossGovernorManualRecoveryEvidence::new(
                "operator-1",
                "",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1_100,
                128,
            ),
            Err(LossGovernorManualRecoveryEvidenceError::EmptyEvidencePath)
        );
        assert_eq!(
            LossGovernorManualRecoveryEvidence::new(
                "operator-1",
                "manual-recovery/evidence.json",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1_100,
                "manual-recovery/evidence.json".len() - 1,
            ),
            Err(LossGovernorManualRecoveryEvidenceError::EvidencePathTooLong)
        );
        assert_eq!(
            LossGovernorManualRecoveryEvidence::new(
                "operator-1",
                "/manual-recovery/evidence.json",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1_100,
                128,
            ),
            Err(LossGovernorManualRecoveryEvidenceError::AbsoluteEvidencePath)
        );
        assert_eq!(
            LossGovernorManualRecoveryEvidence::new(
                "operator-1",
                "manual-recovery/../evidence.json",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1_100,
                128,
            ),
            Err(LossGovernorManualRecoveryEvidenceError::ParentEvidencePath)
        );
        assert_eq!(
            LossGovernorManualRecoveryEvidence::new(
                "operator-1",
                "manual-recovery/evidence.json",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                0,
                128,
            ),
            Err(LossGovernorManualRecoveryEvidenceError::MissingObservedAt)
        );
    }
}
