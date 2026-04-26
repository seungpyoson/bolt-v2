//! Integration tests for the bolt-v3 client registration boundary.
//!
//! These tests guard the contract that:
//!   1. The bolt-v3 LiveNode build path actually invokes the
//!      registration boundary after adapter mapping.
//!   2. NT client registration only fires after secret resolution and
//!      adapter mapping both succeed; missing or mismatched secrets
//!      surface as the matching `BoltV3LiveNodeError` variant *before*
//!      registration.
//!   3. Registered NT client kinds match the configured venue blocks
//!      (verified via `data_engine.registered_clients()` and
//!      `exec_engine.client_ids()` after `LiveNodeBuilder::build`).
//!   4. The registration module source itself does not introduce any
//!      connect / disconnect / run / subscribe / order-submit path.

mod support;

use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_client_registration::BoltV3RegisteredVenue,
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with_summary},
};
use nautilus_model::identifiers::ClientId;

#[test]
fn live_node_build_path_registers_polymarket_data_polymarket_exec_and_binance_data() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    let (node, summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build through the registration boundary");

    // The summary records bolt-v3's intent at the registration boundary.
    assert_eq!(summary.venues.len(), 2, "two configured venues");
    match summary
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must appear in summary")
    {
        BoltV3RegisteredVenue::Polymarket { data, execution } => {
            assert!(*data, "fixture polymarket_main has a [data] block");
            assert!(
                *execution,
                "fixture polymarket_main has an [execution] block"
            );
        }
        other => panic!("expected Polymarket entry, got {other:?}"),
    }
    match summary
        .venues
        .get("binance_reference")
        .expect("binance_reference must appear in summary")
    {
        BoltV3RegisteredVenue::Binance { data } => {
            assert!(*data, "fixture binance_reference has a [data] block");
        }
        other => panic!("expected Binance entry, got {other:?}"),
    }

    // NT-side state confirms the actual registrations happened. The
    // bolt-v3 venue identifier is reused as the NT registration name,
    // so the NT engines expose ClientIds matching those keys. This
    // proves the wiring goes all the way through `factory.create` and
    // `engine.register_client` without a parallel NT mock.
    let registered_data: Vec<ClientId> = node.kernel().data_engine.borrow().registered_clients();
    assert!(
        registered_data.contains(&ClientId::from("polymarket_main")),
        "data engine should expose polymarket_main; got {registered_data:?}"
    );
    assert!(
        registered_data.contains(&ClientId::from("binance_reference")),
        "data engine should expose binance_reference; got {registered_data:?}"
    );

    let registered_exec: Vec<ClientId> = node.kernel().exec_engine.borrow().client_ids();
    assert!(
        registered_exec.contains(&ClientId::from("polymarket_main")),
        "exec engine should expose polymarket_main; got {registered_exec:?}"
    );
    assert!(
        !registered_exec.contains(&ClientId::from("binance_reference")),
        "binance_reference has no [execution] block, must not be on the exec engine; got {registered_exec:?}"
    );
}

#[test]
fn missing_polymarket_private_key_secret_fails_before_registration() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    // Inject a resolver that fails on the polymarket private_key SSM
    // path. This must surface as `SecretResolution`, never reaching
    // registration.
    let bad_resolver = |region: &str, path: &str| -> Result<String, &'static str> {
        if path == "/bolt/polymarket_main/private_key" {
            Err("simulated SSM permissions denied for polymarket private key")
        } else {
            support::fake_bolt_v3_resolver(region, path)
        }
    };
    let error = build_bolt_v3_live_node_with_summary(&loaded, |_| false, bad_resolver)
        .expect_err("missing private key must block before registration");

    assert!(
        matches!(error, BoltV3LiveNodeError::SecretResolution(_)),
        "expected SecretResolution variant, got {error:?}"
    );
}

#[test]
fn forbidden_credential_env_var_fails_before_registration() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    // Forbidden env-var detection is the very first gate; registration
    // must never run when a credential env var is set.
    let error = build_bolt_v3_live_node_with_summary(
        &loaded,
        |var| var == "POLYMARKET_PK",
        support::fake_bolt_v3_resolver,
    )
    .expect_err("forbidden env var must block before registration");
    assert!(
        matches!(error, BoltV3LiveNodeError::ForbiddenEnv(_)),
        "expected ForbiddenEnv variant, got {error:?}"
    );
}

#[test]
fn registration_module_remains_a_no_trade_boundary() {
    // Source-level inspection of the registration module. The module
    // is allowed to name NT factories (that is the whole point), but
    // it must never call connect, disconnect, run, subscribe, market
    // selection, order construction, or order submission. Forbidden
    // tokens live in this integration test (not in the module's own
    // source) so the assertion does not self-trip.
    let source = include_str!("../src/bolt_v3_client_registration.rs");
    for forbidden in [
        ".connect(",
        ".disconnect(",
        "node.run(",
        ".start(",
        ".stop(",
        "subscribe_quote_ticks",
        "subscribe_trade_ticks",
        "subscribe_order_book_deltas",
        "subscribe_order_book_snapshots",
        "subscribe_instruments",
        "select_market",
        "submit_order",
        "submit_order_list",
        "modify_order",
        "cancel_order",
        "OrderBuilder",
        "PolymarketOrderBuilder",
        "OrderSubmitter",
    ] {
        assert!(
            !source.contains(forbidden),
            "src/bolt_v3_client_registration.rs must remain a no-trade boundary; \
             source unexpectedly references `{forbidden}`"
        );
    }
}

#[test]
fn empty_venues_root_config_registers_zero_clients() {
    // Build a synthetic root config with zero venues so registration
    // must succeed but produce an empty summary, and the resulting
    // node must expose no registered NT clients.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let empty_root = BoltV3RootConfig {
        venues: BTreeMap::new(),
        ..loaded.root.clone()
    };
    let empty_loaded = LoadedBoltV3Config {
        root_path: loaded.root_path.clone(),
        root: empty_root,
        strategies: Vec::new(),
    };

    // No venues means no SSM paths are touched; the resolver is never
    // called, so the closure body cannot be reached.
    let resolver = |_region: &str, _path: &str| -> Result<String, &'static str> {
        Err("resolver must not be called when no venues are configured")
    };
    let (node, summary) = build_bolt_v3_live_node_with_summary(&empty_loaded, |_| false, resolver)
        .expect("empty venue set should still build a clean LiveNode");
    assert!(summary.venues.is_empty());
    assert!(
        node.kernel()
            .data_engine
            .borrow()
            .registered_clients()
            .is_empty()
    );
    assert!(node.kernel().exec_engine.borrow().client_ids().is_empty());
}
