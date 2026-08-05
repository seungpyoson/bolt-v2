//! admission_orders integration-test harness (#990 slice 7) - consolidates the 7
//! basket / capital-admission / order-intent / submit integration tests into
//! one
//! [[test]] binary; tests are unchanged, re-homed as admission_orders::<member>::.

mod bolt_v3_basket_admission;
mod bolt_v3_capital_admission_runtime_feed;
mod bolt_v3_order_intent;
mod bolt_v3_order_reject_observer_feed;
mod bolt_v3_submit_admission;

// Shared helper module (tests/support/mod.rs); not a [[test]] member.
mod support;
