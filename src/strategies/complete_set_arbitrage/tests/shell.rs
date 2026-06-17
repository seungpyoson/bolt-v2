use nautilus_model::enums::OrderSide;
use rust_decimal::Decimal;

use super::super::{
    CompleteSetArbitrageShell, CompleteSetSettlementPolicy, forward_settlement_signal,
    live_settlement_policy,
};
use crate::bolt_v3_basket_execution::{
    BoltV3BasketExecutionConfig, BoltV3BasketExecutionEvent, BoltV3BasketExecutionLegIntent,
    BoltV3BasketExecutionState, BoltV3BasketExecutionStatus,
    BoltV3BasketExecutionSubmitDisposition, BoltV3BasketFillSource, BoltV3BasketRepairPolicy,
    BoltV3BasketUnwindPolicy,
};

const SUBMIT_NOW_UNIX_MS: u64 = 2_000;
const SUBMIT_MAX_OBSERVATION_AGE_MS: u64 = 500;
const SUBMIT_OBSERVED_UNIX_MS: u64 = 1_750;

#[test]
fn shell_forwards_events_into_shared_executor_without_submit_mechanics() {
    let contract = crate::bolt_v3_order_execution::nt_order_management_contract();
    assert!(!contract.order_list_type.is_empty());
    assert!(!contract.submit_order_list_type.is_empty());

    let mut shell = CompleteSetArbitrageShell::new("complete-set-main");
    let policy = shell.mechanics_policy();
    assert!(policy.shared_basket_execution_owns_admission);
    assert!(policy.shared_basket_execution_owns_venue_mutation);
    assert!(policy.shared_basket_execution_owns_fillability);
    assert!(policy.shared_basket_execution_owns_repair_unwind);

    let mut basket = reserved_basket();
    basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-SHELL",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("submit command should persist client order ids before fills");
    basket
        .apply_event(BoltV3BasketExecutionEvent::VenueOrderId {
            client_order_id: "COID-YES".to_string(),
            venue_order_id: "VOID-YES".to_string(),
        })
        .expect("venue id should apply before shell forwards fill");
    shell
        .forward_executor_event(
            &mut basket,
            BoltV3BasketExecutionEvent::LegFill {
                client_order_id: "COID-YES".to_string(),
                venue_order_id: Some("VOID-YES".to_string()),
                quantity: dec("1.0"),
                cost: dec("0.44"),
                source: BoltV3BasketFillSource::Strategy,
            },
        )
        .expect("shell should forward event to shared executor");

    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Partial);
    assert_eq!(shell.forwarded_event_count(), 1);
}

#[test]
fn shell_keeps_live_settlement_disabled_until_task0_signal_is_reachable() {
    assert_eq!(
        live_settlement_policy(),
        CompleteSetSettlementPolicy::RejectUntilReachableNtSignal
    );

    let mut basket = reserved_basket();
    let rejected = forward_settlement_signal(&mut basket)
        .expect_err("settlement signal remains gated by Task 0 disposition");
    assert_eq!(
        rejected.to_string(),
        "live settlement signal is not reachable"
    );
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Reserved);
}

fn reserved_basket() -> BoltV3BasketExecutionState {
    let config = BoltV3BasketExecutionConfig {
        repair: BoltV3BasketRepairPolicy {
            max_retries: 2,
            max_book_age_ms: 250,
            max_slippage_bps: 50,
            max_depth_levels: 4,
            allow_unwind_when_repair_denied: true,
        },
        unwind: BoltV3BasketUnwindPolicy {
            max_retries: 2,
            max_book_age_ms: 250,
            max_slippage_bps: 50,
            max_depth_levels: 4,
        },
    };
    let mut basket = BoltV3BasketExecutionState::candidate(
        "basket-1",
        "complete-set-main",
        "polymarket-main",
        vec![
            ("YES", "YES.POLYMARKET", dec("1.0")),
            ("NO", "NO.POLYMARKET", dec("1.0")),
        ],
        vec![vec![dec("1.0"), dec("0.0")], vec![dec("0.0"), dec("1.0")]],
        dec("0.10"),
        dec("1000"),
        config,
    )
    .expect("candidate should build");
    basket
        .apply_event(BoltV3BasketExecutionEvent::ReservationPersisted)
        .expect("reservation should persist");
    basket
}

fn leg_intents() -> Vec<BoltV3BasketExecutionLegIntent> {
    vec![
        BoltV3BasketExecutionLegIntent {
            leg_id: "YES".to_string(),
            instrument_id: "YES.POLYMARKET".to_string(),
            venue: "POLYMARKET".to_string(),
            client_order_id: "COID-YES".to_string(),
            side: OrderSide::Buy,
            quantity: dec("1.0"),
            notional: dec("0.44"),
            observed_unix_ms: SUBMIT_OBSERVED_UNIX_MS,
        },
        BoltV3BasketExecutionLegIntent {
            leg_id: "NO".to_string(),
            instrument_id: "NO.POLYMARKET".to_string(),
            venue: "POLYMARKET".to_string(),
            client_order_id: "COID-NO".to_string(),
            side: OrderSide::Buy,
            quantity: dec("1.0"),
            notional: dec("0.46"),
            observed_unix_ms: SUBMIT_OBSERVED_UNIX_MS,
        },
    ]
}

fn dec(value: &str) -> Decimal {
    value.parse().expect("decimal fixture should parse")
}
