//! kill_switch_loss integration-test harness (#990 slice 5) - consolidates the 9
//! kill-switch and loss-protection integration tests into one [[test]]
//! binary;
//! tests are unchanged, re-homed as kill_switch_loss::<member>::.

mod bolt_v3_kill_switch;
mod bolt_v3_kill_switch_action_router;
mod bolt_v3_kill_switch_cancel;
mod bolt_v3_kill_switch_config;
mod bolt_v3_kill_switch_flatten;
mod bolt_v3_kill_switch_runtime;
mod bolt_v3_kill_switch_store;
mod bolt_v3_loss_governor_manual_recovery_ops;
mod bolt_v3_loss_protection;
mod bolt_v3_loss_runtime_feed;

// Shared helper module (tests/support/mod.rs); not a [[test]] member.
mod support;
