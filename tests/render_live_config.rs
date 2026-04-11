mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use bolt_v2::{MaterializationOutcome, config::Config, materialize_live_config};
use support::{TempCaseDir, live_local_chainlink_operator_input, repo_path};

#[test]
fn materialize_live_config_creates_read_only_output() {
    let tempdir = TempCaseDir::new("create-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should create the runtime config");

    assert_eq!(outcome, MaterializationOutcome::Created);
    assert!(output_path.exists());
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn materialize_live_config_creates_missing_parent_directories() {
    let tempdir = TempCaseDir::new("nested-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("nested/runtime/live.toml");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should create nested output directories");

    assert_eq!(outcome, MaterializationOutcome::Created);
    assert!(output_path.exists());
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn materialize_live_config_updates_drifted_contents() {
    let tempdir = TempCaseDir::new("updated-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    materialize_live_config(&input_path, &output_path).expect("first render should succeed");

    #[cfg(unix)]
    set_mode(&output_path, 0o600);
    #[cfg(not(unix))]
    make_writable_if_needed(&output_path);
    fs::write(&output_path, "drifted = true\n").expect("drifted output should be writable");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should repair drifted contents");

    assert_eq!(outcome, MaterializationOutcome::Updated);
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
    #[cfg(unix)]
    assert_mode(&output_path, 0o400);
}

#[test]
fn materialize_live_config_repairs_permissions_without_rewriting_contents() {
    let tempdir = TempCaseDir::new("permissions-repaired");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    materialize_live_config(&input_path, &output_path).expect("first render should succeed");

    let modified_before = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");
    thread::sleep(Duration::from_millis(20));

    #[cfg(unix)]
    set_mode(&output_path, 0o600);
    #[cfg(not(unix))]
    make_writable_if_needed(&output_path);

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should repair writable drift");

    let modified_after = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");

    assert_eq!(outcome, MaterializationOutcome::PermissionsRepaired);
    assert_eq!(
        modified_before, modified_after,
        "permission repair should not rewrite contents"
    );
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
    #[cfg(unix)]
    assert_mode(&output_path, 0o400);
}

#[test]
fn materialize_live_config_leaves_matching_read_only_output_unchanged() {
    let tempdir = TempCaseDir::new("unchanged-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    materialize_live_config(&input_path, &output_path).expect("first render should succeed");

    let modified_before = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");
    thread::sleep(Duration::from_millis(20));

    let outcome =
        materialize_live_config(&input_path, &output_path).expect("second render should succeed");

    let modified_after = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");

    assert_eq!(outcome, MaterializationOutcome::Unchanged);
    assert_eq!(
        modified_before, modified_after,
        "unchanged output should not be rewritten"
    );
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn render_live_config_binary_supports_relative_paths() {
    let tempdir = TempCaseDir::new("relative-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let _ = input_path;

    let output = Command::new(env!("CARGO_BIN_EXE_render_live_config"))
        .current_dir(tempdir.path())
        .args(["--input", "live.local.toml", "--output", "live.toml"])
        .output()
        .expect("renderer binary should run");

    assert!(
        output.status.success(),
        "binary failed: stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let output_path = tempdir.path().join("live.toml");
    assert!(output_path.exists());
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn legacy_operator_config_without_phase1_sections_still_materializes() {
    let tempdir = TempCaseDir::new("legacy-phase1-compat");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = r#"
[node]
name = "BOLT-V2-TEST"
trader_id = "BOLT-TEST"

[polymarket]
event_slug = "btc-updown-5m"
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"
"#;

    fs::write(&input_path, input).expect("input config should be written");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("legacy operator config should still materialize");

    assert_eq!(outcome, MaterializationOutcome::Created);

    let rendered = fs::read_to_string(&output_path).expect("rendered config should be readable");
    assert!(!rendered.contains("\n[reference]\n"));
    assert!(!rendered.contains("\n[[rulesets]]\n"));
    assert!(!rendered.contains("\n[audit]\n"));
    assert_generated_output(&output_path);
}

#[test]
fn materialize_live_config_rejects_rendered_runtime_validation_failures() {
    let tempdir = TempCaseDir::new("runtime-validation-failure");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = r#"
[node]
name = "BOLT-V2-TEST"
trader_id = "BOLT-TEST"

[polymarket]
event_slug = "btc-updown-5m"
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"

[reference]
publish_topic = "platform.reference.default"
min_publish_interval_ms = 100

[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 0.35
stale_after_ms = 1500
disable_after_ms = 5000

[[rulesets]]
id = "PRIMARY"
venue = "polymarket"
tag_slug = "bitcoin"
resolution_basis = "kraken_btcusd_1m"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 1000
candidate_load_timeout_secs = 30

[audit]
local_dir = "var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
upload_attempt_timeout_secs = 30
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#;

    fs::write(&input_path, input).expect("input config should be written");

    let error = materialize_live_config(&input_path, &output_path)
        .expect_err("runtime-invalid rendered config should fail materialization")
        .to_string();

    assert!(
        error.contains("Runtime config validation failed"),
        "unexpected materialization error: {error}"
    );
    assert!(
        error.contains("rulesets[0].resolution_basis"),
        "runtime validation error should mention resolution_basis: {error}"
    );
    assert!(
        !output_path.exists(),
        "materialization should not write output on runtime validation failure"
    );
}

#[test]
fn render_live_config_binary_resolves_contract_path_from_repo_root() {
    let tempdir = TempCaseDir::new("relative-contract-root");
    std::fs::write(
        tempdir.path().join("Cargo.toml"),
        "[package]\nname = \"temp\"\n",
    )
    .expect("repo marker should be written");
    std::fs::create_dir_all(tempdir.path().join("config")).expect("config dir should exist");
    std::fs::create_dir_all(tempdir.path().join("contracts")).expect("contracts dir should exist");
    std::fs::write(
        tempdir.path().join("contracts/polymarket.toml"),
        "schema_version = 1\nvenue = \"test\"\nadapter_version = \"bolt-v2\"\n\n\
         [streams.quotes]\ncapability = \"supported\"\npolicy = \"required\"\n\n\
         [streams.trades]\ncapability = \"supported\"\npolicy = \"required\"\n\n\
         [streams.order_book_deltas]\ncapability = \"supported\"\npolicy = \"required\"\n\n\
         [streams.order_book_depths]\ncapability = \"unsupported\"\n\n\
         [streams.index_prices]\ncapability = \"unsupported\"\n\n\
         [streams.mark_prices]\ncapability = \"unsupported\"\n\n\
         [streams.instrument_closes]\ncapability = \"unsupported\"\n",
    )
    .expect("contract fixture should be written");

    let source = tracked_live_local_example()
        .replace(
            "# contract_path = \"contracts/polymarket.toml\"",
            "contract_path = \"contracts/polymarket.toml\"",
        )
        .replace("catalog_path = \"\"", "catalog_path = \"var/catalog\"");
    fs::write(tempdir.path().join("config/live.local.toml"), source)
        .expect("input config should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_render_live_config"))
        .current_dir(tempdir.path())
        .args(["--input", "config/live.local.toml", "--output", "live.toml"])
        .output()
        .expect("renderer binary should run");

    assert!(
        output.status.success(),
        "binary failed: stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let rendered = fs::read_to_string(tempdir.path().join("live.toml"))
        .expect("rendered config should be readable");
    let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");
    let expected_root = fs::canonicalize(tempdir.path()).expect("tempdir should resolve");
    assert_eq!(
        cfg.streaming.contract_path.as_deref(),
        Some(
            expected_root
                .join("contracts/polymarket.toml")
                .to_str()
                .expect("absolute contract path should be utf-8")
        )
    );
}

#[cfg(unix)]
#[test]
fn render_live_config_binary_respects_restrictive_umask_on_create() {
    let tempdir = TempCaseDir::new("umask-create");
    let _input_path = write_input(&tempdir, "live.local.toml");

    let output = Command::new("/bin/sh")
        .current_dir(tempdir.path())
        .env("BIN", env!("CARGO_BIN_EXE_render_live_config"))
        .env("INPUT", "live.local.toml")
        .env("OUTPUT", "live.toml")
        .arg("-c")
        .arg("umask 077 && \"$BIN\" --input \"$INPUT\" --output \"$OUTPUT\"")
        .output()
        .expect("renderer binary should run with restrictive umask");

    assert!(
        output.status.success(),
        "binary failed: stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let output_path = tempdir.path().join("live.toml");
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
    assert_mode(&output_path, 0o400);
}

#[test]
fn materialize_live_config_renders_nested_chainlink_reference_settings() {
    let tempdir = TempCaseDir::new("render-chainlink-reference");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = live_local_chainlink_operator_input();

    fs::write(&input_path, input).expect("input config should be written");
    materialize_live_config(&input_path, &output_path)
        .expect("chainlink operator config should materialize");

    let rendered = fs::read_to_string(&output_path).expect("rendered config should be readable");
    let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");
    let shared = cfg
        .reference
        .chainlink
        .as_ref()
        .expect("rendered shared chainlink config should be present");
    let chainlink = cfg.reference.venues[0]
        .chainlink
        .as_ref()
        .expect("rendered chainlink config should be present");

    assert!(rendered.contains("[reference.chainlink]"));
    assert!(rendered.contains("[reference.venues.chainlink]"));
    assert_eq!(shared.region, "us-east-1");
    assert_eq!(shared.api_key, "/bolt/chainlink/api_key");
    assert_eq!(shared.api_secret, "/bolt/chainlink/api_secret");
    assert_eq!(shared.ws_url, "wss://streams.chain.link");
    assert_eq!(shared.ws_reconnect_alert_threshold, 5);
    assert_eq!(
        chainlink.feed_id,
        "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
    );
    assert_eq!(chainlink.price_scale, 8);
}

#[test]
fn materialize_live_config_preserves_valid_chainlink_ws_fallback_origins() {
    let tempdir = TempCaseDir::new("render-chainlink-ws-fallback-origins");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = live_local_chainlink_operator_input().replace(
        "ws_url = \"wss://streams.chain.link\"",
        "ws_url = \"wss://primary.chain.link,wss://fallback.chain.link\"",
    );

    fs::write(&input_path, input).expect("input config should be written");
    materialize_live_config(&input_path, &output_path)
        .expect("chainlink operator config with ws fallback origins should materialize");

    let rendered = fs::read_to_string(&output_path).expect("rendered config should be readable");
    let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");
    let shared = cfg
        .reference
        .chainlink
        .as_ref()
        .expect("rendered shared chainlink config should be present");

    assert_eq!(
        shared.ws_url,
        "wss://primary.chain.link,wss://fallback.chain.link"
    );
}

#[test]
fn materialize_live_config_rejects_invalid_chainlink_ws_fallback_origin() {
    let tempdir = TempCaseDir::new("invalid-chainlink-ws-fallback-origin");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = live_local_chainlink_operator_input().replace(
        "ws_url = \"wss://streams.chain.link\"",
        "ws_url = \"wss://primary.chain.link,ws://fallback.chain.link\"",
    );

    fs::write(&input_path, input).expect("input config should be written");
    let error = materialize_live_config(&input_path, &output_path)
        .expect_err("invalid chainlink ws fallback origin should fail validation")
        .to_string();

    assert!(error.contains("reference.chainlink.ws_url"));
    assert!(error.contains("must start with wss://"));
}

fn write_input(tempdir: &TempCaseDir, file_name: &str) -> PathBuf {
    let input_path = tempdir.path().join(file_name);
    fs::write(&input_path, tracked_live_local_example()).expect("input config should be written");
    input_path
}

fn tracked_live_local_example() -> String {
    fs::read_to_string(repo_path("config/live.local.example.toml"))
        .expect("tracked operator template should be readable")
}

fn assert_generated_output(path: &Path) {
    let rendered = fs::read_to_string(path).expect("generated output should be readable");
    assert!(rendered.contains("# GENERATED FILE - DO NOT EDIT."));
    assert!(rendered.contains("# Source of truth:"));
    assert!(rendered.contains("[[data_clients]]"));
    assert!(rendered.contains("[[exec_clients]]"));
    assert!(rendered.contains("[[strategies]]"));
}

fn assert_read_only(path: &Path) {
    let metadata = fs::metadata(path).expect("output metadata should exist");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(metadata.permissions().mode() & 0o222, 0);
    }

    #[cfg(not(unix))]
    {
        assert!(metadata.permissions().readonly());
    }
}

#[cfg(not(unix))]
fn make_writable_if_needed(path: &Path) {
    let mut permissions = fs::metadata(path)
        .expect("output metadata should exist")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).expect("output should become writable");
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .expect("output metadata should exist")
        .permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).expect("output mode should be updated");
}

#[cfg(unix)]
fn assert_mode(path: &Path, expected_mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    let actual_mode = fs::metadata(path)
        .expect("output metadata should exist")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(actual_mode, expected_mode);
}
