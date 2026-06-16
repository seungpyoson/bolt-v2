//! Shared maker risk-governor action selection.
//!
//! This module owns only the generic decision of which quote-lifecycle action a
//! maker risk state implies. It does not submit orders, read venue names, or
//! construct crossing reduce orders. Until a real reduce-order compiler exists,
//! hard-flat requests are fail-closed: resting quotes are drained, and the
//! decision carries a block reason proving the active flatten is not silently
//! claimed as implemented.

use crate::{
    bolt_v3_loss_governor::{LossAdmissionDecision, LossHaltReason},
    bolt_v3_quote_lifecycle::{Leg, MarketAction, MarketQuote},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerRiskMode {
    SoftHold,
    CancelOnly,
    ReduceOnly { leg: Leg },
    HardFlat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerRiskBlockReason {
    HardFlatReduceUnsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerRiskDecision {
    pub mode: MakerRiskMode,
    pub action: Option<MarketAction>,
    pub blocked_by: Option<MakerRiskBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerLossRiskPolicy {
    pub on_loss_breach: MakerRiskMode,
    pub on_untrusted_snapshot: MakerRiskMode,
}

#[must_use]
pub fn maker_risk_mode_for_loss_decision(
    policy: &MakerLossRiskPolicy,
    decision: &LossAdmissionDecision,
) -> MakerRiskMode {
    if decision.accepted {
        return MakerRiskMode::SoftHold;
    }

    decision
        .halt_reasons
        .iter()
        .map(|reason| mode_for_loss_reason(policy, *reason))
        .max_by_key(|mode| risk_mode_rank(*mode))
        .unwrap_or(MakerRiskMode::CancelOnly)
}

#[must_use]
pub fn apply_maker_risk_mode(market: &mut MarketQuote, mode: MakerRiskMode) -> MakerRiskDecision {
    match mode {
        MakerRiskMode::SoftHold => decision(mode, None, None),
        MakerRiskMode::CancelOnly => decision(mode, market.drain(), None),
        MakerRiskMode::ReduceOnly { leg } => decision(mode, market.cancel_one_side(leg), None),
        MakerRiskMode::HardFlat => decision(
            mode,
            market.drain(),
            Some(MakerRiskBlockReason::HardFlatReduceUnsupported),
        ),
    }
}

fn mode_for_loss_reason(policy: &MakerLossRiskPolicy, reason: LossHaltReason) -> MakerRiskMode {
    match reason {
        LossHaltReason::StaleLossSnapshot => policy.on_untrusted_snapshot,
        LossHaltReason::PerTradeLossLimit
        | LossHaltReason::DailyLossLimit
        | LossHaltReason::RollingLossLimit
        | LossHaltReason::MaxDrawdownLimit => policy.on_loss_breach,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MakerRiskRank {
    SoftHold,
    CancelOnly,
    ReduceOnly,
    HardFlat,
}

fn risk_mode_rank(mode: MakerRiskMode) -> MakerRiskRank {
    match mode {
        MakerRiskMode::SoftHold => MakerRiskRank::SoftHold,
        MakerRiskMode::CancelOnly => MakerRiskRank::CancelOnly,
        MakerRiskMode::ReduceOnly { .. } => MakerRiskRank::ReduceOnly,
        MakerRiskMode::HardFlat => MakerRiskRank::HardFlat,
    }
}

fn decision(
    mode: MakerRiskMode,
    action: Option<MarketAction>,
    blocked_by: Option<MakerRiskBlockReason>,
) -> MakerRiskDecision {
    MakerRiskDecision {
        mode,
        action,
        blocked_by,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_quote_lifecycle::{LegEvent, LegState, MarketState};

    fn resting_market() -> MarketQuote {
        let mut market = MarketQuote::new(false);
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: false,
            },
        );
        market.on_leg_event(
            Leg::No,
            LegEvent::QuoteTrigger {
                requote_needed: false,
            },
        );
        market.on_leg_event(Leg::Yes, LegEvent::Accepted);
        market.on_leg_event(Leg::No, LegEvent::Accepted);
        assert_eq!(market.market_state(), MarketState::Quoting);
        market
    }

    fn accepted_loss_decision() -> LossAdmissionDecision {
        LossAdmissionDecision {
            accepted: true,
            halt_reasons: Vec::new(),
        }
    }

    fn rejected_loss_decision(reason: LossHaltReason) -> LossAdmissionDecision {
        LossAdmissionDecision {
            accepted: false,
            halt_reasons: vec![reason],
        }
    }

    #[test]
    fn accepted_loss_decision_soft_holds_without_mutation() {
        let policy = MakerLossRiskPolicy {
            on_loss_breach: MakerRiskMode::HardFlat,
            on_untrusted_snapshot: MakerRiskMode::CancelOnly,
        };
        let mut market = resting_market();

        let mode = maker_risk_mode_for_loss_decision(&policy, &accepted_loss_decision());
        let decision = apply_maker_risk_mode(&mut market, mode);

        assert_eq!(mode, MakerRiskMode::SoftHold);
        assert_eq!(decision.action, None);
        assert_eq!(decision.blocked_by, None);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
        assert_eq!(market.leg_state(Leg::No), LegState::Resting);
    }

    #[test]
    fn stale_loss_snapshot_drains_quotes_when_policy_is_cancel_only() {
        let policy = MakerLossRiskPolicy {
            on_loss_breach: MakerRiskMode::HardFlat,
            on_untrusted_snapshot: MakerRiskMode::CancelOnly,
        };
        let mut market = resting_market();

        let mode = maker_risk_mode_for_loss_decision(
            &policy,
            &rejected_loss_decision(LossHaltReason::StaleLossSnapshot),
        );
        let decision = apply_maker_risk_mode(&mut market, mode);

        assert_eq!(mode, MakerRiskMode::CancelOnly);
        assert_eq!(decision.action, Some(MarketAction::CancelAllBothLegs));
        assert_eq!(decision.blocked_by, None);
        assert_eq!(market.market_state(), MarketState::Draining);
    }

    #[test]
    fn loss_breach_hard_flat_drains_and_blocks_until_reduce_path_exists() {
        let policy = MakerLossRiskPolicy {
            on_loss_breach: MakerRiskMode::HardFlat,
            on_untrusted_snapshot: MakerRiskMode::CancelOnly,
        };
        let mut market = resting_market();

        let mode = maker_risk_mode_for_loss_decision(
            &policy,
            &rejected_loss_decision(LossHaltReason::DailyLossLimit),
        );
        let decision = apply_maker_risk_mode(&mut market, mode);

        assert_eq!(mode, MakerRiskMode::HardFlat);
        assert_eq!(decision.action, Some(MarketAction::CancelAllBothLegs));
        assert_eq!(
            decision.blocked_by,
            Some(MakerRiskBlockReason::HardFlatReduceUnsupported)
        );
        assert_eq!(market.market_state(), MarketState::Draining);
    }

    #[test]
    fn reduce_only_cancels_only_the_configured_leg() {
        let mut market = resting_market();

        let decision =
            apply_maker_risk_mode(&mut market, MakerRiskMode::ReduceOnly { leg: Leg::Yes });

        assert_eq!(
            decision.action,
            Some(MarketAction::CancelAllOneSide { leg: Leg::Yes })
        );
        assert_eq!(decision.blocked_by, None);
        assert_eq!(market.leg_state(Leg::Yes), LegState::CancelPending);
        assert_eq!(market.leg_state(Leg::No), LegState::Resting);
    }

    #[test]
    fn idle_hard_flat_still_reports_unsupported_reduce_path() {
        let mut market = MarketQuote::new(false);

        let decision = apply_maker_risk_mode(&mut market, MakerRiskMode::HardFlat);

        assert_eq!(decision.action, None);
        assert_eq!(
            decision.blocked_by,
            Some(MakerRiskBlockReason::HardFlatReduceUnsupported)
        );
        assert_eq!(market.market_state(), MarketState::Idle);
    }
}
