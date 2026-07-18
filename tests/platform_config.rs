//! platform_config integration-test harness (#990 slice 9) - consolidates the 13
//! platform / config / deploy integration tests into one [[test]] binary; tests
//! are unchanged, re-homed as platform_config::<member>::.

mod bolt_v3_adapter_mapping;
mod bolt_v3_backtest_config_override;
mod bolt_v3_dead_gate_removal;
mod bolt_v3_polyresearch_auth;
mod build_script_git_head_rerun_paths;
mod cli;
mod config_parsing;
mod deploy_install;
mod deploy_systemd;
mod hyperliquid_fail_closed;
mod hyperliquid_live_submit_artifact;
mod hyperliquid_product_matrix;
mod nautilus_source_capabilities;
mod venue_contract;
// Shared helper module (tests/support/mod.rs); not a [[test]] member.
mod support;
