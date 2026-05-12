//! Integration tests for the bolt-v3 client registration boundary.
//!
//! These tests guard the contract that:
//!   1. The bolt-v3 LiveNode build path actually invokes the
//!      registration boundary after adapter mapping.
//!   2. NT client registration only fires after secret resolution and
//!      adapter mapping both succeed; missing or mismatched secrets
//!      surface as the matching `BoltV3LiveNodeError` variant *before*
//!      registration.
//!   3. Registered NT client kinds match the configured client blocks
//!      (verified via `data_engine.registered_clients()` and
//!      `exec_engine.client_ids()` after `LiveNodeBuilder::build`).
//!   4. The registration module source itself does not introduce any
//!      connect / disconnect / run / subscribe / order-submit path.

mod support;

use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with_summary},
    bolt_v3_providers::polymarket,
};
use nautilus_model::identifiers::ClientId;

fn single_client_id_matching(
    loaded: &LoadedBoltV3Config,
    label: &str,
    predicate: impl Fn(&bolt_v2::bolt_v3_config::ClientBlock) -> bool,
) -> String {
    let matches = loaded
        .root
        .clients
        .iter()
        .filter_map(|(client_id, block)| predicate(block).then_some(client_id.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "fixture should define exactly one {label} client, got {matches:?}"
    );
    matches[0].clone()
}

fn fixture_data_and_execution_client_id(loaded: &LoadedBoltV3Config) -> String {
    single_client_id_matching(loaded, "data+execution", |block| {
        block.data.is_some() && block.execution.is_some()
    })
}

fn fixture_data_only_client_id(loaded: &LoadedBoltV3Config) -> String {
    single_client_id_matching(loaded, "data-only", |block| {
        block.data.is_some() && block.execution.is_none()
    })
}

fn fixture_secret_string(loaded: &LoadedBoltV3Config, client_id: &str, field: &str) -> String {
    loaded
        .root
        .clients
        .get(client_id)
        .and_then(|client| client.secrets.as_ref())
        .and_then(toml::Value::as_table)
        .and_then(|secrets| secrets.get(field))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("fixture client {client_id} should define secrets.{field}"))
        .to_string()
}

fn fixture_forbidden_env_var() -> &'static str {
    polymarket::FORBIDDEN_ENV_VARS
        .first()
        .copied()
        .expect("Polymarket provider binding should define forbidden env vars")
}

#[test]
fn live_node_build_path_registers_polymarket_data_polymarket_exec_and_binance_data() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.strategies.clear();
    let data_and_execution_client_id = fixture_data_and_execution_client_id(&loaded);
    let data_only_client_id = fixture_data_only_client_id(&loaded);

    let (node, summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build through the registration boundary");

    // The summary records bolt-v3's intent at the registration boundary.
    assert_eq!(summary.clients.len(), 2, "two configured clients");
    let data_and_execution = summary
        .clients
        .get(&data_and_execution_client_id)
        .expect("data+execution client must appear in summary");
    assert!(
        data_and_execution.data,
        "fixture {data_and_execution_client_id} has a [data] block"
    );
    assert!(
        data_and_execution.execution,
        "fixture {data_and_execution_client_id} has an [execution] block"
    );
    let data_only = summary
        .clients
        .get(&data_only_client_id)
        .expect("data-only client must appear in summary");
    assert!(
        data_only.data,
        "fixture {data_only_client_id} has a [data] block"
    );
    assert!(
        !data_only.execution,
        "fixture {data_only_client_id} has no [execution] block"
    );

    // NT-side state confirms the actual registrations happened. The
    // bolt-v3 client identifier is reused as the NT registration name,
    // so the NT engines expose ClientIds matching those keys. This
    // proves the wiring goes all the way through `factory.create` and
    // `engine.register_client` without a parallel NT mock.
    let registered_data: Vec<ClientId> = node.kernel().data_engine.borrow().registered_clients();
    assert!(
        registered_data.contains(&ClientId::from(data_and_execution_client_id.as_str())),
        "data engine should expose {data_and_execution_client_id}; got {registered_data:?}"
    );
    assert!(
        registered_data.contains(&ClientId::from(data_only_client_id.as_str())),
        "data engine should expose {data_only_client_id}; got {registered_data:?}"
    );

    let registered_exec: Vec<ClientId> = node.kernel().exec_engine.borrow().client_ids();
    assert!(
        registered_exec.contains(&ClientId::from(data_and_execution_client_id.as_str())),
        "exec engine should expose {data_and_execution_client_id}; got {registered_exec:?}"
    );
    assert!(
        !registered_exec.contains(&ClientId::from(data_only_client_id.as_str())),
        "{data_only_client_id} has no [execution] block, must not be on the exec engine; got {registered_exec:?}"
    );
}

#[test]
fn missing_polymarket_private_key_secret_fails_before_registration() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let execution_client_id = fixture_data_and_execution_client_id(&loaded);
    let private_key_path =
        fixture_secret_string(&loaded, &execution_client_id, "private_key_ssm_path");

    // Inject a resolver that fails on the polymarket private_key SSM
    // path. This must surface as `SecretResolution`, never reaching
    // registration.
    let bad_resolver = move |region: &str, path: &str| -> Result<String, &'static str> {
        if path == private_key_path {
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
        |var| var == fixture_forbidden_env_var(),
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
fn empty_clients_root_config_registers_zero_clients() {
    // Build a synthetic root config with zero clients so registration
    // must succeed but produce an empty summary, and the resulting
    // node must expose no registered NT clients.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let empty_root = BoltV3RootConfig {
        clients: BTreeMap::new(),
        ..loaded.root.clone()
    };
    let empty_loaded = LoadedBoltV3Config {
        root_path: loaded.root_path.clone(),
        root: empty_root,
        strategies: Vec::new(),
    };

    // No clients means no SSM paths are touched; the resolver is never
    // called, so the closure body cannot be reached.
    let resolver = |_region: &str, _path: &str| -> Result<String, &'static str> {
        Err("resolver must not be called when no clients are configured")
    };
    let (node, summary) = build_bolt_v3_live_node_with_summary(&empty_loaded, |_| false, resolver)
        .expect("empty client set should still build a clean LiveNode");
    assert!(summary.clients.is_empty());
    assert!(
        node.kernel()
            .data_engine
            .borrow()
            .registered_clients()
            .is_empty()
    );
    assert!(node.kernel().exec_engine.borrow().client_ids().is_empty());
}
