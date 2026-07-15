#![cfg(test)]

use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    enums::{LiquiditySide, OrderSide, OrderType},
    events::{OrderAccepted, OrderCancelRejected, OrderFilled},
    identifiers::{
        AccountId, ClientOrderId, InstrumentId, StrategyId, TradeId, TraderId, VenueOrderId,
    },
    types::{Currency, Price, Quantity},
};
use rust_decimal::Decimal;
use ustr::Ustr;

use super::super::{CompleteSetForwardingError, CompleteSetNtEventForwarder};
use crate::bolt_v3_basket_execution::{
    BoltV3BasketExecutionConfig, BoltV3BasketExecutionEvent, BoltV3BasketExecutionLegIntent,
    BoltV3BasketExecutionState, BoltV3BasketExecutionStatus,
    BoltV3BasketExecutionSubmitDisposition, BoltV3BasketFillSource, BoltV3BasketRepairPolicy,
    BoltV3BasketUnwindPolicy,
};

const EXECUTOR_ID: &str = "polymarket-main";
const SUBMIT_NOW_UNIX_MS: u64 = 2_000;
const SUBMIT_MAX_OBSERVATION_AGE_MS: u64 = 500;
const SUBMIT_OBSERVED_UNIX_MS: u64 = 1_750;

#[test]
fn actor_substrate_forwards_only_accepted_filled_and_cancel_rejected_events() {
    let mut forwarder = forwarder_with_submitted_basket();

    forwarder
        .forward_order_accepted(&accepted("COID-YES", "VOID-YES"))
        .expect("accepted should forward venue order identity");
    forwarder
        .forward_order_filled(&filled("COID-YES", "VOID-YES", "0.5", "0.44"))
        .expect("filled should forward a strategy fill");

    let basket = forwarder
        .test_basket("COID-YES")
        .expect("test basket should remain mapped");
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Partial);
    assert_eq!(
        basket.fill_sources(),
        vec![BoltV3BasketFillSource::Strategy]
    );

    forwarder
        .forward_order_cancel_rejected(&cancel_rejected("COID-YES", "cancel denied"))
        .expect("cancel rejection should forward to the shared executor");

    let basket = forwarder
        .test_basket("COID-YES")
        .expect("test basket should remain mapped");
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(basket.reservation_held());
    assert!(basket.unresolved_real_exposure());
    assert_eq!(forwarder.shell.forwarded_event_count(), 3);
    assert_eq!(forwarder.shell.failed_event_count(), 0);
}

#[test]
fn production_empty_lookup_rejects_and_records_unknown_executor_identity() {
    let mut forwarder = CompleteSetNtEventForwarder::new("complete-set-main", EXECUTOR_ID);

    let error = forwarder
        .forward_order_accepted(&accepted("COID-YES", "VOID-YES"))
        .expect_err("production-empty basket lookup must fail closed");

    assert_eq!(
        error,
        CompleteSetForwardingError::UnknownExecutorLegIdentity {
            execution_client_id: EXECUTOR_ID.to_string(),
            client_order_id: "COID-YES".to_string(),
        }
    );
    assert_eq!(forwarder.shell.forwarded_event_count(), 0);
    assert_eq!(forwarder.shell.failed_event_count(), 1);
    assert_eq!(forwarder.shell.last_failure(), Some(&error));
    assert!(forwarder.baskets_by_handle.is_empty());
    assert!(forwarder.basket_handle_by_client_order_id.is_empty());
}

#[test]
fn two_concurrent_baskets_on_one_executor_route_by_client_order_id() {
    let mut forwarder = CompleteSetNtEventForwarder::new("complete-set-main", EXECUTOR_ID);
    forwarder
        .insert_test_basket(
            "basket-a",
            submitted_basket_with_ids("basket-a", "COID-A-YES", "COID-A-NO", "OL-A"),
        )
        .expect("first basket identities should index");
    forwarder
        .insert_test_basket(
            "basket-b",
            submitted_basket_with_ids("basket-b", "COID-B-YES", "COID-B-NO", "OL-B"),
        )
        .expect("second basket identities should index without overwriting the first");

    forwarder
        .forward_order_accepted(&accepted("COID-A-YES", "VOID-A-YES"))
        .expect("basket A accepted event should route by client order id");
    forwarder
        .forward_order_filled(&filled("COID-B-YES", "VOID-B-YES", "0.5", "0.44"))
        .expect("basket B fill should route by client order id");
    forwarder
        .forward_order_cancel_rejected(&cancel_rejected("COID-A-YES", "cancel denied"))
        .expect("basket A cancel rejection should route by client order id");

    assert_eq!(
        forwarder
            .test_basket("COID-A-YES")
            .expect("basket A should remain indexed")
            .status(),
        BoltV3BasketExecutionStatus::Stuck
    );
    assert_eq!(
        forwarder
            .test_basket("COID-B-YES")
            .expect("basket B should remain indexed")
            .status(),
        BoltV3BasketExecutionStatus::Partial
    );
    assert_eq!(forwarder.baskets_by_handle.len(), 2);
    assert_eq!(forwarder.basket_handle_by_client_order_id.len(), 4);
    assert_eq!(forwarder.shell.forwarded_event_count(), 3);
    assert_eq!(forwarder.shell.failed_event_count(), 0);
}

#[test]
fn forwarding_unknown_identity_matrix_is_deterministic_and_fail_closed() {
    assert_unknown_forwarding("accepted", |forwarder| {
        forwarder.forward_order_accepted(&accepted("COID-UNKNOWN", "VOID-UNKNOWN"))
    });
    assert_unknown_forwarding("fill", |forwarder| {
        forwarder.forward_order_filled(&filled("COID-UNKNOWN", "VOID-UNKNOWN", "0.5", "0.44"))
    });
    assert_unknown_forwarding("cancel rejected", |forwarder| {
        forwarder.forward_order_cancel_rejected(&cancel_rejected("COID-UNKNOWN", "cancel denied"))
    });
}

#[test]
fn test_basket_index_rejects_client_order_collisions_without_overwrite() {
    let mut forwarder = forwarder_with_submitted_basket();
    let error = forwarder
        .insert_test_basket(
            "basket-collision",
            submitted_basket_with_ids(
                "basket-collision",
                "COID-YES",
                "COID-COLLISION-NO",
                "OL-COLLISION",
            ),
        )
        .expect_err("duplicate client order identity must not overwrite its basket handle");

    assert_eq!(
        error,
        CompleteSetForwardingError::DuplicateExecutorLegIdentity("COID-YES".to_string())
    );
    assert_eq!(forwarder.baskets_by_handle.len(), 1);
    assert_eq!(forwarder.basket_handle_by_client_order_id.len(), 2);
    assert!(forwarder.test_basket("COID-COLLISION-NO").is_none());
}

#[test]
fn duplicate_overfill_and_nonpositive_fills_are_not_deduplicated_or_rejected() {
    let mut duplicate = forwarder_with_submitted_basket();
    let repeated_fill = filled("COID-YES", "VOID-YES", "0.6", "0.44");
    duplicate
        .forward_order_filled(&repeated_fill)
        .expect("first duplicate-key fill should apply");
    duplicate
        .forward_order_filled(&repeated_fill)
        .expect("second duplicate-key fill should apply without dedupe");
    assert_eq!(
        duplicate
            .test_basket("COID-YES")
            .expect("basket should remain mapped")
            .status(),
        BoltV3BasketExecutionStatus::Stuck
    );
    assert_eq!(duplicate.shell.forwarded_event_count(), 2);
    assert_eq!(duplicate.shell.failed_event_count(), 0);

    for (case, quantity, price) in [
        ("zero quantity", "0", "0.44"),
        ("zero cost", "0.5", "0"),
        ("direct overfill", "1.1", "0.44"),
    ] {
        let mut forwarder = forwarder_with_submitted_basket();
        forwarder
            .forward_order_filled(&filled("COID-YES", "VOID-YES", quantity, price))
            .unwrap_or_else(|error| panic!("{case} must be state-machine Ok: {error}"));
        let basket = forwarder
            .test_basket("COID-YES")
            .expect("basket should remain mapped");
        assert_eq!(
            basket.status(),
            BoltV3BasketExecutionStatus::Stuck,
            "{case}"
        );
        assert!(basket.reservation_held(), "{case}");
        assert!(basket.unresolved_real_exposure(), "{case}");
        assert_eq!(forwarder.shell.forwarded_event_count(), 1, "{case}");
        assert_eq!(forwarder.shell.failed_event_count(), 0, "{case}");
    }

    let mut negative = forwarder_with_submitted_basket();
    negative
        .forward_event(
            "COID-YES",
            BoltV3BasketExecutionEvent::LegFill {
                client_order_id: "COID-YES".to_string(),
                venue_order_id: Some("VOID-YES".to_string()),
                quantity: dec("-0.5"),
                cost: dec("-0.22"),
                source: BoltV3BasketFillSource::Strategy,
            },
        )
        .expect("negative fill should remain state-machine Ok");
    let basket = negative
        .test_basket("COID-YES")
        .expect("negative-fill basket should remain mapped");
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(basket.reservation_held());
    assert!(basket.unresolved_real_exposure());
    assert_eq!(negative.shell.forwarded_event_count(), 1);
    assert_eq!(negative.shell.failed_event_count(), 0);
}

#[test]
fn hook_ownership_and_shadow_prohibition_remain_explicit() {
    let strategy_source = include_str!("../mod.rs");
    let data_actor = strategy_source
        .split("impl DataActor for CompleteSetArbitrage")
        .nth(1)
        .and_then(|source| source.split("nautilus_strategy!").next())
        .expect("DataActor implementation should precede strategy macro");
    assert!(data_actor.contains("fn on_order_filled"));

    let strategy_hooks = strategy_source
        .split("nautilus_strategy!(CompleteSetArbitrage, {")
        .nth(1)
        .and_then(|source| source.split("});").next())
        .expect("strategy hook block should be present");
    assert!(strategy_hooks.contains("fn on_order_accepted"));
    assert!(strategy_hooks.contains("fn on_order_cancel_rejected"));
    assert!(!strategy_hooks.contains("fn on_order_filled"));

    let envelope_source = include_str!("../../../bolt_v3_validate/strategy_envelope.rs");
    assert!(envelope_source.contains("validate_complete_set_activation_is_shadow_only"));
    assert!(envelope_source.contains("runtime.order_execution_mode must be shadow"));
}

fn forwarder_with_submitted_basket() -> CompleteSetNtEventForwarder {
    let mut forwarder = CompleteSetNtEventForwarder::new("complete-set-main", EXECUTOR_ID);
    forwarder
        .insert_test_basket("basket-1", submitted_basket())
        .expect("test basket identities should index");
    forwarder
}

fn assert_unknown_forwarding(
    case: &str,
    forward: impl FnOnce(&mut CompleteSetNtEventForwarder) -> Result<(), CompleteSetForwardingError>,
) {
    let mut forwarder = forwarder_with_submitted_basket();
    let error = forward(&mut forwarder).expect_err(case);
    assert_eq!(
        error,
        CompleteSetForwardingError::UnknownExecutorLegIdentity {
            execution_client_id: EXECUTOR_ID.to_string(),
            client_order_id: "COID-UNKNOWN".to_string(),
        },
        "{case}"
    );
    assert_eq!(forwarder.shell.forwarded_event_count(), 0, "{case}");
    assert_eq!(forwarder.shell.failed_event_count(), 1, "{case}");
    assert_eq!(forwarder.shell.last_failure(), Some(&error), "{case}");
    assert_eq!(
        forwarder
            .test_basket("COID-YES")
            .expect("basket should remain mapped")
            .status(),
        BoltV3BasketExecutionStatus::Submitting,
        "{case}"
    );
}

fn submitted_basket() -> BoltV3BasketExecutionState {
    submitted_basket_with_ids("basket-1", "COID-YES", "COID-NO", "OL-FORWARDING")
}

fn submitted_basket_with_ids(
    basket_id: &str,
    yes_client_order_id: &str,
    no_client_order_id: &str,
    order_list_id: &str,
) -> BoltV3BasketExecutionState {
    let mut basket = BoltV3BasketExecutionState::candidate(
        basket_id,
        "complete-set-main",
        EXECUTOR_ID,
        vec![
            ("YES", "YES.POLYMARKET", dec("1.0")),
            ("NO", "NO.POLYMARKET", dec("1.0")),
        ],
        vec![vec![dec("1.0"), dec("0.0")], vec![dec("0.0"), dec("1.0")]],
        dec("0.10"),
        dec("1000"),
        BoltV3BasketExecutionConfig {
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
        },
    )
    .expect("candidate should build");
    basket
        .apply_event(BoltV3BasketExecutionEvent::ReservationPersisted)
        .expect("reservation should persist");
    basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            order_list_id,
            leg_intents(yes_client_order_id, no_client_order_id),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("submit command should persist client order ids");
    basket
}

fn leg_intents(
    yes_client_order_id: &str,
    no_client_order_id: &str,
) -> Vec<BoltV3BasketExecutionLegIntent> {
    vec![
        BoltV3BasketExecutionLegIntent {
            leg_id: "YES".to_string(),
            instrument_id: "YES.POLYMARKET".to_string(),
            venue: "POLYMARKET".to_string(),
            client_order_id: yes_client_order_id.to_string(),
            side: OrderSide::Buy,
            quantity: dec("1.0"),
            notional: dec("0.44"),
            observed_unix_ms: SUBMIT_OBSERVED_UNIX_MS,
        },
        BoltV3BasketExecutionLegIntent {
            leg_id: "NO".to_string(),
            instrument_id: "NO.POLYMARKET".to_string(),
            venue: "POLYMARKET".to_string(),
            client_order_id: no_client_order_id.to_string(),
            side: OrderSide::Buy,
            quantity: dec("1.0"),
            notional: dec("0.46"),
            observed_unix_ms: SUBMIT_OBSERVED_UNIX_MS,
        },
    ]
}

fn accepted(client_order_id: &str, venue_order_id: &str) -> OrderAccepted {
    OrderAccepted::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("complete-set-main"),
        InstrumentId::from("YES.POLYMARKET"),
        ClientOrderId::from(client_order_id),
        VenueOrderId::from(venue_order_id),
        AccountId::from("POLYMARKET-001"),
        UUID4::new(),
        UnixNanos::from(1_000_u64),
        UnixNanos::from(1_000_u64),
        false,
    )
}

fn filled(client_order_id: &str, venue_order_id: &str, quantity: &str, price: &str) -> OrderFilled {
    OrderFilled::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("complete-set-main"),
        InstrumentId::from("YES.POLYMARKET"),
        ClientOrderId::from(client_order_id),
        VenueOrderId::from(venue_order_id),
        AccountId::from("POLYMARKET-001"),
        TradeId::from("TRADE-001"),
        OrderSide::Buy,
        OrderType::Limit,
        Quantity::from(quantity),
        Price::from(price),
        Currency::USDC(),
        LiquiditySide::Taker,
        UUID4::new(),
        UnixNanos::from(1_000_u64),
        UnixNanos::from(1_000_u64),
        false,
        None,
        None,
    )
}

fn cancel_rejected(client_order_id: &str, reason: &str) -> OrderCancelRejected {
    OrderCancelRejected::new(
        TraderId::from("TRADER-001"),
        StrategyId::from("complete-set-main"),
        InstrumentId::from("YES.POLYMARKET"),
        ClientOrderId::from(client_order_id),
        Ustr::from(reason),
        UUID4::new(),
        UnixNanos::from(1_000_u64),
        UnixNanos::from(1_000_u64),
        false,
        Some(VenueOrderId::from("VOID-YES")),
        Some(AccountId::from("POLYMARKET-001")),
    )
}

fn dec(value: &str) -> Decimal {
    value.parse().expect("decimal fixture should parse")
}
