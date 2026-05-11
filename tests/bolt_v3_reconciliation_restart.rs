mod support;

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config, bolt_v3_live_node::make_live_node_config,
    bolt_v3_release_identity::bolt_v3_compiled_nautilus_trader_revision,
};

#[test]
fn bolt_v3_maps_toml_reconciliation_settings_to_nt_live_config() {
    let loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy v3 TOML should load");
    let config = make_live_node_config(&loaded);
    let expected = &loaded.root.nautilus.exec_engine;

    assert_eq!(config.exec_engine.reconciliation, expected.reconciliation);
    assert_eq!(
        config.exec_engine.reconciliation_startup_delay_secs,
        expected.reconciliation_startup_delay_seconds as f64
    );
    assert_eq!(
        config.exec_engine.filter_unclaimed_external_orders,
        expected.filter_unclaimed_external_orders
    );
    assert_eq!(
        config.exec_engine.generate_missing_orders,
        expected.generate_missing_orders
    );
}

#[test]
fn pinned_nt_startup_reconciliation_registers_external_orders_with_execution_clients() {
    let nt_root = pinned_nt_checkout();
    let node_source = fs::read_to_string(nt_root.join("crates/live/src/node.rs"))
        .expect("pinned NT live node source should read");
    let reconciliation_block = node_source
        .split("async fn perform_startup_reconciliation")
        .nth(1)
        .expect("pinned NT live node should define startup reconciliation")
        .split("async fn run_reconciliation_checks")
        .next()
        .expect("startup reconciliation block should precede executor init");

    assert!(
        reconciliation_block.contains("reconcile_execution_mass_status"),
        "NT startup reconciliation should reconcile mass status through live manager"
    );
    assert!(
        reconciliation_block.contains("result.external_orders"),
        "NT startup reconciliation should surface external orders from reconciliation result"
    );
    assert!(
        reconciliation_block.contains("exec_engine.register_external_order"),
        "NT startup reconciliation should hand external orders back to execution clients"
    );
}

#[test]
fn pinned_nt_polymarket_can_generate_mass_status_but_does_not_track_external_orders() {
    let nt_root = pinned_nt_checkout();
    let source =
        fs::read_to_string(nt_root.join("crates/adapters/polymarket/src/execution/mod.rs"))
            .expect("pinned NT Polymarket execution source should read");
    let mass_status_method = source
        .split("async fn generate_mass_status")
        .nth(1)
        .expect("Polymarket execution client should implement mass status generation")
        .split("fn process_cancel_result")
        .next()
        .expect("mass status method should precede cancel helper");

    assert!(
        mass_status_method.contains("reconciliation::generate_mass_status"),
        "Polymarket adapter should delegate mass-status generation to its reconciliation module"
    );

    let register_method = source
        .split("fn register_external_order")
        .nth(1)
        .expect("Polymarket execution client should implement external-order registration hook")
        .split("fn on_instrument")
        .next()
        .expect("external-order registration hook should precede instrument callback");
    let register_body = register_method
        .split_once('{')
        .and_then(|(_, rest)| rest.rsplit_once('}').map(|(body, _)| body))
        .expect("external-order registration hook body should parse");
    let non_empty_lines = register_body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    assert!(
        non_empty_lines.is_empty(),
        "Polymarket external-order registration hook should currently be empty; \
         if upstream NT implements this, F10 blocker status must be re-evaluated: {non_empty_lines:?}"
    );
}

fn pinned_nt_checkout() -> PathBuf {
    let revision =
        bolt_v3_compiled_nautilus_trader_revision().expect("Cargo.toml should pin one NT revision");
    let short_revision = revision
        .get(..7)
        .expect("NT revision should be at least 7 chars");
    let cargo_home = cargo_home();
    let checkouts = cargo_home.join("git/checkouts");

    for entry in fs::read_dir(&checkouts).expect("Cargo git checkouts dir should read") {
        let entry = entry.expect("Cargo git checkout entry should read");
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !file_name.starts_with("nautilus_trader-") {
            continue;
        }
        let candidate = entry.path().join(short_revision);
        if candidate.is_dir() {
            return candidate;
        }
    }

    panic!(
        "pinned NT checkout {short_revision} not found under {}; run cargo fetch/test first",
        checkouts.display()
    );
}

fn cargo_home() -> PathBuf {
    if let Some(path) = env::var_os("CARGO_HOME") {
        return PathBuf::from(path);
    }

    let home = env::var_os("HOME").expect("HOME should be set when CARGO_HOME is unset");
    Path::new(&home).join(".cargo")
}
