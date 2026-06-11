use bolt_v2::{
    bolt_v3_maker_quote_control::{QuoteControlBlockReason, QuoteControlInput, drive_quote_leg},
    bolt_v3_quote_lifecycle::{
        Leg, LegEvent, LegState, LifecycleAction, MarketAction, MarketQuote,
    },
    bolt_v3_requote_budget::RequoteBudget,
};

const EPSILON: f64 = 0.001;

#[test]
fn quote_trigger_is_budget_gated_before_mutating_lifecycle() {
    let mut market = MarketQuote::new(false);
    let mut exhausted = RequoteBudget::new(0, 60_000, 0);

    let denied = drive_quote_leg(
        &mut market,
        &mut exhausted,
        QuoteControlInput {
            leg: Leg::Yes,
            desired_price: 0.42,
            resting_price: None,
            requote_threshold: 0.01,
            eps: EPSILON,
            now_ms: 1_000,
            action_cost: 1,
        },
    );

    assert_eq!(denied.action, None);
    assert_eq!(
        denied.blocked_by,
        Some(QuoteControlBlockReason::RequoteBudgetExhausted)
    );
    assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
    assert_eq!(exhausted.in_window(), 0);

    let mut allowed = RequoteBudget::new(1, 60_000, 0);
    let accepted = drive_quote_leg(
        &mut market,
        &mut allowed,
        QuoteControlInput {
            leg: Leg::Yes,
            desired_price: 0.42,
            resting_price: None,
            requote_threshold: 0.01,
            eps: EPSILON,
            now_ms: 1_001,
            action_cost: 1,
        },
    );

    assert_eq!(
        accepted.action,
        Some(MarketAction::Leg {
            leg: Leg::Yes,
            action: LifecycleAction::Submit,
        })
    );
    assert_eq!(accepted.blocked_by, None);
    assert_eq!(market.leg_state(Leg::Yes), LegState::SubmitPending);
    assert_eq!(allowed.cost_in_window(), 1);
}

#[test]
fn resting_quote_requotes_only_when_target_moves_beyond_threshold() {
    let mut market = resting_market();
    let mut budget = RequoteBudget::new(2, 60_000, 0);

    let unchanged = drive_quote_leg(
        &mut market,
        &mut budget,
        QuoteControlInput {
            leg: Leg::Yes,
            desired_price: 0.504,
            resting_price: Some(0.50),
            requote_threshold: 0.01,
            eps: EPSILON,
            now_ms: 1_000,
            action_cost: 1,
        },
    );

    assert_eq!(unchanged.action, None);
    assert_eq!(unchanged.blocked_by, None);
    assert!(!unchanged.requote_needed);
    assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
    assert_eq!(budget.cost_in_window(), 0);

    let moved = drive_quote_leg(
        &mut market,
        &mut budget,
        QuoteControlInput {
            leg: Leg::Yes,
            desired_price: 0.52,
            resting_price: Some(0.50),
            requote_threshold: 0.01,
            eps: EPSILON,
            now_ms: 1_001,
            action_cost: 1,
        },
    );

    assert_eq!(
        moved.action,
        Some(MarketAction::Leg {
            leg: Leg::Yes,
            action: LifecycleAction::Cancel,
        })
    );
    assert_eq!(moved.blocked_by, None);
    assert!(moved.requote_needed);
    assert_eq!(market.leg_state(Leg::Yes), LegState::RequotePending);
    assert_eq!(budget.cost_in_window(), 1);
}

#[test]
fn resting_quote_without_resting_price_fails_closed() {
    let mut market = resting_market();
    let mut budget = RequoteBudget::new(1, 60_000, 0);

    let decision = drive_quote_leg(
        &mut market,
        &mut budget,
        QuoteControlInput {
            leg: Leg::Yes,
            desired_price: 0.52,
            resting_price: None,
            requote_threshold: 0.01,
            eps: EPSILON,
            now_ms: 1_000,
            action_cost: 1,
        },
    );

    assert_eq!(decision.action, None);
    assert_eq!(
        decision.blocked_by,
        Some(QuoteControlBlockReason::MissingRestingPrice)
    );
    assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
    assert_eq!(budget.cost_in_window(), 0);
}

#[test]
fn invalid_quote_inputs_fail_closed_without_mutating_state() {
    let mut market = MarketQuote::new(false);
    let mut budget = RequoteBudget::new(1, 60_000, 0);

    let decision = drive_quote_leg(
        &mut market,
        &mut budget,
        QuoteControlInput {
            leg: Leg::Yes,
            desired_price: f64::NAN,
            resting_price: None,
            requote_threshold: 0.01,
            eps: EPSILON,
            now_ms: 1_000,
            action_cost: 1,
        },
    );

    assert_eq!(decision.action, None);
    assert_eq!(
        decision.blocked_by,
        Some(QuoteControlBlockReason::InvalidDesiredPrice)
    );
    assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
    assert_eq!(budget.cost_in_window(), 0);
}

fn resting_market() -> MarketQuote {
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
