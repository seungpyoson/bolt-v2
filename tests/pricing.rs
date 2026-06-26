//! pricing integration-test harness (#990 slice 6) - consolidates the 10
//! pricing / realized-volatility / reference-price integration tests into
//! one
//! [[test]] binary; tests are unchanged, re-homed as pricing::<member>::.

mod bolt_v3_fair_value_pricing;
mod bolt_v3_realized_volatility;
mod bolt_v3_realized_volatility_runtime;
mod bolt_v3_realized_volatility_source_fence;
mod bolt_v3_reference_price;
mod bolt_v3_reference_price_config;
mod bolt_v3_reference_price_runtime;
mod bolt_v3_reference_provider_registration;
mod bolt_v3_static_binary_event_reference_price;
mod bolt_v3_taker_pricing;

// Shared helper module (tests/support/mod.rs); not a [[test]] member.
mod support;
