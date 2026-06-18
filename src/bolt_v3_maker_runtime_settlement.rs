//! Runtime bridge from maker resolution evidence to shared settlement accounting.
//!
//! Callers supply the already-resolved market-family key, the reference close,
//! the strike, and the maker inventory lots. This module derives the terminal
//! payout through the market-family binding, then settles through the shared
//! binary settlement primitive used by live and backtest callers.

use crate::{
    bolt_v3_maker_settlement::{
        BinarySettlementLot, BinarySettlementPayout, BinarySettlementResult, settle_binary_lots,
    },
    bolt_v3_market_families::maker_settlement_payout_from_reference_prices_for_family,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerRuntimeSettlementInput<'a> {
    pub family_key: &'a str,
    pub reference_close_price: f64,
    pub strike_price: f64,
    pub lots: &'a [BinarySettlementLot],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerRuntimeSettlementBlockReason {
    ReferencePayoutUnavailable,
    LotSettlementFailed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerRuntimeSettlementDecision {
    pub payout: Option<BinarySettlementPayout>,
    pub result: Option<BinarySettlementResult>,
    pub blocked_by: Option<MakerRuntimeSettlementBlockReason>,
}

#[must_use]
pub fn settle_maker_runtime_reference_prices(
    input: MakerRuntimeSettlementInput<'_>,
) -> MakerRuntimeSettlementDecision {
    let Some(payout) = maker_settlement_payout_from_reference_prices_for_family(
        input.family_key,
        input.reference_close_price,
        input.strike_price,
    ) else {
        return blocked(MakerRuntimeSettlementBlockReason::ReferencePayoutUnavailable);
    };

    let Some(result) = settle_binary_lots(payout, input.lots) else {
        return MakerRuntimeSettlementDecision {
            payout: Some(payout),
            result: None,
            blocked_by: Some(MakerRuntimeSettlementBlockReason::LotSettlementFailed),
        };
    };

    MakerRuntimeSettlementDecision {
        payout: Some(payout),
        result: Some(result),
        blocked_by: None,
    }
}

fn blocked(reason: MakerRuntimeSettlementBlockReason) -> MakerRuntimeSettlementDecision {
    MakerRuntimeSettlementDecision {
        payout: None,
        result: None,
        blocked_by: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bolt_v3_market_families::{static_binary_event, updown},
        bolt_v3_quote_lifecycle::Leg,
        bolt_v3_quoting::QuoteSide,
    };

    const EPSILON: f64 = 1e-9;

    #[test]
    fn updown_reference_prices_settle_lots_through_shared_primitive() {
        let lots = [
            BinarySettlementLot {
                leg: Leg::Yes,
                side: QuoteSide::Buy,
                quantity: 4.0,
                entry_price: 0.42,
            },
            BinarySettlementLot {
                leg: Leg::No,
                side: QuoteSide::Buy,
                quantity: 3.0,
                entry_price: 0.37,
            },
        ];

        let decision = settle_maker_runtime_reference_prices(MakerRuntimeSettlementInput {
            family_key: updown::KEY,
            reference_close_price: 101.0,
            strike_price: 100.0,
            lots: &lots,
        });

        assert_eq!(decision.blocked_by, None);
        let payout = decision.payout.expect("updown payout should resolve");
        assert_eq!(payout.leg_payout(Leg::Yes), 1.0);
        assert_eq!(payout.leg_payout(Leg::No), 0.0);
        let result = decision.result.expect("valid lots should settle");
        assert!((result.realized_pnl - 1.21).abs() < EPSILON);
        assert!((result.closed_quantity - 7.0).abs() < EPSILON);
        assert!((result.closed_net_yes - 1.0).abs() < EPSILON);
    }

    #[test]
    fn tie_at_strike_resolves_to_yes_payout() {
        let decision = settle_maker_runtime_reference_prices(MakerRuntimeSettlementInput {
            family_key: updown::KEY,
            reference_close_price: 100.0,
            strike_price: 100.0,
            lots: &[],
        });

        let payout = decision.payout.expect("tie should resolve");
        assert_eq!(payout.leg_payout(Leg::Yes), 1.0);
        assert_eq!(payout.leg_payout(Leg::No), 0.0);
        assert_eq!(decision.result, Some(BinarySettlementResult::flat()));
    }

    #[test]
    fn unsupported_family_or_invalid_reference_prices_fail_closed_before_settlement() {
        for input in [
            MakerRuntimeSettlementInput {
                family_key: static_binary_event::KEY,
                reference_close_price: 101.0,
                strike_price: 100.0,
                lots: &[],
            },
            MakerRuntimeSettlementInput {
                family_key: updown::KEY,
                reference_close_price: f64::NAN,
                strike_price: 100.0,
                lots: &[],
            },
        ] {
            let decision = settle_maker_runtime_reference_prices(input);

            assert_eq!(decision.payout, None);
            assert_eq!(decision.result, None);
            assert_eq!(
                decision.blocked_by,
                Some(MakerRuntimeSettlementBlockReason::ReferencePayoutUnavailable)
            );
        }
    }

    #[test]
    fn invalid_lot_fails_closed_after_payout_derivation_without_partial_result() {
        let lots = [BinarySettlementLot {
            leg: Leg::Yes,
            side: QuoteSide::Buy,
            quantity: f64::NAN,
            entry_price: 0.42,
        }];

        let decision = settle_maker_runtime_reference_prices(MakerRuntimeSettlementInput {
            family_key: updown::KEY,
            reference_close_price: 101.0,
            strike_price: 100.0,
            lots: &lots,
        });

        assert_eq!(
            decision.payout.map(|payout| payout.leg_payout(Leg::Yes)),
            Some(1.0)
        );
        assert_eq!(decision.result, None);
        assert_eq!(
            decision.blocked_by,
            Some(MakerRuntimeSettlementBlockReason::LotSettlementFailed)
        );
    }
}
