//! maker_taker integration-test harness (#990 slice 4) - consolidates 7
//! maker/
//! taker integration tests into one [[test]] binary; tests are unchanged,
//! re-homed as maker_taker::<member>::.

mod bolt_v3_binary_oracle_edge_taker_a10_structure;
mod bolt_v3_binary_oracle_entry_log_contract;
mod bolt_v3_binary_oracle_maker_config;
mod bolt_v3_binary_oracle_maker_runtime;
mod bolt_v3_maker_event_fence;
mod bolt_v3_maker_market_selection;
mod bolt_v3_maker_runtime_quote;

// Shared helper module (tests/support/mod.rs); not a [[test]] member.
mod support;
