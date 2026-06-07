//! Pure maker reserved-collateral helpers.

use crate::bolt_v3_numeric::{UNIT_F64, is_non_negative_finite, is_positive_finite};
use crate::bolt_v3_submit_admission::{
    base_quantity_admission_notional, fee_inclusive_admission_notional,
};
use rust_decimal::{Decimal, prelude::FromPrimitive};

/// One resting, in-flight, or candidate binary buy commitment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuyCommitment {
    price: f64,
    quantity: f64,
}

impl BuyCommitment {
    pub fn new(price: f64, quantity: f64) -> Self {
        Self { price, quantity }
    }

    fn admission_notional(self) -> Option<Decimal> {
        let price = binary_price_decimal(self.price)?;
        let quantity = positive_decimal(self.quantity)?;
        Some(base_quantity_admission_notional(price, quantity))
    }
}

/// Inputs for a per-market maker reservation decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReservationRequest<'a> {
    pub open: &'a [BuyCommitment],
    pub candidate: BuyCommitment,
    pub max_fee_bps: f64,
    pub available_collateral: f64,
}

/// Per-market reservation gate verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationDecision {
    Admit,
    Reject,
}

/// Fee-inclusive worst-case reservation for simultaneous fill of all commitments.
pub fn worst_case_reservation(commitments: &[BuyCommitment], max_fee_bps: f64) -> Option<Decimal> {
    let base = base_reservation(commitments)?;
    let max_fee_bps = non_negative_decimal(max_fee_bps)?;
    Some(fee_inclusive_admission_notional(base, max_fee_bps))
}

/// Decide whether adding `candidate` keeps total reservation within collateral.
pub fn evaluate_reservation(request: ReservationRequest<'_>) -> ReservationDecision {
    let Some(available_collateral) = positive_decimal(request.available_collateral) else {
        return ReservationDecision::Reject;
    };
    let Some(open_base) = base_reservation(request.open) else {
        return ReservationDecision::Reject;
    };
    let Some(candidate_base) = request.candidate.admission_notional() else {
        return ReservationDecision::Reject;
    };
    let Some(combined_base) = open_base.checked_add(candidate_base) else {
        return ReservationDecision::Reject;
    };
    let Some(max_fee_bps) = non_negative_decimal(request.max_fee_bps) else {
        return ReservationDecision::Reject;
    };

    if fee_inclusive_admission_notional(combined_base, max_fee_bps) > available_collateral {
        ReservationDecision::Reject
    } else {
        ReservationDecision::Admit
    }
}

fn base_reservation(commitments: &[BuyCommitment]) -> Option<Decimal> {
    commitments
        .iter()
        .try_fold(Decimal::ZERO, |sum, commitment| {
            sum.checked_add(commitment.admission_notional()?)
        })
}

fn binary_price_decimal(value: f64) -> Option<Decimal> {
    if !(is_positive_finite(value) && value <= UNIT_F64) {
        return None;
    }
    positive_decimal(value)
}

fn positive_decimal(value: f64) -> Option<Decimal> {
    if !is_positive_finite(value) {
        return None;
    }
    Decimal::from_f64(value).filter(|decimal| *decimal > Decimal::ZERO)
}

fn non_negative_decimal(value: f64) -> Option<Decimal> {
    if !is_non_negative_finite(value) {
        return None;
    }
    Decimal::from_f64(value).filter(|decimal| *decimal >= Decimal::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_submit_admission::{
        base_quantity_admission_notional, fee_inclusive_admission_notional,
    };
    use rust_decimal::{Decimal, prelude::FromPrimitive};
    use std::str::FromStr;

    fn dec(value: &str) -> Decimal {
        Decimal::from_str(value).expect("test decimal should parse")
    }

    #[test]
    fn reservation_accepts_a_valid_buy_commitment_within_available_budget() {
        let decision = evaluate_reservation(ReservationRequest {
            open: &[BuyCommitment::new(0.40, 5.0)],
            candidate: BuyCommitment::new(0.25, 4.0),
            max_fee_bps: 25.0,
            available_collateral: 3.01,
        });

        assert_eq!(decision, ReservationDecision::Admit);
    }

    #[test]
    fn reservation_rejects_insufficient_budget() {
        let decision = evaluate_reservation(ReservationRequest {
            open: &[BuyCommitment::new(0.40, 5.0)],
            candidate: BuyCommitment::new(0.25, 4.0),
            max_fee_bps: 25.0,
            available_collateral: 3.0,
        });

        assert_eq!(decision, ReservationDecision::Reject);
    }

    #[test]
    fn reservation_matches_submit_admission_fee_inclusive_parity() {
        let open = [BuyCommitment::new(0.40, 5.0)];
        let candidate = BuyCommitment::new(0.25, 4.0);

        let reservation = worst_case_reservation(&[open[0], candidate], 25.0)
            .expect("reservation should be valid");

        let expected_base = base_quantity_admission_notional(dec("0.40"), dec("5.0"))
            + base_quantity_admission_notional(dec("0.25"), dec("4.0"));
        let expected = fee_inclusive_admission_notional(expected_base, dec("25.0"));
        assert_eq!(reservation, expected);
    }

    #[test]
    fn reservation_uses_explicit_f64_to_decimal_boundary_conversion() {
        let price = 0.1_f64 + 0.2_f64;
        let quantity = 3.0_f64;

        let reservation = worst_case_reservation(&[BuyCommitment::new(price, quantity)], 0.0)
            .expect("reservation should be valid");

        let expected_price = Decimal::from_f64(price).expect("price should convert");
        let expected_quantity = Decimal::from_f64(quantity).expect("quantity should convert");
        assert_eq!(
            reservation,
            base_quantity_admission_notional(expected_price, expected_quantity)
        );
    }

    #[test]
    fn invalid_non_finite_negative_or_zero_inputs_fail_closed() {
        let invalid_commitments = [
            BuyCommitment::new(f64::NAN, 5.0),
            BuyCommitment::new(f64::INFINITY, 5.0),
            BuyCommitment::new(-0.10, 5.0),
            BuyCommitment::new(0.0, 5.0),
            BuyCommitment::new(1.01, 5.0),
            BuyCommitment::new(0.40, f64::NAN),
            BuyCommitment::new(0.40, -5.0),
            BuyCommitment::new(0.40, 0.0),
        ];

        for commitment in invalid_commitments {
            assert_eq!(worst_case_reservation(&[commitment], 0.0), None);
            assert_eq!(
                evaluate_reservation(ReservationRequest {
                    open: &[],
                    candidate: commitment,
                    max_fee_bps: 0.0,
                    available_collateral: 10.0,
                }),
                ReservationDecision::Reject
            );
        }

        let valid = [BuyCommitment::new(0.40, 5.0)];
        assert_eq!(worst_case_reservation(&valid, f64::NAN), None);
        assert_eq!(worst_case_reservation(&valid, -1.0), None);
        assert_eq!(
            evaluate_reservation(ReservationRequest {
                open: &[],
                candidate: valid[0],
                max_fee_bps: 0.0,
                available_collateral: 0.0,
            }),
            ReservationDecision::Reject
        );
    }

    #[test]
    fn unrepresentable_boundary_values_fail_closed() {
        assert_eq!(
            worst_case_reservation(&[BuyCommitment::new(0.40, f64::MAX)], 0.0),
            None
        );
        assert_eq!(
            worst_case_reservation(&[BuyCommitment::new(0.40, 5.0)], f64::MAX),
            None
        );
        assert_eq!(
            evaluate_reservation(ReservationRequest {
                open: &[],
                candidate: BuyCommitment::new(0.40, 5.0),
                max_fee_bps: 0.0,
                available_collateral: f64::MAX,
            }),
            ReservationDecision::Reject
        );
    }
}
