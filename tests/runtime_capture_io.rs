//! runtime_capture_io integration-test harness (#990 slice 8) - consolidates the 9
//! atomic-io / capture / lake / nt-runtime integration tests into one [[test]]
//! binary; tests are unchanged, re-homed as runtime_capture_io::<member>::.

mod bolt_v3_atomic_io;
mod bolt_v3_instrument_filters;
mod lake_batch;
mod log_sweep;
mod nt_custom_data_catalog_integration;
mod nt_polymarket_filter_integration;
mod nt_runtime_capture;
mod raw_capture_io;
mod shadow_pnl_report;

// Shared helper module (tests/support/mod.rs); not a [[test]] member.
mod support;
