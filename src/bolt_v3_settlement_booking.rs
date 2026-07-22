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
        OrderLifecycleOutcome, OrderLifecycleTransition, SettlementBookingErrorReason,
        SettlementFact,
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
pub enum SettlementPositionOrigin {
    Live,
    RecoveryBootstrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSettlementEligibilityReason {
    MarketExpired,
    RecoveryUnknownInterval,
}

impl TerminalSettlementEligibilityReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::MarketExpired => stringify!(market_expired),
            Self::RecoveryUnknownInterval => stringify!(recovery_unknown_interval),
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
    origin: SettlementPositionOrigin,
    observed_at_ns: u64,
) -> Result<TerminalSettlementEligibility> {
    let observed_at_ms = observed_at_ns / NANOS_PER_MILLI_U64;
    let reason = if position
        .interval_end_ms
        .is_some_and(|interval_end_ms| observed_at_ms >= interval_end_ms)
    {
        TerminalSettlementEligibilityReason::MarketExpired
    } else if origin == SettlementPositionOrigin::RecoveryBootstrap
        && position.interval_end_ms.is_none()
    {
        TerminalSettlementEligibilityReason::RecoveryUnknownInterval
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

pub fn recovered_terminal_settlement_eligibility(
    position: &SettlementPositionKey,
    observed_at_ns: u64,
) -> Result<TerminalSettlementEligibility> {
    terminal_settlement_eligibility(
        position,
        SettlementPositionOrigin::RecoveryBootstrap,
        observed_at_ns,
    )
    .map_err(|_| {
        anyhow::anyhow!(
            "recovered terminal settlement is ineligible before market expiry for position {}",
            position.position_id
        )
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
    origin: SettlementPositionOrigin,
    reason: SettlementBookingErrorReason,
    detail: String,
    observed_at_ns: u64,
    existing_booking_error_keys: &BTreeSet<String>,
) -> Result<Option<SettlementBookingErrorTransition>> {
    if existing_booking_error_keys.contains(&position.settlement_key) {
        return Ok(None);
    }
    let eligibility = terminal_settlement_eligibility(position, origin, observed_at_ns)?;
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
    pub booking_error_keys: BTreeSet<String>,
    pub terminal_settlement_keys: BTreeSet<String>,
    pub settled_evidence: Vec<SettlementFact>,
}

pub fn recover_settlement_bootstrap(
    capability: Option<&SettlementCapability>,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> Result<SettlementRecoveryDelta> {
    let Some(recovery) = capability.and_then(SettlementCapability::recovery) else {
        return Ok(SettlementRecoveryDelta {
            settled_position_keys: BTreeSet::new(),
            booking_error_keys: BTreeSet::new(),
            terminal_settlement_keys: BTreeSet::new(),
            settled_evidence: Vec::new(),
        });
    };
    Ok(SettlementRecoveryDelta {
        settled_position_keys: recovery
            .settlements()
            .keys()
            .filter(|key| recovery_scope_settlement_keys.contains(*key))
            .cloned()
            .collect(),
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
        settled_evidence: recovery
            .settlements()
            .iter()
            .filter(|(key, _)| recovery_scope_settlement_keys.contains(*key))
            .map(|(_, fact)| fact.clone())
            .collect(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementRecoveryEntryDecision {
    Continue,
    Flat,
    EnterBlindCacheProbe,
    EnterBlindMultipleOpenPositions {
        count: usize,
    },
    ApplyPriorBookingError {
        eligibility: TerminalSettlementEligibility,
        canonical_evidence_already_durable: bool,
    },
    EnterBlindSettlementRecovery {
        transition: OrderLifecycleTransition,
        outcome: OrderLifecycleOutcome,
        detail: String,
    },
}

pub fn bootstrap_recovery_from_cache(
    cache_probe_succeeded: bool,
    open_position: Option<&SettlementPositionKey>,
    open_position_count: usize,
    observed_at_ns: u64,
    booking_error_keys: &BTreeSet<String>,
    terminal_settlement_keys: &BTreeSet<String>,
) -> SettlementRecoveryEntryDecision {
    if !cache_probe_succeeded {
        return SettlementRecoveryEntryDecision::EnterBlindCacheProbe;
    }
    if open_position_count == 0 {
        return SettlementRecoveryEntryDecision::Flat;
    }
    if open_position_count > 1 {
        return SettlementRecoveryEntryDecision::EnterBlindMultipleOpenPositions {
            count: open_position_count,
        };
    }
    let Some(position) = open_position else {
        return enter_blind_settlement_recovery("single cache position projection is missing");
    };
    if !booking_error_keys.contains(&position.settlement_key) {
        return SettlementRecoveryEntryDecision::Continue;
    }
    match recovered_terminal_settlement_eligibility(position, observed_at_ns) {
        Ok(eligibility) => SettlementRecoveryEntryDecision::ApplyPriorBookingError {
            canonical_evidence_already_durable: terminal_settlement_keys
                .contains(&position.settlement_key),
            eligibility,
        },
        Err(error) => enter_blind_settlement_recovery(error),
    }
}

pub fn enter_blind_settlement_recovery(
    error: impl std::fmt::Display,
) -> SettlementRecoveryEntryDecision {
    SettlementRecoveryEntryDecision::EnterBlindSettlementRecovery {
        transition: OrderLifecycleTransition::SettlementEvidenceRecoveryBlocked,
        outcome: OrderLifecycleOutcome::BlindRecovery,
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booking_error_transition_is_terminal_and_idempotent() {
        let position = SettlementPositionKey {
            settlement_key: "product:position".to_string(),
            position_id: "position".to_string(),
            interval_end_ms: Some(20),
        };
        let transition = record_settlement_booking_error(
            &position,
            SettlementPositionOrigin::Live,
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
                SettlementPositionOrigin::Live,
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
        assert_eq!(
            bootstrap_recovery_from_cache(false, None, 0, 0, &BTreeSet::new(), &BTreeSet::new(),),
            SettlementRecoveryEntryDecision::EnterBlindCacheProbe
        );
        assert!(matches!(
            enter_blind_settlement_recovery("durable evidence unreadable"),
            SettlementRecoveryEntryDecision::EnterBlindSettlementRecovery {
                transition: OrderLifecycleTransition::SettlementEvidenceRecoveryBlocked,
                outcome: OrderLifecycleOutcome::BlindRecovery,
                ..
            }
        ));
    }
}
