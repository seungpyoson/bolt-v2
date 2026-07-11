use std::fs;
use std::sync::Arc;

use bolt_v2::{
    bolt_v3_basket_execution::{
        BoltV3BasketExecutionConfig, BoltV3BasketExecutionError, BoltV3BasketExecutionEvent,
        BoltV3BasketExecutionLegIntent, BoltV3BasketExecutionState, BoltV3BasketExecutionStatus,
        BoltV3BasketExecutionSubmitDisposition, BoltV3BasketFillSource,
        BoltV3BasketNtSubmitCommand, BoltV3BasketRepairInput, BoltV3BasketRepairOutcome,
        BoltV3BasketRepairPolicy, BoltV3BasketRestartReport, BoltV3BasketSettlementSignal,
        BoltV3BasketUnwindInput, BoltV3BasketUnwindOutcome, BoltV3BasketUnwindPolicy,
        BoltV3ExecutableRepairLeg, BoltV3ExternalReportClass, REPAIR_EDGE_INEQUALITY,
        executor_event_integration_contract, nt_order_management_contract,
        trip_stuck_basket_kill_switch,
    },
    bolt_v3_basket_store::{
        BoltV3BasketRecoveryReason, BoltV3BasketRecoveryState, BoltV3BasketStore,
    },
    bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter,
    bolt_v3_kill_switch::{KillSwitchHaltTriggerKind, KillSwitchState, KillSwitchStateKind},
    bolt_v3_kill_switch_store::KillSwitchStore,
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
        BoltV3SubmitLifecyclePolicy,
    },
};
use nautilus_model::enums::OrderSide;
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;

const SUBMIT_NOW_UNIX_MS: u64 = 2_000;
const SUBMIT_MAX_OBSERVATION_AGE_MS: u64 = 500;
const SUBMIT_OBSERVED_UNIX_MS: u64 = 1_750;
const TEST_MAX_STATE_FILE_BYTES: u64 = 65_536;

#[test]
fn complete_and_partial_fill_transitions_hold_and_release_reservation_explicitly() {
    let mut partial = reserved_basket();
    partial
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-COMPLETE",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("same-venue NT submit command should build");

    partial
        .apply_event(BoltV3BasketExecutionEvent::LegFill {
            client_order_id: "COID-YES".to_string(),
            venue_order_id: Some("VOID-YES".to_string()),
            quantity: dec("1.0"),
            cost: dec("0.44"),
            source: BoltV3BasketFillSource::Strategy,
        })
        .expect("first fill should apply");

    assert_eq!(partial.status(), BoltV3BasketExecutionStatus::Partial);
    assert!(partial.reservation_held());
    assert!(partial.unresolved_real_exposure());

    partial
        .apply_event(BoltV3BasketExecutionEvent::LegFill {
            client_order_id: "COID-NO".to_string(),
            venue_order_id: Some("VOID-NO".to_string()),
            quantity: dec("1.0"),
            cost: dec("0.46"),
            source: BoltV3BasketFillSource::Strategy,
        })
        .expect("second fill should apply");

    assert_eq!(partial.status(), BoltV3BasketExecutionStatus::Complete);
    assert!(partial.reservation_held());
    assert!(!partial.unresolved_real_exposure());

    partial
        .apply_event(BoltV3BasketExecutionEvent::TerminalClose)
        .expect("terminal close should apply after complete fill");
    assert_eq!(partial.status(), BoltV3BasketExecutionStatus::Closed);
    assert!(!partial.reservation_held());
}

#[test]
fn leg_fill_rejects_non_positive_quantity_and_negative_cost() {
    let mut zero_quantity = reserved_basket();
    zero_quantity
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-ZERO-FILL",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("command should build");

    zero_quantity
        .apply_event(BoltV3BasketExecutionEvent::LegFill {
            client_order_id: "COID-YES".to_string(),
            venue_order_id: Some("VOID-YES".to_string()),
            quantity: Decimal::ZERO,
            cost: Decimal::ZERO,
            source: BoltV3BasketFillSource::Strategy,
        })
        .expect("malformed fill should classify without panicking");

    assert_eq!(zero_quantity.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(zero_quantity.unresolved_real_exposure());
    assert!(zero_quantity.reservation_held());

    let mut negative_cost = reserved_basket();
    negative_cost
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-NEGATIVE-FILL",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("command should build");

    negative_cost
        .apply_event(BoltV3BasketExecutionEvent::LegFill {
            client_order_id: "COID-YES".to_string(),
            venue_order_id: Some("VOID-YES".to_string()),
            quantity: dec("1.0"),
            cost: dec("-0.01"),
            source: BoltV3BasketFillSource::Strategy,
        })
        .expect("malformed fill should classify without panicking");

    assert_eq!(negative_cost.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(negative_cost.unresolved_real_exposure());
    assert!(negative_cost.reservation_held());
}

#[test]
fn same_venue_submit_uses_nt_order_list_contract_and_persists_client_ids_before_submit() {
    let contract = nt_order_management_contract();
    assert!(contract.order_list_type.ends_with("OrderList"));
    assert!(contract.submit_order_list_type.ends_with("SubmitOrderList"));
    assert!(contract.cancel_order_type.ends_with("CancelOrder"));
    assert!(
        contract
            .batch_cancel_orders_type
            .ends_with("BatchCancelOrders")
    );
    assert!(contract.cancel_all_orders_type.ends_with("CancelAllOrders"));
    assert!(contract.modify_order_type.ends_with("ModifyOrder"));

    let mut basket = reserved_basket();
    let command = basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-SAME-VENUE",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("reuse_nt same venue basket should build");

    assert_eq!(
        command,
        BoltV3BasketNtSubmitCommand {
            order_list_id: "OL-SAME-VENUE".to_string(),
            client_order_ids: vec!["COID-YES".to_string(), "COID-NO".to_string()],
            venue: "POLYMARKET".to_string(),
        }
    );
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Submitting);
    assert_eq!(basket.order_list_id(), Some("OL-SAME-VENUE"));
    assert_eq!(
        basket.client_order_ids(),
        vec!["COID-YES".to_string(), "COID-NO".to_string()]
    );
}

#[test]
fn same_venue_submit_rejects_duplicate_leg_intents_before_mutating_state() {
    let mut basket = reserved_basket();
    let mut intents = leg_intents();
    intents[1].leg_id = "YES".to_string();
    intents[1].client_order_id = "COID-YES-DUPLICATE".to_string();

    let error = basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-DUPLICATE",
            intents,
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect_err("duplicate leg intents must reject before submit mutation");

    assert!(matches!(
        error,
        BoltV3BasketExecutionError::LegShapeMismatch
    ));
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Reserved);
    assert_eq!(basket.order_list_id(), None);
    assert!(basket.client_order_ids().is_empty());
}

#[test]
fn same_venue_submit_rejects_mismatched_leg_intents_before_mutating_state() {
    let mut basket = reserved_basket();
    let mut intents = leg_intents();
    intents[0].instrument_id = "WRONG.POLYMARKET".to_string();

    let error = basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-MISMATCH",
            intents,
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect_err("intent instrument must match the reserved basket leg");

    assert!(matches!(
        error,
        BoltV3BasketExecutionError::LegShapeMismatch
    ));
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Reserved);
    assert_eq!(basket.order_list_id(), None);
    assert!(basket.client_order_ids().is_empty());
}

#[test]
fn same_venue_submit_rejects_duplicate_client_order_ids_before_mutating_state() {
    let mut basket = reserved_basket();
    let mut intents = leg_intents();
    intents[1].client_order_id = "COID-YES".to_string();

    let error = basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-DUPLICATE-COID",
            intents,
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect_err("duplicate client order ids must reject before submit mutation");

    assert!(matches!(
        error,
        BoltV3BasketExecutionError::LegShapeMismatch
    ));
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Reserved);
    assert_eq!(basket.order_list_id(), None);
    assert!(basket.client_order_ids().is_empty());
}

#[test]
fn same_venue_submit_rejects_empty_client_order_id_before_mutating_state() {
    let mut basket = reserved_basket();
    let mut intents = leg_intents();
    intents[0].client_order_id.clear();

    let error = basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-EMPTY-COID",
            intents,
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect_err("empty client order id must reject before submit mutation");

    assert!(matches!(
        error,
        BoltV3BasketExecutionError::LegShapeMismatch
    ));
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Reserved);
    assert_eq!(basket.order_list_id(), None);
    assert!(basket.client_order_ids().is_empty());
}

#[test]
fn same_venue_submit_rejects_intent_venue_that_disagrees_with_instrument_before_mutating_state() {
    let mut basket = reserved_basket();
    let mut intents = leg_intents();
    for intent in &mut intents {
        intent.venue = "HYPERLIQUID".to_string();
    }

    let error = basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-WRONG-VENUE",
            intents,
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect_err("intent venue must match the durable basket instrument venue");

    assert!(matches!(
        error,
        BoltV3BasketExecutionError::LegShapeMismatch
    ));
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Reserved);
    assert_eq!(basket.order_list_id(), None);
    assert!(basket.client_order_ids().is_empty());
}

#[test]
fn same_venue_submit_rejects_empty_venue_before_mutating_state() {
    let mut basket = reserved_basket();
    let mut intents = leg_intents();
    for intent in &mut intents {
        intent.venue.clear();
    }

    let error = basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-EMPTY-VENUE",
            intents,
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect_err("empty venue must reject before submit mutation");

    assert!(matches!(
        error,
        BoltV3BasketExecutionError::LegShapeMismatch
    ));
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Reserved);
    assert_eq!(basket.order_list_id(), None);
    assert!(basket.client_order_ids().is_empty());
}

#[test]
fn same_venue_submit_rechecks_freshness_before_mutating_state() {
    let mut basket = reserved_basket();
    let mut intents = leg_intents();
    intents[0].observed_unix_ms = SUBMIT_NOW_UNIX_MS - SUBMIT_MAX_OBSERVATION_AGE_MS - 1;

    let error = basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-STALE",
            intents,
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect_err("stale submit intent must reject before submit mutation");

    assert!(matches!(
        error,
        BoltV3BasketExecutionError::StaleSubmitIntent
    ));
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Reserved);
    assert_eq!(basket.order_list_id(), None);
    assert!(basket.client_order_ids().is_empty());
}

#[test]
fn audited_submit_mode_without_nt_order_list_is_rejected_without_fallback() {
    let mut basket = reserved_basket();

    let error = basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::RejectForNow,
            "OL-REJECTED",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect_err("submit mode without SubmitOrderList support must reject");

    assert_eq!(error.to_string(), "basket execution submit mode rejected");
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Reserved);
    assert_eq!(basket.order_list_id(), None);
}

#[test]
fn repair_math_requires_explicit_edge_inequality_and_plans_residual_quantities() {
    assert_eq!(
        REPAIR_EDGE_INEQUALITY,
        "min(M * (filled_qty + repair_qty)) - (filled_cost + repair_cost) preserves admitted absolute edge floor and normalized edge_bps floor"
    );

    let input = repair_input(
        vec![("YES", dec("1.0"), dec("0.44"))],
        vec![("NO", dec("1.0"), dec("0.45"))],
    );

    let outcome = input.plan_repair(&repair_policy());

    assert_eq!(
        outcome,
        BoltV3BasketRepairOutcome::Repair {
            residuals: vec![("NO".to_string(), dec("1.0"))],
            projected_absolute_edge: dec("0.11"),
            projected_edge_bps: dec_from_i64(1235),
        }
    );
}

#[test]
fn repair_costs_only_the_residual_quantity_needed_from_a_larger_quote() {
    let input = repair_input(
        vec![
            ("YES", dec("1.0"), dec("0.40")),
            ("NO", dec("0.50"), dec("0.20")),
        ],
        vec![("NO", dec("1.0"), dec("0.50"))],
    );

    let outcome = input.plan_repair(&repair_policy());

    assert_eq!(
        outcome,
        BoltV3BasketRepairOutcome::Repair {
            residuals: vec![("NO".to_string(), dec("0.50"))],
            projected_absolute_edge: dec("0.15"),
            projected_edge_bps: dec_from_i64(1764),
        }
    );
}

#[test]
fn repair_denial_transitions_to_unwind_or_stuck_without_live_submit() {
    let mut stale = repair_input(
        vec![("YES", dec("1.0"), dec("0.44"))],
        vec![("NO", dec("1.0"), dec("0.45"))],
    );
    stale.now_unix_ms = 1_000;
    stale.executable_repair_legs = vec![repair_leg("NO", dec("1.0"), dec("0.45"), 1)];

    assert_eq!(
        stale.plan_repair(&repair_policy()),
        BoltV3BasketRepairOutcome::Stuck {
            reason: "fresh executable repair books are required".to_string()
        }
    );

    let expensive = repair_input(
        vec![("YES", dec("1.0"), dec("0.95"))],
        vec![("NO", dec("1.0"), dec("0.95"))],
    );

    assert_eq!(
        expensive.plan_repair(&repair_policy()),
        BoltV3BasketRepairOutcome::Unwind {
            reason: "repair cannot preserve admitted edge floors".to_string()
        }
    );
}

#[test]
fn repair_and_unwind_reject_duplicate_leg_identifiers_instead_of_deduping() {
    let duplicate_targets = BoltV3BasketRepairInput {
        admitted_target_quantities: vec![
            ("YES".to_string(), dec("1.0")),
            ("YES".to_string(), dec("1.0")),
        ],
        filled_quantities: vec![("YES".to_string(), dec("0.5"))],
        filled_cost: dec("0.20"),
        payout_matrix: vec![vec![dec("1.0"), dec("1.0")]],
        executable_repair_legs: vec![repair_leg("YES", dec("1.0"), dec("0.43"), 1_000)],
        admitted_absolute_edge_floor: dec("0.01"),
        admitted_edge_bps_floor: dec("1"),
        remaining_retry_budget: 1,
        now_unix_ms: 1_100,
    };

    assert_eq!(
        duplicate_targets.plan_repair(&repair_policy()),
        BoltV3BasketRepairOutcome::Stuck {
            reason: "basket repair leg identifiers must be unique".to_string()
        }
    );

    let duplicate_unwind = BoltV3BasketUnwindInput {
        filled_quantities: vec![
            ("YES".to_string(), dec("1.0")),
            ("YES".to_string(), dec("1.0")),
        ],
        executable_unwind_legs: vec![repair_leg("YES", dec("1.0"), dec("0.43"), 1_000)],
        now_unix_ms: 1_100,
        settled: false,
        remaining_retry_budget: 1,
    };

    assert_eq!(
        duplicate_unwind.plan_unwind(&unwind_policy()),
        BoltV3BasketUnwindOutcome::Stuck {
            reason: "basket unwind leg identifiers must be unique".to_string()
        }
    );
}

#[test]
fn unwind_requires_fresh_executable_reductions_for_every_filled_leg() {
    let input = BoltV3BasketUnwindInput {
        filled_quantities: vec![
            ("YES".to_string(), dec("1.0")),
            ("NO".to_string(), dec("1.0")),
        ],
        executable_unwind_legs: vec![repair_leg("YES", dec("1.0"), dec("0.43"), 1_000)],
        now_unix_ms: 1_100,
        settled: false,
        remaining_retry_budget: 1,
    };

    assert_eq!(
        input.plan_unwind(&unwind_policy()),
        BoltV3BasketUnwindOutcome::Stuck {
            reason: "fresh executable unwind books are required".to_string()
        }
    );

    assert_eq!(
        BoltV3BasketUnwindInput {
            executable_unwind_legs: Vec::new(),
            ..input.clone()
        }
        .plan_unwind(&unwind_policy()),
        BoltV3BasketUnwindOutcome::Stuck {
            reason: "fresh executable unwind books are required".to_string()
        }
    );

    assert_eq!(
        BoltV3BasketUnwindInput {
            executable_unwind_legs: vec![
                repair_leg("YES", dec("0.50"), dec("0.21"), 1_000),
                repair_leg("NO", dec("1.0"), dec("0.43"), 1_000),
            ],
            ..input
        }
        .plan_unwind(&unwind_policy()),
        BoltV3BasketUnwindOutcome::Stuck {
            reason: "fresh executable unwind books are required".to_string()
        }
    );
}

#[test]
fn unwind_is_allowed_only_with_fresh_books_before_settlement() {
    let allowed = BoltV3BasketUnwindInput {
        filled_quantities: vec![("YES".to_string(), dec("1.0"))],
        executable_unwind_legs: vec![repair_leg("YES", dec("1.0"), dec("0.43"), 1_000)],
        now_unix_ms: 1_100,
        settled: false,
        remaining_retry_budget: 1,
    };

    assert_eq!(
        allowed.plan_unwind(&unwind_policy()),
        BoltV3BasketUnwindOutcome::Unwind {
            reductions: vec![("YES".to_string(), dec("1.0"))]
        }
    );

    assert_eq!(
        BoltV3BasketUnwindInput {
            settled: true,
            ..allowed.clone()
        }
        .plan_unwind(&unwind_policy()),
        BoltV3BasketUnwindOutcome::Stuck {
            reason: "unwind is forbidden after durable settlement".to_string()
        }
    );
}

#[test]
fn polymarket_live_settlement_rejects_until_reachable_signal_and_hip4_synthetic_fill_is_separate() {
    let mut polymarket = reserved_basket();
    let rejected = polymarket
        .apply_event(BoltV3BasketExecutionEvent::SettlementSignal(
            BoltV3BasketSettlementSignal::LiveSettlementRejectedUntilReachableNtSignal,
        ))
        .expect_err("Polymarket live execution remains gated by Task 0 settlement ledger");

    assert_eq!(
        rejected.to_string(),
        "live settlement signal is not reachable"
    );
    assert_eq!(polymarket.status(), BoltV3BasketExecutionStatus::Reserved);

    let mut hip4 = reserved_basket();
    hip4.build_same_venue_submit_command(
        BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
        "OL-HIP4",
        leg_intents(),
        SUBMIT_NOW_UNIX_MS,
        SUBMIT_MAX_OBSERVATION_AGE_MS,
    )
    .expect("same venue command should build");
    hip4.apply_event(BoltV3BasketExecutionEvent::LegFill {
        client_order_id: "COID-YES".to_string(),
        venue_order_id: Some("VOID-YES".to_string()),
        quantity: dec("1.0"),
        cost: dec("0.44"),
        source: BoltV3BasketFillSource::Hip4SyntheticSettlement,
    })
    .expect("synthetic settlement fill should be recorded separately");

    assert_eq!(hip4.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(hip4.settled());
    assert_eq!(
        hip4.fill_sources(),
        vec![BoltV3BasketFillSource::Hip4SyntheticSettlement]
    );
    let persisted = serde_json::to_value(&hip4).expect("basket should serialize");
    assert_eq!(
        persisted["legs"][0]["filled_quantity"], "0",
        "synthetic settlement must not contaminate strategy fill quantity"
    );
    assert_eq!(
        persisted["legs"][0]["filled_cost"], "0",
        "synthetic settlement must not contaminate strategy fill cost"
    );
}

#[test]
fn cancel_rejection_retry_exhaustion_and_stuck_hold_reservation() {
    let mut cancel_rejected = reserved_basket();
    cancel_rejected
        .apply_event(BoltV3BasketExecutionEvent::CancelRejected {
            reason: "venue rejected cancel".to_string(),
        })
        .expect("cancel rejection should be classified");
    assert_eq!(cancel_rejected.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(cancel_rejected.reservation_held());
    assert!(cancel_rejected.unresolved_real_exposure());

    let mut retries = reserved_basket();
    retries
        .apply_event(BoltV3BasketExecutionEvent::RetryBudgetExhausted {
            reason: "repair attempts exhausted".to_string(),
        })
        .expect("retry exhaustion should be classified");
    assert_eq!(retries.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(retries.reservation_held());
}

#[test]
fn reachable_nt_close_completes_no_exposure_and_rejects_late_strategy_fill() {
    let mut basket = reserved_basket();
    basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-SETTLED",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("same venue command should build");

    basket
        .apply_event(BoltV3BasketExecutionEvent::SettlementSignal(
            BoltV3BasketSettlementSignal::ReachableNtClose,
        ))
        .expect("reachable NT close should settle a no-exposure basket");

    assert!(basket.settled());
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Complete);
    assert!(!basket.unresolved_real_exposure());

    let late_fill = basket
        .apply_event(BoltV3BasketExecutionEvent::LegFill {
            client_order_id: "COID-YES".to_string(),
            venue_order_id: Some("VOID-YES".to_string()),
            quantity: dec("1.0"),
            cost: dec("0.44"),
            source: BoltV3BasketFillSource::Strategy,
        })
        .expect_err("ordinary fills after settlement must not mutate state");

    assert_eq!(
        late_fill,
        BoltV3BasketExecutionError::InvalidStateTransition
    );
    let persisted = serde_json::to_value(&basket).expect("basket should serialize");
    assert_eq!(persisted["legs"][0]["filled_quantity"], "0");
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Complete);
}

#[test]
fn late_cancel_and_retry_events_do_not_overwrite_terminal_basket_states() {
    let mut basket = reserved_basket();
    basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-TERMINAL",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("same venue command should build");

    for (client_order_id, venue_order_id, cost) in [
        ("COID-YES", "VOID-YES", dec("0.44")),
        ("COID-NO", "VOID-NO", dec("0.46")),
    ] {
        basket
            .apply_event(BoltV3BasketExecutionEvent::LegFill {
                client_order_id: client_order_id.to_string(),
                venue_order_id: Some(venue_order_id.to_string()),
                quantity: dec("1.0"),
                cost,
                source: BoltV3BasketFillSource::Strategy,
            })
            .expect("fill should apply");
    }
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Complete);

    basket
        .apply_event(BoltV3BasketExecutionEvent::CancelRejected {
            reason: "late cancel reject".to_string(),
        })
        .expect("late cancel rejection should no-op for complete basket");
    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Complete);
    assert!(!basket.unresolved_real_exposure());

    basket
        .apply_event(BoltV3BasketExecutionEvent::TerminalClose)
        .expect("terminal close should release a complete basket");
    basket
        .apply_event(BoltV3BasketExecutionEvent::RetryBudgetExhausted {
            reason: "late retry exhaustion".to_string(),
        })
        .expect("late retry exhaustion should no-op for closed basket");

    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Closed);
    assert!(!basket.reservation_held());
    assert!(!basket.unresolved_real_exposure());
}

#[test]
fn strategy_fill_after_stuck_does_not_clear_unresolved_exposure_or_release_reservation() {
    let mut basket = reserved_basket();
    basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-STUCK",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("same venue command should build");
    basket
        .apply_event(BoltV3BasketExecutionEvent::CancelRejected {
            reason: "venue rejected cancel".to_string(),
        })
        .expect("cancel rejection should latch stuck exposure");

    for (client_order_id, venue_order_id, cost) in [
        ("COID-YES", "VOID-YES", dec("0.44")),
        ("COID-NO", "VOID-NO", dec("0.46")),
    ] {
        basket
            .apply_event(BoltV3BasketExecutionEvent::LegFill {
                client_order_id: client_order_id.to_string(),
                venue_order_id: Some(venue_order_id.to_string()),
                quantity: dec("1.0"),
                cost,
                source: BoltV3BasketFillSource::Strategy,
            })
            .expect("late strategy fill should record without un-sticking");
    }

    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(basket.reservation_held());
    assert!(basket.unresolved_real_exposure());
    assert_eq!(
        basket
            .apply_event(BoltV3BasketExecutionEvent::TerminalClose)
            .expect_err("terminal close must not release unresolved stuck exposure"),
        BoltV3BasketExecutionError::UnresolvedExposure
    );
}

#[test]
fn overfilled_basket_fails_closed_instead_of_marking_complete() {
    let mut basket = reserved_basket();
    basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-OVERFILL",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("same venue command should build");
    basket
        .apply_event(BoltV3BasketExecutionEvent::LegFill {
            client_order_id: "COID-YES".to_string(),
            venue_order_id: Some("VOID-YES".to_string()),
            quantity: dec("1.1"),
            cost: dec("0.48"),
            source: BoltV3BasketFillSource::Strategy,
        })
        .expect("overfill should record");

    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(basket.reservation_held());
    assert!(basket.unresolved_real_exposure());
}

#[test]
fn durable_store_round_trips_basket_specific_recovery_state_and_enforces_size_limit() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("basket-state.json");
    let store = BoltV3BasketStore::new(path.clone(), 65_536);
    let mut basket = reserved_basket();
    basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-RECOVER",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("command should build");
    basket
        .apply_event(BoltV3BasketExecutionEvent::VenueOrderId {
            client_order_id: "COID-YES".to_string(),
            venue_order_id: "VOID-YES".to_string(),
        })
        .expect("venue order id should persist");

    store
        .write_state(&basket)
        .expect("basket state should persist");

    assert_eq!(
        store.load_recovery_state().expect("state should load"),
        BoltV3BasketRecoveryState::Recovered(basket)
    );
    assert!(
        fs::read(&path)
            .expect("state file should read")
            .ends_with(b"\n"),
        "state file should have stable trailing newline"
    );

    let tiny_store = BoltV3BasketStore::new(path, 8);
    assert_eq!(
        tiny_store
            .load_recovery_state()
            .expect("oversized state should classify"),
        BoltV3BasketRecoveryState::FailClosed {
            reason: BoltV3BasketRecoveryReason::StateFileTooLarge,
            state: None,
        }
    );
}

#[test]
fn basket_state_v1_old_bytes_remain_readable() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("basket-state.json");
    fs::write(
        &path,
        include_bytes!("fixtures/bolt_v3/compatibility/basket_state_v1.json"),
    )
    .expect("old-byte basket fixture should write");
    let store = BoltV3BasketStore::new(path, 65_536);

    let state = match store
        .load_recovery_state()
        .expect("old-byte basket state should parse")
    {
        BoltV3BasketRecoveryState::Recovered(state) => state,
        other => panic!("expected Recovered, got {other:?}"),
    };
    let expected = BoltV3BasketExecutionState::candidate(
        "legacy-basket",
        "legacy-strategy",
        "legacy-client",
        vec![("yes", "YES.POLYMARKET", dec("1"))],
        vec![vec![dec("1")]],
        dec("0.01"),
        dec("100"),
        BoltV3BasketExecutionConfig {
            repair: BoltV3BasketRepairPolicy {
                max_retries: 1,
                max_book_age_ms: 1_000,
                max_slippage_bps: 25,
                max_depth_levels: 5,
                allow_unwind_when_repair_denied: true,
            },
            unwind: BoltV3BasketUnwindPolicy {
                max_retries: 1,
                max_book_age_ms: 1_000,
                max_slippage_bps: 25,
                max_depth_levels: 5,
            },
        },
    )
    .expect("expected legacy basket semantics should be valid");
    assert_eq!(state, expected);
}

#[test]
fn restart_reconciliation_joins_client_id_then_venue_id_and_stucks_orphans() {
    let mut basket = reserved_basket();
    basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-RECON",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("command should build");
    basket
        .apply_event(BoltV3BasketExecutionEvent::VenueOrderId {
            client_order_id: "COID-NO".to_string(),
            venue_order_id: "VOID-NO".to_string(),
        })
        .expect("venue order id should persist");

    basket
        .reconcile_restart(&[
            BoltV3BasketRestartReport {
                instrument_id: "YES.POLYMARKET".to_string(),
                client_order_id: Some("COID-YES".to_string()),
                venue_order_id: Some("DIFFERENT-YES".to_string()),
                filled_quantity: dec("1.0"),
                filled_cost: dec("0.44"),
                report_class: BoltV3ExternalReportClass::StrategyOwned,
            },
            BoltV3BasketRestartReport {
                instrument_id: "NO.POLYMARKET".to_string(),
                client_order_id: None,
                venue_order_id: Some("VOID-NO".to_string()),
                filled_quantity: dec("1.0"),
                filled_cost: dec("0.46"),
                report_class: BoltV3ExternalReportClass::StrategyOwned,
            },
        ])
        .expect("deterministic reports should adopt");

    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Complete);
    assert!(!basket.unresolved_real_exposure());

    let mut orphan = reserved_basket();
    orphan
        .reconcile_restart(&[BoltV3BasketRestartReport {
            instrument_id: "MAYBE.POLYMARKET".to_string(),
            client_order_id: None,
            venue_order_id: Some("VOID-ORPHAN".to_string()),
            filled_quantity: dec("1.0"),
            filled_cost: dec("0.52"),
            report_class: BoltV3ExternalReportClass::Unclaimed,
        }])
        .expect("orphan should classify without panicking");

    assert_eq!(orphan.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(orphan.unresolved_real_exposure());
}

#[test]
fn restart_reconciliation_stucks_submitting_baskets_without_complete_reports() {
    let mut basket = reserved_basket();
    basket
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-MISSING-REPORTS",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("command should build");

    basket
        .reconcile_restart(&[])
        .expect("missing restart reports should classify without panicking");

    assert_eq!(basket.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(basket.unresolved_real_exposure());
    assert!(basket.reservation_held());
}

#[test]
fn restart_reconciliation_rejects_non_strategy_or_cross_instrument_reports() {
    let mut external = reserved_basket();
    external
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-EXTERNAL",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("command should build");
    external
        .apply_event(BoltV3BasketExecutionEvent::VenueOrderId {
            client_order_id: "COID-NO".to_string(),
            venue_order_id: "VOID-NO".to_string(),
        })
        .expect("venue order id should persist");

    external
        .reconcile_restart(&[BoltV3BasketRestartReport {
            instrument_id: "NO.POLYMARKET".to_string(),
            client_order_id: None,
            venue_order_id: Some("VOID-NO".to_string()),
            filled_quantity: dec("1.0"),
            filled_cost: dec("0.46"),
            report_class: BoltV3ExternalReportClass::EngineClassifiedExternal,
        }])
        .expect("external report should classify without panicking");

    assert_eq!(external.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(external.unresolved_real_exposure());

    let mut wrong_instrument = reserved_basket();
    wrong_instrument
        .reconcile_restart(&[BoltV3BasketRestartReport {
            instrument_id: "NO.POLYMARKET".to_string(),
            client_order_id: Some("COID-YES".to_string()),
            venue_order_id: None,
            filled_quantity: dec("1.0"),
            filled_cost: dec("0.44"),
            report_class: BoltV3ExternalReportClass::StrategyOwned,
        }])
        .expect("cross-instrument report should classify without panicking");

    assert_eq!(
        wrong_instrument.status(),
        BoltV3BasketExecutionStatus::Stuck
    );
    assert!(wrong_instrument.unresolved_real_exposure());
}

#[test]
fn restart_reconciliation_rejects_duplicate_or_negative_reports() {
    let mut duplicate = reserved_basket();
    duplicate
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-DUPLICATE-REPORT",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("command should build");
    duplicate
        .reconcile_restart(&[
            BoltV3BasketRestartReport {
                instrument_id: "YES.POLYMARKET".to_string(),
                client_order_id: Some("COID-YES".to_string()),
                venue_order_id: None,
                filled_quantity: dec("0.5"),
                filled_cost: dec("0.22"),
                report_class: BoltV3ExternalReportClass::StrategyOwned,
            },
            BoltV3BasketRestartReport {
                instrument_id: "YES.POLYMARKET".to_string(),
                client_order_id: Some("COID-YES".to_string()),
                venue_order_id: None,
                filled_quantity: dec("1.0"),
                filled_cost: dec("0.44"),
                report_class: BoltV3ExternalReportClass::StrategyOwned,
            },
        ])
        .expect("duplicate report should classify without panicking");

    assert_eq!(duplicate.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(duplicate.unresolved_real_exposure());

    let mut negative = reserved_basket();
    negative
        .build_same_venue_submit_command(
            BoltV3BasketExecutionSubmitDisposition::ReuseNtSubmitOrderList,
            "OL-NEGATIVE-REPORT",
            leg_intents(),
            SUBMIT_NOW_UNIX_MS,
            SUBMIT_MAX_OBSERVATION_AGE_MS,
        )
        .expect("command should build");
    negative
        .reconcile_restart(&[BoltV3BasketRestartReport {
            instrument_id: "YES.POLYMARKET".to_string(),
            client_order_id: Some("COID-YES".to_string()),
            venue_order_id: None,
            filled_quantity: dec("1.0"),
            filled_cost: dec("-0.01"),
            report_class: BoltV3ExternalReportClass::StrategyOwned,
        }])
        .expect("negative report should classify without panicking");

    assert_eq!(negative.status(), BoltV3BasketExecutionStatus::Stuck);
    assert!(negative.unresolved_real_exposure());
}

#[test]
fn stuck_basket_trips_dedicated_kill_switch_and_blocks_new_admission() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let kill_store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let writer = Arc::new(NoopDecisionEvidenceWriter);
    let submit_admission = BoltV3SubmitAdmissionState::new(writer);
    let mut basket = reserved_basket();
    basket
        .apply_event(BoltV3BasketExecutionEvent::CancelRejected {
            reason: "unresolved real exposure".to_string(),
        })
        .expect("stuck should apply");

    let kill_state =
        trip_stuck_basket_kill_switch(&basket, &kill_store, &submit_admission, 1_717_200_000)
            .expect("stuck basket should trip kill switch");

    assert_eq!(kill_state.kind(), KillSwitchStateKind::Halted);
    let KillSwitchState::Halted { trigger, .. } = kill_state else {
        panic!("expected halted kill switch");
    };
    assert_eq!(
        trigger.kind,
        KillSwitchHaltTriggerKind::BasketExecutionStuck
    );

    let blocked = submit_admission
        .admit(&sample_submit_request())
        .expect_err("latched basket kill switch must block new entry admission");
    assert_eq!(
        blocked.to_string(),
        "bolt-v3 submit admission is blocked by kill-switch state Halted"
    );
}

#[test]
fn stuck_basket_kill_switch_preserves_existing_non_armed_state() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let kill_store = KillSwitchStore::new(
        temp.path().join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let existing = KillSwitchState::FailedManualIntervention {
        halt_id: "existing-halt".to_string(),
        reason: "operator intervention required".to_string(),
    };
    kill_store
        .write_state(&existing)
        .expect("existing state should persist");
    let writer = Arc::new(NoopDecisionEvidenceWriter);
    let submit_admission = BoltV3SubmitAdmissionState::new(writer);
    let mut basket = reserved_basket();
    basket
        .apply_event(BoltV3BasketExecutionEvent::CancelRejected {
            reason: "unresolved real exposure".to_string(),
        })
        .expect("stuck should apply");

    let kill_state =
        trip_stuck_basket_kill_switch(&basket, &kill_store, &submit_admission, 1_717_200_000)
            .expect("existing fail-closed state should be preserved");

    assert_eq!(kill_state, existing);
    let blocked = submit_admission
        .admit(&sample_submit_request())
        .expect_err("preserved failed-manual-intervention state must block entry admission");
    assert_eq!(
        blocked.to_string(),
        "bolt-v3 submit admission is blocked by kill-switch state FailedManualIntervention"
    );
}

#[cfg(unix)]
#[test]
fn stuck_basket_kill_switch_latches_failed_manual_intervention_on_store_failure() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir should create");
    let blocked_dir = temp.path().join("blocked");
    fs::create_dir(&blocked_dir).expect("blocked directory should create");
    fs::set_permissions(&blocked_dir, fs::Permissions::from_mode(0o500))
        .expect("blocked directory should become read-only");
    let kill_store = KillSwitchStore::new(
        blocked_dir.join("kill-switch.json"),
        TEST_MAX_STATE_FILE_BYTES,
    );
    let writer = Arc::new(NoopDecisionEvidenceWriter);
    let submit_admission = BoltV3SubmitAdmissionState::new(writer);
    let mut basket = reserved_basket();
    basket
        .apply_event(BoltV3BasketExecutionEvent::CancelRejected {
            reason: "unresolved real exposure".to_string(),
        })
        .expect("stuck should apply");

    let kill_state =
        trip_stuck_basket_kill_switch(&basket, &kill_store, &submit_admission, 1_717_200_000)
            .expect("store failure should still produce an in-memory fail-closed state");

    assert_eq!(
        kill_state.kind(),
        KillSwitchStateKind::FailedManualIntervention
    );
    let blocked = submit_admission
        .admit(&sample_submit_request())
        .expect_err("failed-manual-intervention state must block entry admission");
    assert_eq!(
        blocked.to_string(),
        "bolt-v3 submit admission is blocked by kill-switch state FailedManualIntervention"
    );
    fs::set_permissions(&blocked_dir, fs::Permissions::from_mode(0o700))
        .expect("blocked directory permissions should restore for cleanup");
}

#[test]
fn executor_event_contract_keeps_strategy_shell_out_of_submit_and_venue_mutation() {
    let contract = executor_event_integration_contract();

    assert_eq!(
        contract.forwarded_nt_events,
        vec!["order", "fill", "cancel", "instrument_status", "settlement"]
    );
    assert!(!contract.strategy_shell_may_call_submit_admission);
    assert!(!contract.strategy_shell_may_mutate_venue);
    assert!(contract.shared_executor_owns_state_transitions);
}

fn reserved_basket() -> BoltV3BasketExecutionState {
    let config = BoltV3BasketExecutionConfig {
        repair: repair_policy(),
        unwind: unwind_policy(),
    };
    let mut basket = BoltV3BasketExecutionState::candidate(
        "basket-1",
        "strategy-complete-set",
        "exec-poly",
        vec![
            ("YES", "YES.POLYMARKET", dec("1.0")),
            ("NO", "NO.POLYMARKET", dec("1.0")),
        ],
        vec![vec![dec("1.0"), dec("0.0")], vec![dec("0.0"), dec("1.0")]],
        dec("0.10"),
        dec_from_i64(1_000),
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

fn repair_input(
    fills: Vec<(&str, Decimal, Decimal)>,
    repair_books: Vec<(&str, Decimal, Decimal)>,
) -> BoltV3BasketRepairInput {
    BoltV3BasketRepairInput {
        admitted_target_quantities: vec![
            ("YES".to_string(), dec("1.0")),
            ("NO".to_string(), dec("1.0")),
        ],
        filled_quantities: fills
            .iter()
            .map(|(leg, quantity, _cost)| ((*leg).to_string(), *quantity))
            .collect(),
        filled_cost: fills
            .iter()
            .fold(Decimal::ZERO, |total, (_leg, _quantity, cost)| {
                total + *cost
            }),
        payout_matrix: vec![vec![dec("1.0"), dec("0.0")], vec![dec("0.0"), dec("1.0")]],
        executable_repair_legs: repair_books
            .into_iter()
            .map(|(leg, quantity, cost)| repair_leg(leg, quantity, cost, 1_000))
            .collect(),
        admitted_absolute_edge_floor: dec("0.10"),
        admitted_edge_bps_floor: dec_from_i64(1_000),
        remaining_retry_budget: 1,
        now_unix_ms: 1_100,
    }
}

fn repair_policy() -> BoltV3BasketRepairPolicy {
    BoltV3BasketRepairPolicy {
        max_retries: 2,
        max_book_age_ms: 250,
        max_slippage_bps: 50,
        max_depth_levels: 4,
        allow_unwind_when_repair_denied: true,
    }
}

fn unwind_policy() -> BoltV3BasketUnwindPolicy {
    BoltV3BasketUnwindPolicy {
        max_retries: 2,
        max_book_age_ms: 250,
        max_slippage_bps: 50,
        max_depth_levels: 4,
    }
}

fn sample_submit_request() -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-complete-set".to_string(),
        execution_client_id: "exec-poly".to_string(),
        client_order_id: "COID-NEW".to_string(),
        instrument_id: "YES.POLYMARKET".to_string(),
        notional: dec("0.10"),
        order_side: OrderSide::Buy,
        order_quantity: dec("1.0"),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
        risk_reducing_exit_proof: None,
        kill_switch_forced_reduction: None,
        admission_evidence: None,
    }
}

fn dec(value: &str) -> Decimal {
    value.parse().expect("decimal fixture should parse")
}

fn dec_from_i64(value: i64) -> Decimal {
    Decimal::from_i64(value).expect("integer decimal should convert")
}

fn repair_leg(
    leg_id: &str,
    quantity: Decimal,
    cost: Decimal,
    observed_unix_ms: u64,
) -> BoltV3ExecutableRepairLeg {
    BoltV3ExecutableRepairLeg {
        leg_id: leg_id.to_string(),
        quantity,
        cost,
        observed_unix_ms,
        slippage_bps: 10,
        depth_levels: 1,
    }
}

#[derive(Debug)]
struct NoopDecisionEvidenceWriter;

impl BoltV3DecisionEvidenceWriter for NoopDecisionEvidenceWriter {
    fn record_strategy_input_snapshot(
        &self,
        _snapshot: &bolt_v2::bolt_v3_decision_evidence::BoltV3StrategyInputEvidenceSnapshot,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_order_intent(
        &self,
        _intent: &bolt_v2::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_admission_decision(
        &self,
        _decision: &bolt_v2::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_basket_admission_decision(
        &self,
        _decision: &bolt_v2::bolt_v3_decision_evidence::BoltV3BasketAdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_capital_admission_rebuild_audit(
        &self,
        _audit: &bolt_v2::bolt_v3_decision_evidence::BoltV3CapitalAdmissionRebuildAuditEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &bolt_v2::bolt_v3_decision_evidence::BoltV3SubmitReservationMetadataEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &bolt_v2::bolt_v3_decision_evidence::BoltV3SubmitReservationFillEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_entry_skip(
        &self,
        _skip: &bolt_v2::bolt_v3_decision_evidence::BoltV3EntrySkipEvidence,
    ) -> anyhow::Result<()> {
        anyhow::bail!("basket execution noop writer received entry-skip evidence")
    }

    fn record_exit_decision(
        &self,
        _decision: &bolt_v2::bolt_v3_decision_evidence::BoltV3ExitDecisionEvidence,
    ) -> anyhow::Result<()> {
        anyhow::bail!("basket execution noop writer received exit-decision evidence")
    }

    fn record_exit_evaluation(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3ExitEvaluationEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_loss_governor_halt(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3LossGovernorHaltEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_requote_throttle(
        &self,
        _throttle: &bolt_v2::bolt_v3_decision_evidence::BoltV3RequoteThrottleEvidence,
    ) -> anyhow::Result<()> {
        anyhow::bail!("basket execution noop writer received requote-throttle evidence")
    }

    fn record_order_reject(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3OrderRejectEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_settlement(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3SettlementEvidence,
    ) -> anyhow::Result<()> {
        anyhow::bail!("basket execution noop writer received settlement evidence")
    }

    fn record_settlement_booking_error(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3SettlementBookingErrorEvidence,
    ) -> anyhow::Result<()> {
        anyhow::bail!("basket execution noop writer received settlement booking-error evidence")
    }

    fn drain_shutdown(&self) -> anyhow::Result<()> {
        // Deliberate no-op: this basket execution fixture never owns durable evidence.
        Ok(())
    }
}
