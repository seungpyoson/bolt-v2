//! wiring_registration integration-test harness (#990 slice 2) - consolidates
//! 13 wiring-registration integration tests into one [[test]] binary; tests are
//! unchanged, re-homed as wiring_registration::<member>::.

mod bolt_v3_chainlink_config;
mod bolt_v3_chainlink_registration;
mod bolt_v3_client_registration;
mod bolt_v3_complete_set_strategy_shell;
mod bolt_v3_controlled_connect;
mod bolt_v3_credential_log_suppression;
mod bolt_v3_decision_evidence;
mod bolt_v3_evidence_novelty;
mod bolt_v3_operator_health;
mod bolt_v3_prod_profile;
mod bolt_v3_production_entrypoint;
mod bolt_v3_provider_binding;
mod bolt_v3_readiness;
mod bolt_v3_strategy_registration;

// Shared helper module (tests/support/mod.rs); not a [[test]] member.
mod support;
