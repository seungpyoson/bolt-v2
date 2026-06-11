use bolt_v2::{
    bolt_v3_maker_quote_set::{QuoteSetBlockReason, QuoteSetInput, drive_binary_quote_set},
    bolt_v3_quote_lifecycle::{
        Leg, LegEvent, LegState, LifecycleAction, MarketAction, MarketQuote,
    },
    bolt_v3_quoting::{QuoteSide, QuoteTargetLeg, QuoteTargets},
    bolt_v3_requote_budget::RequoteBudget,
};

const EPSILON: f64 = 0.001;

#[test]
fn idle_quote_set_reserves_combined_collateral_before_submitting_both_legs() {
    let mut market = MarketQuote::new(false);
    let mut budget = RequoteBudget::new(2, 60_000, 0);

    let decision = drive_binary_quote_set(&mut market, &mut budget, quote_set_input(7.0));

    assert_eq!(
        decision.yes.control.action,
        Some(MarketAction::Leg {
            leg: Leg::Yes,
            action: LifecycleAction::Submit,
        })
    );
    assert_eq!(
        decision.no.control.action,
        Some(MarketAction::Leg {
            leg: Leg::No,
            action: LifecycleAction::Submit,
        })
    );
    assert_eq!(decision.yes.blocked_by, None);
    assert_eq!(decision.no.blocked_by, None);
    assert_eq!(market.leg_state(Leg::Yes), LegState::SubmitPending);
    assert_eq!(market.leg_state(Leg::No), LegState::SubmitPending);
    assert_eq!(budget.cost_in_window(), 2);
}

#[test]
fn quote_set_blocks_second_leg_when_combined_reservation_exceeds_collateral() {
    let mut market = MarketQuote::new(false);
    let mut budget = RequoteBudget::new(2, 60_000, 0);

    let decision = drive_binary_quote_set(&mut market, &mut budget, quote_set_input(6.0));

    assert_eq!(
        decision.yes.control.action,
        Some(MarketAction::Leg {
            leg: Leg::Yes,
            action: LifecycleAction::Submit,
        })
    );
    assert_eq!(decision.yes.blocked_by, None);
    assert_eq!(decision.no.control.action, None);
    assert_eq!(
        decision.no.blocked_by,
        Some(QuoteSetBlockReason::ReservationRejected)
    );
    assert_eq!(market.leg_state(Leg::Yes), LegState::SubmitPending);
    assert_eq!(market.leg_state(Leg::No), LegState::Idle);
    assert_eq!(budget.cost_in_window(), 1);
}

#[test]
fn quote_set_fails_closed_for_unsupported_quote_side_without_mutating_lifecycle() {
    let mut market = MarketQuote::new(false);
    let mut budget = RequoteBudget::new(2, 60_000, 0);
    let mut input = quote_set_input(7.0);
    input.targets.leg_a.side = QuoteSide::Sell;
    input.targets.leg_b.side = QuoteSide::Sell;

    let decision = drive_binary_quote_set(&mut market, &mut budget, input);

    assert_eq!(
        decision.yes.blocked_by,
        Some(QuoteSetBlockReason::UnsupportedQuoteSide)
    );
    assert_eq!(
        decision.no.blocked_by,
        Some(QuoteSetBlockReason::UnsupportedQuoteSide)
    );
    assert_eq!(decision.yes.control.action, None);
    assert_eq!(decision.no.control.action, None);
    assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
    assert_eq!(market.leg_state(Leg::No), LegState::Idle);
    assert_eq!(budget.cost_in_window(), 0);
}

#[test]
fn resting_cancel_requote_is_not_blocked_by_new_submit_collateral() {
    let mut market = resting_yes_market();
    let mut budget = RequoteBudget::new(1, 60_000, 0);
    let mut input = quote_set_input(0.0);
    input.targets.leg_a.price = 0.52;
    input.yes_resting_price = Some(0.50);

    let decision = drive_binary_quote_set(&mut market, &mut budget, input);

    assert_eq!(
        decision.yes.control.action,
        Some(MarketAction::Leg {
            leg: Leg::Yes,
            action: LifecycleAction::Cancel,
        })
    );
    assert_eq!(decision.yes.blocked_by, None);
    assert_eq!(market.leg_state(Leg::Yes), LegState::RequotePending);
    assert_eq!(budget.cost_in_window(), 1);
}

fn quote_set_input(available_collateral: f64) -> QuoteSetInput<'static> {
    QuoteSetInput {
        targets: QuoteTargets {
            leg_a: QuoteTargetLeg {
                side: QuoteSide::Buy,
                price: 0.40,
            },
            leg_b: QuoteTargetLeg {
                side: QuoteSide::Buy,
                price: 0.30,
            },
        },
        yes_quantity: 10.0,
        no_quantity: 10.0,
        yes_resting_price: None,
        no_resting_price: None,
        open_commitments: &[],
        max_fee_bps: 0.0,
        available_collateral,
        requote_threshold: 0.01,
        eps: EPSILON,
        now_ms: 1_000,
        action_cost: 1,
    }
}

fn resting_yes_market() -> MarketQuote {
    let mut market = MarketQuote::new(false);
    assert_eq!(
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: false,
            },
        ),
        Some(MarketAction::Leg {
            leg: Leg::Yes,
            action: LifecycleAction::Submit,
        })
    );
    assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Accepted), None);
    assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
    market
}
