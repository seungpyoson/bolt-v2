use std::{
    env, fs,
    path::{Path, PathBuf},
};

use bolt_v2::bolt_v3_release_identity::bolt_v3_compiled_nautilus_trader_revision;

#[test]
fn bolt_v3_polymarket_execution_mapping_requires_resolved_ssm_secrets() {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bolt_v3_providers/polymarket.rs"),
    )
    .expect("bolt-v3 Polymarket provider source should read");
    let map_adapters = source
        .split("pub fn map_adapters")
        .nth(1)
        .expect("Polymarket provider should define map_adapters")
        .split("pub fn build_fee_provider")
        .next()
        .expect("map_adapters should precede fee provider");

    assert!(
        map_adapters.contains("let secrets = secrets_for"),
        "Polymarket execution mapping must require resolved SSM secrets before constructing NT execution config"
    );
    assert!(
        map_adapters.contains("PolymarketExecutionClientFactory"),
        "Polymarket execution mapping must stay on the NT execution client factory path"
    );

    let map_execution = source
        .split("fn map_execution")
        .nth(1)
        .expect("Polymarket provider should define execution mapping")
        .split("fn secrets_for")
        .next()
        .expect("execution mapping should precede secrets lookup helper");
    for required_field in ["private_key", "api_key", "api_secret", "passphrase"] {
        assert!(
            map_execution.contains(required_field),
            "Polymarket execution mapping must pass {required_field} into NT config"
        );
    }
}

#[test]
fn pinned_nt_polymarket_user_channel_is_authenticated_execution_infrastructure() {
    let nt_root = pinned_nt_checkout();
    let ws_client =
        fs::read_to_string(nt_root.join("crates/adapters/polymarket/src/websocket/client.rs"))
            .expect("pinned NT Polymarket websocket client source should read");
    let user_constructor = ws_client
        .split("pub fn new_user")
        .nth(1)
        .expect("Polymarket websocket client should define user-channel constructor")
        .split("fn new_inner")
        .next()
        .expect("user-channel constructor should precede inner constructor");

    assert!(
        user_constructor.contains("credential: Credential"),
        "NT Polymarket user-channel constructor must require credentials"
    );
    assert!(
        user_constructor.contains("WsChannel::User"),
        "NT Polymarket user-channel constructor must create authenticated user channel"
    );

    let ws_handler =
        fs::read_to_string(nt_root.join("crates/adapters/polymarket/src/websocket/handler.rs"))
            .expect("pinned NT Polymarket websocket handler source should read");
    let subscribe_user = ws_handler
        .split("async fn send_subscribe_user")
        .nth(1)
        .expect("Polymarket websocket handler should define user subscribe")
        .split("fn parse_messages")
        .next()
        .expect("user subscribe should precede parser");

    for required_field in ["api_key", "api_secret", "passphrase"] {
        assert!(
            subscribe_user.contains(required_field),
            "NT Polymarket user-channel subscribe must send {required_field}"
        );
    }
    assert!(
        subscribe_user.contains("User channel subscribe requires credential"),
        "NT Polymarket user-channel subscribe must fail closed without credentials"
    );
}

#[test]
fn pinned_nt_polymarket_execution_connects_user_channel_and_account_state() {
    let nt_root = pinned_nt_checkout();
    let source =
        fs::read_to_string(nt_root.join("crates/adapters/polymarket/src/execution/mod.rs"))
            .expect("pinned NT Polymarket execution source should read");
    let connect = source
        .split("async fn connect")
        .nth(1)
        .expect("Polymarket execution client should define connect")
        .split("async fn disconnect")
        .next()
        .expect("connect should precede disconnect");

    assert!(
        connect.contains("self.start_ws_stream().await"),
        "NT Polymarket execution connect must start the execution/user websocket stream"
    );
    assert!(
        connect.contains("self.refresh_account_state().await"),
        "NT Polymarket execution connect must refresh private account state"
    );
    assert!(
        connect.contains("self.await_account_registered"),
        "NT Polymarket execution connect must wait for account registration"
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
