use std::fs;

use backtesting_vertical_slice::source_archive_discovery_seed::{
    SourceArchiveDiscoverySeed, SourceArchiveDiscoverySeedStatus,
    write_source_archive_discovery_seed_from_spec_file,
};

#[test]
fn source_archive_discovery_seed_records_bindings_and_representative_objects() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("seed");
    let spec_path = temp_dir.path().join("source-archive-discovery-seed.toml");
    fs::write(
        &spec_path,
        format!(
            r#"
discovery_id = "source-archive-discovery-seed-binance-data-vision-trades-current"
venue = "binance"
source = "data-vision"
window_start = "2026-03-01"
window_end = "2026-03-01"
output_dir = "{output_dir}"

[[binding]]
source_binding = "binance-spot-native-trades"
product_family = "spot"
table_family = "native_trades"
source_uri_template = "https://data.binance.vision/data/spot/daily/trades/{{symbol}}/{{symbol}}-trades-{{dt}}.zip"
list_prefix = "data/spot/daily/trades/"

[binding.representative_object]
symbol = "BNBUSDC"
archive_date = "2026-03-01"
source_url = "https://data.binance.vision/data/spot/daily/trades/BNBUSDC/BNBUSDC-trades-2026-03-01.zip"
http_status = 200
content_length_bytes = 1066394
last_modified = "Sun, 01 Mar 2026 05:13:20 GMT"
etag = "\"sample-spot\""

[[binding]]
source_binding = "binance-usd-m-perpetual-native-trades"
product_family = "usd_m_perpetual"
table_family = "native_trades"
source_uri_template = "https://data.binance.vision/data/futures/um/daily/trades/{{symbol}}/{{symbol}}-trades-{{dt}}.zip"
list_prefix = "data/futures/um/daily/trades/"

[binding.representative_object]
symbol = "BTCUSDT"
archive_date = "2026-03-01"
source_url = "https://data.binance.vision/data/futures/um/daily/trades/BTCUSDT/BTCUSDT-trades-2026-03-01.zip"
http_status = 200
content_length_bytes = 45927411
last_modified = "Sun, 01 Mar 2026 05:13:20 GMT"
etag = "\"sample-usdm-perp\""

[[binding]]
source_binding = "binance-usd-m-delivery-native-trades"
product_family = "usd_m_delivery"
table_family = "native_trades"
source_uri_template = "https://data.binance.vision/data/futures/um/daily/trades/{{symbol}}/{{symbol}}-trades-{{dt}}.zip"
list_prefix = "data/futures/um/daily/trades/"

[binding.representative_object]
symbol = "BTCUSDT_260327"
archive_date = "2026-03-01"
source_url = "https://data.binance.vision/data/futures/um/daily/trades/BTCUSDT_260327/BTCUSDT_260327-trades-2026-03-01.zip"
http_status = 200
content_length_bytes = 179484
last_modified = "Sun, 01 Mar 2026 05:13:20 GMT"
etag = "\"sample-usdm-delivery\""

[[binding]]
source_binding = "binance-coin-m-perpetual-native-trades"
product_family = "coin_m_perpetual"
table_family = "native_trades"
source_uri_template = "https://data.binance.vision/data/futures/cm/daily/trades/{{symbol}}/{{symbol}}-trades-{{dt}}.zip"
list_prefix = "data/futures/cm/daily/trades/"

[binding.representative_object]
symbol = "BTCUSD_PERP"
archive_date = "2026-03-01"
source_url = "https://data.binance.vision/data/futures/cm/daily/trades/BTCUSD_PERP/BTCUSD_PERP-trades-2026-03-01.zip"
http_status = 200
content_length_bytes = 5221954
last_modified = "Sun, 01 Mar 2026 05:13:20 GMT"
etag = "\"sample-coinm-perp\""

[[binding]]
source_binding = "binance-coin-m-delivery-native-trades"
product_family = "coin_m_delivery"
table_family = "native_trades"
source_uri_template = "https://data.binance.vision/data/futures/cm/daily/trades/{{symbol}}/{{symbol}}-trades-{{dt}}.zip"
list_prefix = "data/futures/cm/daily/trades/"

[binding.representative_object]
symbol = "BTCUSD_260327"
archive_date = "2026-03-01"
source_url = "https://data.binance.vision/data/futures/cm/daily/trades/BTCUSD_260327/BTCUSD_260327-trades-2026-03-01.zip"
http_status = 200
content_length_bytes = 257829
last_modified = "Sun, 01 Mar 2026 05:13:20 GMT"
etag = "\"sample-coinm-delivery\""
"#,
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let first = write_source_archive_discovery_seed_from_spec_file(&spec_path).expect("first");
    let second = write_source_archive_discovery_seed_from_spec_file(&spec_path).expect("second");
    assert_eq!(first.content_hash, second.content_hash);

    let seed: SourceArchiveDiscoverySeed =
        serde_json::from_slice(&fs::read(&first.path).expect("read seed")).expect("seed parses");
    assert_eq!(seed.schema_version, "source-archive-discovery-seed.v1");
    assert_eq!(
        seed.discovery_id,
        "source-archive-discovery-seed-binance-data-vision-trades-current"
    );
    assert_eq!(seed.status, SourceArchiveDiscoverySeedStatus::Ready);
    assert_eq!(seed.source_binding_count, 5);
    assert_eq!(seed.representative_object_count, 5);
    assert_eq!(seed.total_representative_object_bytes, 52_653_072);
    assert_eq!(
        seed.product_families,
        vec![
            "coin_m_delivery",
            "coin_m_perpetual",
            "spot",
            "usd_m_delivery",
            "usd_m_perpetual",
        ]
    );
    assert!(
        seed.bindings
            .iter()
            .all(|binding| binding.representative_object.http_status == 200)
    );
    assert!(
        seed.bindings
            .iter()
            .all(|binding| binding.source_uri_template.contains("{symbol}")
                && binding.source_uri_template.contains("{dt}"))
    );
}

#[test]
fn source_archive_discovery_seed_rejects_duplicate_representative_objects() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("seed");
    let spec_path = temp_dir.path().join("source-archive-discovery-seed.toml");
    fs::write(
        &spec_path,
        format!(
            r#"
discovery_id = "source-archive-discovery-seed-duplicate"
venue = "synthetic"
source = "archive"
window_start = "2026-03-01"
window_end = "2026-03-01"
output_dir = "{output_dir}"

[[binding]]
source_binding = "synthetic-a"
product_family = "spot"
table_family = "native_trades"
source_uri_template = "https://example.test/data/{{symbol}}/{{dt}}.zip"
list_prefix = "data/a/"

[binding.representative_object]
symbol = "AAA"
archive_date = "2026-03-01"
source_url = "https://example.test/data/AAA/2026-03-01.zip"
http_status = 200
content_length_bytes = 1

[[binding]]
source_binding = "synthetic-b"
product_family = "linear"
table_family = "native_trades"
source_uri_template = "https://example.test/data/{{symbol}}/{{dt}}.zip"
list_prefix = "data/b/"

[binding.representative_object]
symbol = "BBB"
archive_date = "2026-03-01"
source_url = "https://example.test/data/AAA/2026-03-01.zip"
http_status = 200
content_length_bytes = 1
"#,
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let err = write_source_archive_discovery_seed_from_spec_file(&spec_path)
        .expect_err("duplicate representative URL must fail");
    assert!(
        err.to_string().contains("duplicate representative object"),
        "{err:#}"
    );
}
