//! iv subsystem integration-test harness (#990 slice 1) - consolidates the 12
//! bolt_v3_iv_* integration tests into one [[test]] binary; tests are unchanged,
//! re-homed as iv::<member>::.

mod bolt_v3_iv_capability;
mod bolt_v3_iv_config;
mod bolt_v3_iv_derive;
mod bolt_v3_iv_foundation;
mod bolt_v3_iv_ingest;
mod bolt_v3_iv_live_integration;
mod bolt_v3_iv_policy;
mod bolt_v3_iv_query;
mod bolt_v3_iv_source_fence;
mod bolt_v3_iv_store;
mod bolt_v3_iv_subscription;
mod bolt_v3_iv_support;
