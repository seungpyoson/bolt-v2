//! Strategy-neutral settlement booking and recovery decisions.
//!
//! Strategies project private position/exposure state into these typed inputs,
//! then apply returned deltas to their own state. This module owns no strategy
//! actor, NT cache, or strategy-private exposure type.

use std::collections::BTreeSet;

use anyhow::Result;

use crate::{
    bolt_v3_binary_settlement::{BinarySettlementLot, BinarySettlementResult},
    bolt_v3_binary_settlement_runtime::{
        BinaryRuntimeSettlementInput, settle_binary_runtime_reference_prices,
    },
    bolt_v3_current_evidence::{
        BookingRecoveryFacts, OrderLifecycleOutcome, OrderLifecycleTransition,
        RecoveredSettlementOutcome, SettlementBookingErrorReason, SettlementFact,
        SettlementRecoveryFacts, TerminalSettlementFact,
    },
    bolt_v3_numeric::NANOS_PER_MILLI_U64,
    bolt_v3_strategy_context::SettlementCapability,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementPositionKey {
    pub settlement_key: String,
    pub position_id: String,
    pub interval_end_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSettlementEligibilityReason {
    MarketExpired,
}

impl TerminalSettlementEligibilityReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::MarketExpired => stringify!(market_expired),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSettlementEligibility {
    pub settlement_key: String,
    pub position_id: String,
    pub observed_at_ns: u64,
    pub reason: TerminalSettlementEligibilityReason,
}

pub fn terminal_settlement_eligibility(
    position: &SettlementPositionKey,
    observed_at_ns: u64,
) -> Result<TerminalSettlementEligibility> {
    let observed_at_ms = observed_at_ns / NANOS_PER_MILLI_U64;
    let reason = if position
        .interval_end_ms
        .is_some_and(|interval_end_ms| observed_at_ms >= interval_end_ms)
    {
        TerminalSettlementEligibilityReason::MarketExpired
    } else {
        anyhow::bail!(
            "terminal settlement is ineligible for live-manageable or nonterminal position {}",
            position.position_id
        )
    };
    Ok(TerminalSettlementEligibility {
        settlement_key: position.settlement_key.clone(),
        position_id: position.position_id.clone(),
        observed_at_ns,
        reason,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementTerminalKeyDelta {
    pub settlement_key: String,
    pub insert_booking_error_key: bool,
    pub insert_terminal_key: bool,
    pub remove_close_fetch_attempt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementBookingErrorTransition {
    pub eligibility: TerminalSettlementEligibility,
    pub reason: SettlementBookingErrorReason,
    pub detail: String,
    pub key_delta: SettlementTerminalKeyDelta,
}

pub fn record_settlement_booking_error(
    position: &SettlementPositionKey,
    reason: SettlementBookingErrorReason,
    detail: String,
    observed_at_ns: u64,
    existing_booking_error_keys: &BTreeSet<String>,
) -> Result<Option<SettlementBookingErrorTransition>> {
    if existing_booking_error_keys.contains(&position.settlement_key) {
        return Ok(None);
    }
    let eligibility = terminal_settlement_eligibility(position, observed_at_ns)?;
    Ok(Some(SettlementBookingErrorTransition {
        key_delta: SettlementTerminalKeyDelta {
            settlement_key: position.settlement_key.clone(),
            insert_booking_error_key: true,
            insert_terminal_key: true,
            remove_close_fetch_attempt: true,
        },
        eligibility,
        reason,
        detail,
    }))
}

#[derive(Clone, Copy)]
pub struct ResolutionSettlementInput<'a> {
    pub position: &'a SettlementPositionKey,
    pub resolution_ts_ns: u64,
    pub family_key: &'a str,
    pub reference_close_price: f64,
    pub strike_price: Option<f64>,
    pub lot: Option<BinarySettlementLot>,
    pub market_id_present: bool,
    pub capability: Option<&'a SettlementCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementBookingSkipReason {
    ResolutionTickMismatch,
    AlreadyTerminal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolutionSettlementBooking {
    pub settlement_key: String,
    pub lot: BinarySettlementLot,
    pub payout_per_share: f64,
    pub result: BinarySettlementResult,
    pub key_delta: SettlementTerminalKeyDelta,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolutionSettlementDecision {
    Skip(SettlementBookingSkipReason),
    BookingError {
        reason: SettlementBookingErrorReason,
        detail: String,
    },
    Book(ResolutionSettlementBooking),
}

#[must_use]
pub fn try_book_resolution_settlement(
    input: ResolutionSettlementInput<'_>,
    settled_position_keys: &BTreeSet<String>,
    booking_error_keys: &BTreeSet<String>,
) -> ResolutionSettlementDecision {
    let resolution_ts_ms = input.resolution_ts_ns / NANOS_PER_MILLI_U64;
    let Some(interval_end_ms) = input.position.interval_end_ms else {
        return booking_error(
            SettlementBookingErrorReason::SettlementInputInvalid,
            "settlement input missing interval end",
        );
    };
    if interval_end_ms != resolution_ts_ms {
        return ResolutionSettlementDecision::Skip(
            SettlementBookingSkipReason::ResolutionTickMismatch,
        );
    }
    if settled_position_keys.contains(&input.position.settlement_key)
        || booking_error_keys.contains(&input.position.settlement_key)
    {
        return ResolutionSettlementDecision::Skip(SettlementBookingSkipReason::AlreadyTerminal);
    }
    let Some(lot) = input.lot else {
        return booking_error(
            SettlementBookingErrorReason::SettlementInputInvalid,
            "settlement input missing outcome side",
        );
    };
    let Some(strike_price) = input.strike_price else {
        return booking_error(
            SettlementBookingErrorReason::SettlementInputInvalid,
            "settlement input missing strike price",
        );
    };
    let decision = settle_binary_runtime_reference_prices(BinaryRuntimeSettlementInput {
        family_key: input.family_key,
        reference_close_price: input.reference_close_price,
        strike_price,
        lots: &[lot],
    });
    let Some(result) = decision.result else {
        return booking_error(
            SettlementBookingErrorReason::SettlementBlocked,
            format!("settlement blocked by {:?}", decision.blocked_by),
        );
    };
    let Some(payout) = decision.payout else {
        return booking_error(
            SettlementBookingErrorReason::SettlementBlocked,
            "settlement output missing payout",
        );
    };
    if input
        .capability
        .and_then(SettlementCapability::currency)
        .is_none()
    {
        return booking_error(
            SettlementBookingErrorReason::SettlementInputInvalid,
            "settlement input missing configured settlement currency",
        );
    }
    if !input.market_id_present {
        return booking_error(
            SettlementBookingErrorReason::SettlementInputInvalid,
            "settlement input missing market id",
        );
    }
    if input
        .capability
        .and_then(SettlementCapability::runtime_sink)
        .is_some()
        && input
            .capability
            .and_then(SettlementCapability::account_id)
            .is_none()
    {
        return booking_error(
            SettlementBookingErrorReason::SettlementInputInvalid,
            "settlement input missing configured settlement account id",
        );
    }
    ResolutionSettlementDecision::Book(ResolutionSettlementBooking {
        settlement_key: input.position.settlement_key.clone(),
        lot,
        payout_per_share: payout.leg_payout(lot.leg),
        result,
        key_delta: SettlementTerminalKeyDelta {
            settlement_key: input.position.settlement_key.clone(),
            insert_booking_error_key: false,
            insert_terminal_key: false,
            remove_close_fetch_attempt: true,
        },
    })
}

fn booking_error(
    reason: SettlementBookingErrorReason,
    detail: impl Into<String>,
) -> ResolutionSettlementDecision {
    ResolutionSettlementDecision::BookingError {
        reason,
        detail: detail.into(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettlementRecoveryDelta {
    pub settled_position_keys: BTreeSet<String>,
    pub terminal_settlement_keys: BTreeSet<String>,
    pub settled_evidence: Vec<SettlementFact>,
    pub terminal_evidence: Vec<TerminalSettlementFact>,
}

pub fn recover_settlement_facts(
    recovery: Option<&SettlementRecoveryFacts>,
    strategy_id: &str,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> Result<SettlementRecoveryDelta> {
    let Some(recovery) = recovery else {
        return Ok(SettlementRecoveryDelta {
            settled_position_keys: BTreeSet::new(),
            terminal_settlement_keys: BTreeSet::new(),
            settled_evidence: Vec::new(),
            terminal_evidence: Vec::new(),
        });
    };
    for (key, outcome) in recovery.outcomes() {
        let outcome_strategy_id = match outcome {
            RecoveredSettlementOutcome::Successful(fact) => &fact.strategy_id,
            RecoveredSettlementOutcome::BookingTerminal(fact) => &fact.booking_error.strategy_id,
        };
        if outcome_strategy_id == strategy_id && !recovery_scope_settlement_keys.contains(key) {
            anyhow::bail!(
                "durable settlement outcome `{key}` for strategy `{strategy_id}` is absent from the authoritative NT recovery scope"
            );
        }
    }
    let scoped_outcomes = recovery.outcomes().iter().filter(|(key, outcome)| {
        let outcome_strategy_id = match outcome {
            RecoveredSettlementOutcome::Successful(fact) => &fact.strategy_id,
            RecoveredSettlementOutcome::BookingTerminal(fact) => &fact.booking_error.strategy_id,
        };
        outcome_strategy_id == strategy_id && recovery_scope_settlement_keys.contains(*key)
    });
    Ok(SettlementRecoveryDelta {
        settled_position_keys: scoped_outcomes
            .clone()
            .filter(|(_, outcome)| matches!(outcome, RecoveredSettlementOutcome::Successful(_)))
            .map(|(key, _)| key.clone())
            .collect(),
        terminal_settlement_keys: scoped_outcomes
            .clone()
            .filter(|(_, outcome)| {
                matches!(outcome, RecoveredSettlementOutcome::BookingTerminal(_))
            })
            .map(|(key, _)| key.clone())
            .collect(),
        settled_evidence: scoped_outcomes
            .clone()
            .filter_map(|(_, outcome)| match outcome {
                RecoveredSettlementOutcome::Successful(fact) => Some(fact.clone()),
                RecoveredSettlementOutcome::BookingTerminal(_) => None,
            })
            .collect(),
        terminal_evidence: scoped_outcomes
            .filter_map(|(_, outcome)| match outcome {
                RecoveredSettlementOutcome::Successful(_) => None,
                RecoveredSettlementOutcome::BookingTerminal(fact) => Some(fact.clone()),
            })
            .collect(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookingRecoveryDelta {
    pub booking_error_keys: BTreeSet<String>,
    pub terminal_settlement_keys: BTreeSet<String>,
}

#[must_use]
pub fn recover_booking_facts(
    recovery: Option<&BookingRecoveryFacts>,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> BookingRecoveryDelta {
    let Some(recovery) = recovery else {
        return BookingRecoveryDelta {
            booking_error_keys: BTreeSet::new(),
            terminal_settlement_keys: BTreeSet::new(),
        };
    };
    BookingRecoveryDelta {
        booking_error_keys: recovery
            .booking_error_keys()
            .intersection(recovery_scope_settlement_keys)
            .cloned()
            .collect(),
        terminal_settlement_keys: recovery
            .terminal_settlement_keys()
            .intersection(recovery_scope_settlement_keys)
            .cloned()
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementRecoveryFailure {
    pub transition: OrderLifecycleTransition,
    pub outcome: OrderLifecycleOutcome,
    pub detail: String,
}

pub fn enter_blind_settlement_recovery(error: impl std::fmt::Display) -> SettlementRecoveryFailure {
    SettlementRecoveryFailure {
        transition: OrderLifecycleTransition::SettlementEvidenceRecoveryBlocked,
        outcome: OrderLifecycleOutcome::BlindRecovery,
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_current_evidence::{EvidenceOrderSide, OutcomeSide};

    fn settlement_fact(strategy_id: &str, settlement_key: &str) -> SettlementFact {
        SettlementFact {
            strategy_id: strategy_id.to_string(),
            settlement_key: settlement_key.to_string(),
            market_id: "market-1".to_string(),
            position_id: "position-1".to_string(),
            instrument_id: "YES-USD.POLYMARKET".to_string(),
            product_id: "product-1".to_string(),
            outcome_side: OutcomeSide::Up,
            entry_order_side: EvidenceOrderSide::Buy,
            quantity: "1".to_string(),
            entry_price: "0.4".to_string(),
            family_key: "family-1".to_string(),
            strike_price: "100".to_string(),
            resolution_instrument_id: "resolution-1".to_string(),
            resolution_ts_event_ns: 1,
            reference_close_price: "101".to_string(),
            payout_per_share: "1".to_string(),
            terminal_value: "1".to_string(),
            realized_pnl: "0.6".to_string(),
            settlement_currency: "USD".to_string(),
        }
    }

    #[test]
    fn booking_error_transition_is_terminal_and_idempotent() {
        let position = SettlementPositionKey {
            settlement_key: "product:position".to_string(),
            position_id: "position".to_string(),
            interval_end_ms: Some(20),
        };
        let transition = record_settlement_booking_error(
            &position,
            SettlementBookingErrorReason::ResolutionFeedMissing,
            "missing".to_string(),
            20 * NANOS_PER_MILLI_U64,
            &BTreeSet::new(),
        )
        .expect("expired position should be eligible")
        .expect("new key should transition");
        assert_eq!(
            transition.eligibility.reason,
            TerminalSettlementEligibilityReason::MarketExpired
        );
        assert!(transition.key_delta.insert_booking_error_key);
        assert!(transition.key_delta.insert_terminal_key);

        let existing = BTreeSet::from([position.settlement_key.clone()]);
        assert_eq!(
            record_settlement_booking_error(
                &position,
                SettlementBookingErrorReason::ResolutionFeedMissing,
                "duplicate".to_string(),
                20 * NANOS_PER_MILLI_U64,
                &existing,
            )
            .expect("duplicate is a no-op"),
            None
        );
    }

    #[test]
    fn degraded_recovery_inputs_enter_blind_recovery() {
        let failure = enter_blind_settlement_recovery("durable evidence unreadable");
        assert_eq!(
            failure.transition,
            OrderLifecycleTransition::SettlementEvidenceRecoveryBlocked
        );
        assert_eq!(failure.outcome, OrderLifecycleOutcome::BlindRecovery);
    }

    #[test]
    fn durable_strategy_settlement_outside_nt_scope_fails_closed() {
        let recovery = SettlementRecoveryFacts::from_settlement_for_test(settlement_fact(
            "strategy-1",
            "settlement-1",
        ));

        let error = recover_settlement_facts(Some(&recovery), "strategy-1", &BTreeSet::new())
            .expect_err("a durable strategy outcome missing from NT scope must fail closed");

        assert!(
            error
                .to_string()
                .contains("absent from the authoritative NT recovery scope")
        );
    }

    #[test]
    fn foreign_strategy_settlement_outside_nt_scope_is_ignored() {
        let recovery = SettlementRecoveryFacts::from_settlement_for_test(settlement_fact(
            "strategy-2",
            "settlement-1",
        ));

        let delta = recover_settlement_facts(Some(&recovery), "strategy-1", &BTreeSet::new())
            .expect("foreign strategy evidence is outside this strategy's authority");

        assert!(delta.settled_evidence.is_empty());
        assert!(delta.terminal_evidence.is_empty());
    }
}
