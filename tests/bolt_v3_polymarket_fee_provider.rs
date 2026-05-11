use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{OnceLock, mpsc},
    time::Duration,
};

mod support;

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_providers::polymarket::{ResolvedBoltV3PolymarketSecrets, build_fee_provider},
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
};
use rust_decimal::Decimal;
use serde::Deserialize;

static FEE_PROVIDER_FIXTURE: OnceLock<PolymarketFeeProviderFixture> = OnceLock::new();

#[derive(Debug, Deserialize)]
struct PolymarketFeeProviderFixture {
    local_fee_provider: LocalFeeProviderFixture,
}

#[derive(Debug, Deserialize)]
struct LocalFeeProviderFixture {
    token_id_suffix: String,
    bind_addr: String,
}

fn fee_provider_fixture() -> &'static PolymarketFeeProviderFixture {
    FEE_PROVIDER_FIXTURE.get_or_init(|| {
        let path = support::repo_path(
            "tests/fixtures/bolt_v3_existing_strategy/polymarket_fee_provider.toml",
        );
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} should read: {error}", path.display()));
        toml::from_str(&text)
            .unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()))
    })
}

fn local_fee_provider_fixture() -> &'static LocalFeeProviderFixture {
    &fee_provider_fixture().local_fee_provider
}

fn existing_strategy_root_fixture() -> PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml")
}

fn loaded_fixture_with_execution_http_base_url(base_url_http: String) -> LoadedBoltV3Config {
    let mut loaded =
        load_bolt_v3_config(&existing_strategy_root_fixture()).expect("v3 fixture should load");
    let execution_client_id = loaded.strategies[0].config.execution_client_id.clone();
    let execution = loaded
        .root
        .clients
        .get_mut(&execution_client_id)
        .and_then(|client| client.execution.as_mut())
        .expect("strategy execution client should have execution config");
    execution
        .as_table_mut()
        .expect("execution config should be TOML table")
        .insert(
            "base_url_http".to_string(),
            toml::Value::String(base_url_http),
        );
    loaded
}

fn strategy_execution_client_id(loaded: &LoadedBoltV3Config) -> String {
    loaded
        .strategies
        .first()
        .expect("fixture should load one strategy")
        .config
        .execution_client_id
        .clone()
}

fn execution_config<'a>(
    loaded: &'a LoadedBoltV3Config,
    execution_client_id: &str,
) -> &'a toml::Value {
    loaded
        .root
        .clients
        .get(execution_client_id)
        .and_then(|client| client.execution.as_ref())
        .expect("strategy execution client should have execution config")
}

fn token_id_from_request(request: &str) -> &str {
    let Some(path) = request.split_ascii_whitespace().nth(1) else {
        panic!("fee-rate request should include path: {request:?}");
    };
    let Some(token_id) = path.strip_prefix("/fee-rate?token_id=") else {
        panic!("fee-rate request should target token fee endpoint: {request:?}");
    };
    token_id
        .split('&')
        .next()
        .expect("fee-rate request token_id should be present")
}

fn fixture_fee_token_id(loaded: &LoadedBoltV3Config) -> String {
    let target = &loaded.strategies[0].config.target;
    let underlying = target
        .get("underlying_asset")
        .and_then(toml::Value::as_str)
        .expect("fixture strategy target should include underlying_asset");
    let suffix = &local_fee_provider_fixture().token_id_suffix;
    let mut token_id = String::with_capacity(underlying.len() + suffix.len());
    token_id.push_str(underlying);
    token_id.push_str(suffix);
    token_id
}

fn configured_execution_http_base_url(
    loaded: &LoadedBoltV3Config,
    execution_client_id: &str,
) -> String {
    execution_config(loaded, execution_client_id)
        .get("base_url_http")
        .and_then(toml::Value::as_str)
        .expect("execution config should include base_url_http")
        .to_string()
}

fn fee_rate_zero_response_fixture() -> String {
    std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3_protocol_payloads/polymarket_fee_rate_zero.json",
    ))
    .expect("Polymarket fee-rate response fixture should be readable")
}

fn spawn_fee_rate_server() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind(local_fee_provider_fixture().bind_addr.as_str())
        .expect("local fee server should bind");
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let body = fee_rate_zero_response_fixture();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("local fee server should accept");
        let mut request = Vec::new();
        loop {
            let mut buffer = [0_u8; 512];
            let read = stream
                .read(&mut buffer)
                .expect("local fee server should read request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8_lossy(&request).into_owned();
        tx.send(request)
            .expect("local fee server should record request");

        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("local fee server should write response");
    });

    (base_url, rx)
}

#[test]
fn polymarket_fee_provider_uses_configured_execution_http_base_url() {
    let (base_url, requests) = spawn_fee_rate_server();
    let loaded = loaded_fixture_with_execution_http_base_url(base_url.clone());
    let execution_client_id = strategy_execution_client_id(&loaded);
    let execution = execution_config(&loaded, &execution_client_id);
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM");
    let secrets = resolved
        .get_as::<ResolvedBoltV3PolymarketSecrets>(&execution_client_id)
        .expect("strategy execution client should resolve Polymarket secrets");
    let token_id = fixture_fee_token_id(&loaded);
    let provider = build_fee_provider(
        execution,
        secrets,
        loaded.root.nautilus.timeout_connection_seconds,
    )
    .expect("fee provider should build from execution config");

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(provider.warm(&token_id))
        .expect("fee provider should warm through configured local HTTP base URL");

    assert_eq!(provider.fee_bps(&token_id), Some(Decimal::ZERO));
    let request = requests
        .recv_timeout(Duration::from_secs(
            loaded.root.nautilus.timeout_connection_seconds,
        ))
        .expect("configured local fee server should receive fee-rate request");
    assert_eq!(
        configured_execution_http_base_url(&loaded, &execution_client_id),
        base_url
    );
    assert_eq!(token_id_from_request(&request), token_id);
}
