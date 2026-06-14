use std::{collections::BTreeMap, fs};

use backtesting_vertical_slice::nt_catalog_proof::{
    NtCatalogProofReport, run_nt_catalog_proof_from_spec_file_with_resolver,
};
use tempfile::TempDir;

#[test]
fn nt_catalog_proof_round_trips_two_configured_instruments_without_secret_values() {
    let temp_dir = TempDir::new().expect("temp dir");
    let catalog_root = temp_dir.path().join("catalog");
    let output_dir = temp_dir.path().join("proof-output");
    let spec_path = temp_dir.path().join("proof.toml");
    let spec = format!(
        r#"
proof_id = "backtesting-engine-007-long-proof-id-regression"
catalog_uri = "file://{catalog_root}"
output_dir = "{output_dir}"
ticks_per_instrument = 3
base_timestamp_nanos = 1740787200000000000
trade_interval_nanos = 1000000000

[artifact_store]
storage_options = {{}}
rust_storage_options = {{ region = "local-test" }}

[[instruments]]
symbol = "BTCUSDT"
venue = "SIM"
base_currency = "BTC"
quote_currency = "USDT"
price_precision = 2
size_precision = 3
price_increment = "0.01"
size_increment = "0.001"
quantity = "0.500"
price_start = "50000.00"

[[instruments]]
symbol = "ETHUSDT"
venue = "SIM"
base_currency = "ETH"
quote_currency = "USDT"
price_precision = 2
size_precision = 3
price_increment = "0.01"
size_increment = "0.001"
quantity = "1.500"
price_start = "3000.00"
"#,
        catalog_root = catalog_root.display(),
        output_dir = output_dir.display(),
    );
    fs::write(&spec_path, spec).expect("write spec");

    let mut resolved = BTreeMap::new();
    let artifact = run_nt_catalog_proof_from_spec_file_with_resolver(&spec_path, &mut |_, path| {
        resolved.insert(path.to_string(), "called".to_string());
        Ok(format!("secret-for-{path}"))
    })
    .expect("run proof");

    assert!(
        resolved.is_empty(),
        "file-backed proof must not resolve unused SSM secrets"
    );
    let report: NtCatalogProofReport =
        serde_json::from_slice(&fs::read(&artifact.report_path).expect("read report"))
            .expect("parse report");
    assert_eq!(
        report.proof_id,
        "backtesting-engine-007-long-proof-id-regression"
    );
    assert_eq!(
        report.instrument_ids,
        vec!["BTCUSDT.SIM".to_string(), "ETHUSDT.SIM".to_string()]
    );
    assert_eq!(report.expected_instrument_count, 2);
    assert_eq!(report.nt_instrument_count, 2);
    assert_eq!(report.expected_trade_ticks, 6);
    assert_eq!(report.nt_trade_ticks, 6);
    assert_eq!(report.nt_backtest_iterations, 6);
    assert!(!report.direct_s3_catalog_access_proven);
    assert!(
        !report
            .storage_option_keys
            .iter()
            .any(|key| key.contains("secret")),
        "report may include option keys, not secret values"
    );
    assert!(
        !String::from_utf8(fs::read(&artifact.report_path).expect("read report bytes"))
            .expect("utf8")
            .contains("secret-for-"),
        "report must never include resolved secret material"
    );
}

#[test]
fn nt_catalog_proof_rejects_dirty_catalog_root_before_nt_write() {
    let temp_dir = TempDir::new().expect("temp dir");
    let catalog_root = temp_dir.path().join("catalog");
    fs::create_dir_all(&catalog_root).expect("catalog root");
    fs::write(catalog_root.join("unexpected"), b"dirty").expect("dirty marker");
    let output_dir = temp_dir.path().join("proof-output");
    let spec_path = temp_dir.path().join("proof.toml");
    let spec = format!(
        r#"
proof_id = "test-dirty-catalog-proof"
catalog_uri = "file://{catalog_root}"
output_dir = "{output_dir}"
ticks_per_instrument = 1
base_timestamp_nanos = 1740787200000000000
trade_interval_nanos = 1000000000

[artifact_store]
storage_options = {{}}
rust_storage_options = {{ region = "local-test" }}

[[instruments]]
symbol = "BTCUSDT"
venue = "SIM"
base_currency = "BTC"
quote_currency = "USDT"
price_precision = 2
size_precision = 3
price_increment = "0.01"
size_increment = "0.001"
quantity = "0.500"
price_start = "50000.00"

[[instruments]]
symbol = "ETHUSDT"
venue = "SIM"
base_currency = "ETH"
quote_currency = "USDT"
price_precision = 2
size_precision = 3
price_increment = "0.01"
size_increment = "0.001"
quantity = "1.500"
price_start = "3000.00"
"#,
        catalog_root = catalog_root.display(),
        output_dir = output_dir.display(),
    );
    fs::write(&spec_path, spec).expect("write spec");

    let error = run_nt_catalog_proof_from_spec_file_with_resolver(&spec_path, &mut |_, _| {
        Ok("unused-secret".to_string())
    })
    .expect_err("dirty catalog root must be rejected before NT write");
    assert!(
        error.to_string().contains("catalog root is not empty"),
        "unexpected error: {error}"
    );
}
