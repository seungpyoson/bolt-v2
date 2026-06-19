//! Source fence for the production binary entrypoint.
//!
//! Phase 2 requires `src/main.rs` to enter NT through the bolt-v3
//! `run_bolt_v3_live_node` wrapper, not through a direct production
//! `LiveNode::run` call. This is a best-effort textual guard; it is not
//! a compile-time proof.

#[test]
fn main_uses_bolt_v3_runner_wrapper_only() {
    let source = include_str!("../src/main.rs");

    assert!(
        source.contains("run_bolt_v3_live_node"),
        "production entrypoint must call the bolt-v3 gated runner wrapper"
    );

    for forbidden in ["node.run()", "LiveNode::run("] {
        assert!(
            !source.contains(forbidden),
            "production entrypoint must not call NT runner directly via `{forbidden}`"
        );
    }
}

#[test]
fn main_runs_bolt_v3_runner_inside_local_set() {
    let source = include_str!("../src/main.rs");

    let build_live_node = source
        .find("let mut node = build_bolt_v3_live_node(&loaded)?;")
        .expect("production entrypoint must build the LiveNode");
    let build_runtime = source
        .find("let runtime = tokio::runtime::Builder::new_multi_thread()")
        .expect("production entrypoint must build the Tokio runtime");
    assert!(
        build_live_node < build_runtime,
        "production entrypoint must resolve SSM and build the LiveNode before entering Tokio runtime"
    );
    assert!(
        source.contains("tokio::task::LocalSet::new()"),
        "production entrypoint must create a LocalSet for NT's thread-local runner context"
    );
    assert!(
        source.contains("runtime.block_on(local.run_until(app))"),
        "production entrypoint must enter the bolt-v3 runner future through LocalSet::run_until"
    );
}

#[test]
fn data_client_probe_builds_node_before_entering_tokio_runtime() {
    let source = include_str!("../src/main.rs");
    let probe_fn = source
        .split("fn run_data_client_probe")
        .nth(1)
        .expect("production binary must expose ops data-client-probe runner");

    let build_probe = probe_fn
        .find("build_bolt_v3_strategy_free_data_client_probe_live_node(&loaded, client_key)?")
        .expect("data-client probe must build the scoped LiveNode");
    let build_runtime = probe_fn
        .find("tokio::runtime::Builder::new_multi_thread()")
        .expect("data-client probe must build the Tokio runtime");
    assert!(
        build_probe < build_runtime,
        "data-client probe must resolve SSM and build the LiveNode before entering Tokio runtime"
    );
    assert!(
        probe_fn.contains("run_bolt_v3_data_client_probe(node_runtime, &probe_loaded, client_key)"),
        "data-client probe async runner must receive an already-built node runtime"
    );
    assert!(
        !probe_fn.contains("run_bolt_v3_data_client_probe(&loaded"),
        "data-client probe must not build SSM-backed runtime state from inside LocalSet::run_until"
    );
}

#[test]
fn data_client_census_builds_node_before_entering_tokio_runtime() {
    let source = include_str!("../src/main.rs");
    let census_fn = source
        .split("fn run_data_client_census")
        .nth(1)
        .expect("production binary must expose ops data-client-census runner");

    let build_census = census_fn
        .find("build_bolt_v3_strategy_free_data_client_probe_live_node(&loaded, client_key)?")
        .expect("data-client census must build the scoped LiveNode");
    let build_runtime = census_fn
        .find("tokio::runtime::Builder::new_multi_thread()")
        .expect("data-client census must build the Tokio runtime");
    assert!(
        build_census < build_runtime,
        "data-client census must resolve SSM and build the LiveNode before entering Tokio runtime"
    );
    assert!(
        census_fn
            .contains("run_bolt_v3_data_client_census(node_runtime, &census_loaded, client_key)"),
        "data-client census async runner must receive an already-built node runtime"
    );
    assert!(
        !census_fn.contains("run_bolt_v3_data_client_census(&loaded"),
        "data-client census must not build SSM-backed runtime state from inside LocalSet::run_until"
    );
}

#[test]
fn ops_exposes_no_overwrite_kill_switch_store_bootstrap() {
    let source = include_str!("../src/main.rs");
    let init_fn = source
        .split("fn run_init_kill_switch_store")
        .nth(1)
        .expect("production ops CLI must expose kill-switch store bootstrap");

    assert!(
        source.contains("InitKillSwitchStore"),
        "ops CLI must expose an explicit kill-switch store bootstrap command"
    );
    assert!(
        init_fn.contains("KillSwitchStore::from_root_config_path"),
        "bootstrap command must derive the store path from risk.kill_switch.state_path"
    );
    assert!(
        init_fn.contains("bootstrap_initial_armed_loss_snapshot()"),
        "bootstrap command must use the no-overwrite Armed+zero-loss store writer"
    );
    assert!(
        !init_fn.contains("write_state_with_loss_snapshot"),
        "bootstrap command must not bypass the no-overwrite helper"
    );
}

#[test]
fn bolt_v3_production_path_cannot_load_legacy_config_defaults() {
    let production_sources = [
        ("src/main.rs", include_str!("../src/main.rs")),
        (
            "src/bolt_v3_live_node.rs",
            include_str!("../src/bolt_v3_live_node.rs"),
        ),
    ];

    assert!(
        production_sources[0].1.contains("load_bolt_v3_config"),
        "production binary must load the bolt-v3 root TOML contract"
    );
    assert!(
        production_sources[1].1.contains("LoadedBoltV3Config"),
        "bolt-v3 LiveNode builder must accept the loaded bolt-v3 config contract"
    );

    for (path, source) in production_sources {
        for forbidden in [
            "Config::load",
            "LiveLocalConfig::load",
            "materialize_live_config",
            "crate::config",
            "crate::live_config",
            "clients::polymarket",
            "clients::chainlink",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} must not reach legacy config/default surfaces via `{forbidden}`"
            );
        }
    }
}

#[test]
fn codebase_does_not_expose_dead_platform_runtime_actor_or_catalog_modules() {
    for forbidden_path in [
        "src/platform/runtime.rs",
        "src/platform/mod.rs",
        "src/platform/audit.rs",
        "src/platform/reference.rs",
        "src/platform/ruleset.rs",
        "src/platform/reference_actor.rs",
        "src/platform/polymarket_catalog.rs",
        "src/clients/bybit.rs",
        "src/clients/deribit.rs",
        "src/clients/hyperliquid.rs",
        "src/clients/kraken.rs",
        "src/clients/okx.rs",
        "src/clients/binance.rs",
        "src/bin/raw_capture.rs",
        "src/bin/render_live_config.rs",
        "src/live_config.rs",
        "src/live_node_setup.rs",
        "src/startup_validation.rs",
        "src/raw_capture_transport.rs",
        "src/validate/tests.rs",
        "src/clients/mod.rs",
        "src/clients/chainlink.rs",
        "src/clients/polymarket.rs",
        "src/clients/polymarket/fees.rs",
        "src/config.rs",
        "src/validate.rs",
        "src/platform/resolution_basis.rs",
        "src/bolt_v3_market_identity.rs",
        "tests/ruleset_selector.rs",
    ] {
        assert!(
            !std::path::Path::new(forbidden_path).exists(),
            "{forbidden_path} is a dead default path; bolt-v3 production must keep one runtime path"
        );
    }

    let lib = include_str!("../src/lib.rs");
    assert!(
        !lib.contains("pub mod platform;"),
        "lib must not expose dead platform runtime/reference modules"
    );
    assert!(
        !lib.contains("pub mod live_node_setup;"),
        "lib must not expose dead legacy LiveNode setup"
    );
    assert!(
        !lib.contains("pub mod raw_capture_transport;"),
        "lib must not expose dead legacy raw-capture transport"
    );
    assert!(
        !lib.contains("pub mod bolt_v3_market_identity;"),
        "lib must not expose retired bolt-v3 market-identity module (superseded by bolt_v3_instrument_filters)"
    );
    assert!(
        !lib.contains("pub mod clients;"),
        "lib must not expose dead legacy clients"
    );
    assert!(
        !lib.contains("pub mod config;"),
        "lib must not expose dead legacy config"
    );
    assert!(
        !lib.contains("pub mod validate;"),
        "lib must not expose dead legacy validator"
    );

    let strategy = bolt_v2::bolt_v3_source_integrity::production_module_source_text(
        bolt_v2::bolt_v3_source_integrity::STRATEGY_KEY,
    );
    let strategy = strategy.as_str();
    for forbidden in [
        "runtime_selection_topic",
        "platform.runtime.selection",
        "subscribe_any",
        "try_get_actor_unchecked",
        "\"market_slug\"",
        "\"market_id\"",
        "\"Up\"",
        "\"Down\"",
        "max_buy_execution_within_vwap_slippage_bps",
        "LOT_SIZE_SCALE",
        "trunc_with_scale",
        "adjust_market_buy_amount",
        "compute_maker_taker_amounts",
        "polymarket_clob_lot_size_step",
        "direct amount",
        "lattice",
        "OutcomeSide::Up => self.active.books.up.best_ask,\n            OutcomeSide::Down => self.active.books.down.best_ask,",
        "OrderSide::Buy,\n            PositionSide::Long,\n            OrderSide::Sell,\n            PositionSide::Long,",
    ] {
        assert!(
            !strategy.contains(forbidden),
            "binary oracle strategy must use deeper Module Interfaces instead of inline hardcode `{forbidden}`"
        );
    }
    assert!(
        strategy.contains("provider_normalize_base_order_quantity"),
        "binary oracle strategy must delegate provider-specific base quantity normalization before submit"
    );
}

#[test]
fn production_entrypoint_vocab_does_not_claim_future_or_env_query_path() {
    let checked_surfaces = [
        (
            "src/bolt_v3_live_node.rs",
            include_str!("../src/bolt_v3_live_node.rs"),
        ),
        (
            "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md",
            include_str!("../docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"),
        ),
    ];

    for (path, source) in checked_surfaces {
        for forbidden in [
            "which queries `std::env`",
            "future production v3 entrypoint",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} must not describe bolt-v3 production as future or env-query driven via `{forbidden}`"
            );
        }
    }
}
