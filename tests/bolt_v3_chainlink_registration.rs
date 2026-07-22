//! Provider-registration test for the `CHAINLINK_DATA_STREAMS` strike source.
//!
//! Modeled on `tests/bolt_v3_client_registration.rs`, this guards that a
//! configured Chainlink Data Streams strike client (a `[data]` block plus the
//! `[secrets]` it consumes, with feed bindings supplied by the shared root
//! catalog) registers as a data-only NT client through the bolt-v3 LiveNode
//! build path: secret resolution and adapter mapping both succeed, the
//! `ChainlinkStrikeSourceFactory` builds a `DataClient`, and the NT data engine
//! exposes the matching `ClientId` — with no execution client created (the
//! strike source declares no `[execution]` block).

use crate::support;

use bolt_v2::{
    bolt_v3_config::{ClientBlock, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::build_bolt_v3_all_configured_client_mapping_live_node_with_summary,
};
use nautilus_model::identifiers::ClientId;

const CHAINLINK_CLIENT_KEY: &str = "chainlink_strike";
const CHAINLINK_API_KEY_SSM_PATH: &str = "/bolt/chainlink_strike/api_key";
const CHAINLINK_API_SECRET_SSM_PATH: &str = "/bolt/chainlink_strike/api_secret";

fn chainlink_client_toml() -> String {
    format!(
        r#"
venue = "CHAINLINK_DATA_STREAMS"

[data]
rest_base_url = "https://api.example.com/"
report_endpoint_path = "/api/v1/reports/bulk"
http_timeout_secs = 5

[secrets]
api_key_ssm_parameter = "{CHAINLINK_API_KEY_SSM_PATH}"
api_secret_ssm_parameter = "{CHAINLINK_API_SECRET_SSM_PATH}"
"#
    )
}

fn loaded_config_with_chainlink_client() -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let client: ClientBlock =
        toml::from_str(&chainlink_client_toml()).expect("chainlink test client should parse");
    loaded
        .root
        .clients
        .insert(CHAINLINK_CLIENT_KEY.to_string(), client);
    loaded
}

/// Resolver that supplies the Chainlink strike SSM credential paths in addition
/// to the shared fixture paths, mirroring the wrapping-resolver pattern used by
/// the client-registration tests. Returns synthetic non-secret material; no
/// real credential values are involved.
fn chainlink_resolver(region: &str, path: &str) -> Result<String, &'static str> {
    match path {
        CHAINLINK_API_KEY_SSM_PATH => Ok("chainlink-strike-api-key".to_string()),
        CHAINLINK_API_SECRET_SSM_PATH => Ok("chainlink-strike-api-secret".to_string()),
        _ => support::fake_bolt_v3_resolver(region, path),
    }
}

#[test]
fn chainlink_strike_client_registers_as_data_only_via_provider_binding() {
    let mut loaded = loaded_config_with_chainlink_client();
    let temp = support::TempCaseDir::new("bolt-v3-chainlink-strike-registration");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    support::current_evidence::prepare_current_evidence_generation(&loaded);

    let (node, summary) = build_bolt_v3_all_configured_client_mapping_live_node_with_summary(
        &loaded,
        |_| false,
        chainlink_resolver,
    )
    .expect("chainlink strike client should register through the LiveNode boundary");

    let row = summary
        .clients
        .get(CHAINLINK_CLIENT_KEY)
        .expect("chainlink strike client must appear in the registration summary");
    assert!(
        row.data,
        "the chainlink strike client must register as data-capable"
    );
    assert!(
        !row.execution,
        "the chainlink strike source is data-only and must not register an execution client"
    );

    let registered_data = node.registered_data_client_ids();
    assert!(
        registered_data.contains(&ClientId::from(CHAINLINK_CLIENT_KEY)),
        "the NT data engine must expose the chainlink strike client; got {registered_data:?}"
    );

    let registered_exec = node.registered_exec_client_ids();
    assert!(
        !registered_exec.contains(&ClientId::from(CHAINLINK_CLIENT_KEY)),
        "the data-only chainlink strike client must not appear on the exec engine; got {registered_exec:?}"
    );
}
