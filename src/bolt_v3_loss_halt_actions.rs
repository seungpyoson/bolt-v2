use std::rc::Rc;

use nautilus_model::enums::TradingState;
use serde::Deserialize;

use crate::bolt_v3_loss_governor::{LossAdmissionDecision, LossHaltReason, LossSnapshot};

pub type LossGovernorHaltActionHandler = Rc<dyn Fn(Option<&LossSnapshot>, u64)>;

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
}

#[must_use]
pub fn loss_governor_trading_state_action_for_reasons(
    policy: &LossGovernorHaltActionPolicy,
    halt_reasons: &[LossHaltReason],
) -> LossGovernorTradingStateAction {
    if halt_reasons.is_empty() {
        return LossGovernorTradingStateAction::None;
    }

    if halt_reasons
        .iter()
        .any(|reason| matches!(reason, LossHaltReason::StaleLossSnapshot))
    {
        return policy.on_untrusted_snapshot_trading_state;
    }

    policy.on_loss_breach_trading_state
}

#[must_use]
pub fn next_loss_governor_trading_state(
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

    let target = loss_governor_trading_state_action_for_reasons(policy, &decision.halt_reasons)
        .as_trading_state()?;
    (trading_state_severity(target) > trading_state_severity(current_state)).then_some(target)
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

#[cfg(test)]
mod tests {
    use super::{
        LossGovernorHaltActionPolicy, LossGovernorRecoveryMode, LossGovernorTradingStateAction,
        next_loss_governor_trading_state,
    };
    use crate::bolt_v3_loss_governor::{LossAdmissionDecision, LossHaltReason};
    use nautilus_model::enums::TradingState;

    fn policy(
        on_loss_breach_trading_state: LossGovernorTradingStateAction,
        on_untrusted_snapshot_trading_state: LossGovernorTradingStateAction,
    ) -> LossGovernorHaltActionPolicy {
        LossGovernorHaltActionPolicy {
            on_loss_breach_trading_state,
            on_untrusted_snapshot_trading_state,
            recovery_mode: LossGovernorRecoveryMode::Manual,
        }
    }

    fn rejected(halt_reasons: Vec<LossHaltReason>) -> LossAdmissionDecision {
        LossAdmissionDecision {
            accepted: false,
            halt_reasons,
        }
    }

    fn accepted() -> LossAdmissionDecision {
        LossAdmissionDecision {
            accepted: true,
            halt_reasons: Vec::new(),
        }
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
}
