//! Round-trip proof for the Bybit derivatives-trade and kline converters.
//!
//! Parses committed hermetic fixtures (downsampled from real staged Bybit
//! objects) into the correct NautilusTrader types, writes them into a temporary
//! NautilusTrader `ParquetDataCatalog`, queries them back, and asserts the
//! round-tripped count and ordering match. That proves the venue's staged data
//! lands in an NT catalog NautilusTrader can replay = backtestable.
//!
//! Hermetic: the tests read the committed fixtures, never S3.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::{
    canonical_bybit::{
        BybitInstrumentSpec, NT_DATA_TYPE_BAR, NT_DATA_TYPE_TRADE_TICK,
        append_bybit_deriv_tick_trades_archive, append_bybit_mark_price_kline_1m_archive,
        normalize_bybit_deriv_tick_trades, normalize_bybit_kline_1m,
        normalize_bybit_mark_price_kline_1m, project_bybit_bars_to_catalog,
        project_bybit_trades_to_catalog, read_back_bars, read_back_mark_bars,
        read_back_trade_ticks,
    },
    source_proof::{
        AcceptanceMode, AcceptedDataset, EvidenceState, FixtureType, IngestManifestObjectRecord,
        NtMappingStatus, RequiredCheck, RequiredChecks, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
    },
};
use flate2::{Compression, write::GzEncoder};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("bybit")
        .join(name)
}

/// Build an accepted dataset whose provenance matches a real staged object and
/// whose coverage window brackets `archive_date`.
fn accepted_dataset(
    source_binding: &str,
    product_family: &str,
    product_category: &str,
    table_family: &str,
    s3_uri: &str,
    sha256: &str,
    bytes: u64,
    archive_date: &str,
    coverage_start: &str,
    coverage_end: &str,
    schema_columns: Vec<String>,
) -> AcceptedDataset {
    let checks = RequiredChecks {
        source_access: RequiredCheck::passed("manifest"),
        license: RequiredCheck::passed("attestation"),
        schema: RequiredCheck::passed("schema"),
        time_semantics: RequiredCheck::passed("time"),
        instrument_universe: RequiredCheck::passed("universe"),
        coverage: RequiredCheck::passed("manifest"),
        granularity: RequiredCheck::passed("native"),
        completeness: RequiredCheck::passed("manifest"),
        nt_mapping: RequiredCheck::passed("nt"),
        storage: RequiredCheck::passed("artifact_root"),
    };
    let object = IngestManifestObjectRecord {
        s3_uri: s3_uri.to_string(),
        source_url: format!("https://example.invalid/{archive_date}"),
        sha256: sha256.to_string(),
        bytes,
        archive_date: archive_date.to_string(),
        schema_columns,
    };
    let proof = SourceProofReport {
        source_proof_id: format!("source-proof-{source_binding}"),
        source_proof_version: 1,
        contract_version: "backfill-table-contract.v1".to_string(),
        schema_version: "backfill-source-proof.v1".to_string(),
        status: SourceProofStatus::Pending,
        source_binding: source_binding.to_string(),
        venue: "bybit".to_string(),
        product_family: product_family.to_string(),
        product_category: product_category.to_string(),
        table_family: table_family.to_string(),
        evidence_state: EvidenceState::OwnerArchiveBackfillable,
        fixture_type: FixtureType::PerpsSpot,
        requested_time_range: TimeRange {
            start_utc: coverage_start.to_string(),
            end_utc: coverage_end.to_string(),
        },
        coverage_time_range: TimeRange {
            start_utc: coverage_start.to_string(),
            end_utc: coverage_end.to_string(),
        },
        instrument_universe_id: "bybit-instruments".to_string(),
        raw_sample_uri: object.s3_uri.clone(),
        raw_sample_hash: object.sha256.clone(),
        schema_sample_uri: "s3://example/schema.json".to_string(),
        schema_sample_hash: "deadbeef".to_string(),
        license_ref: "https://example.invalid/ (attestation)".to_string(),
        retention_ref: "https://example.invalid/".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::TradeBarReplay,
        forbidden_claims: vec!["No execution-quality or queue-position claims.".to_string()],
        gap_policy_id: String::new(),
        required_checks: checks,
        acceptance_mode: None,
        accepted_by: None,
        accepted_at: None,
        supersedes_source_proof_id: None,
    }
    .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
    .expect("accept proof");
    select_accepted_dataset(&proof, &object, sha256).expect("select accepted dataset")
}

/// DOGEUSDT-05JUN26 linear-perp trade fixture: derivatives tick-trades CSV ->
/// TradeTick -> NT catalog -> query back.
#[test]
fn deriv_trades_roundtrip_through_nt_catalog() {
    let csv_text =
        std::fs::read_to_string(fixture_path("tick_trades_linear.csv")).expect("read csv");
    // The committed fixture is the decompressed body of the real staged object
    // 69081e42...524503.csv.gz (category=linear, dt=2026-05-21, DOGEUSDT-05JUN26).
    let accepted = accepted_dataset(
        "bybit-linear-tick-trades",
        "linear",
        "linear",
        "trades",
        "s3://bolt-parquet/backfill-staging/2026-06-01/bybit/raw/v1/source=public_archive/family=tick_trades/category=linear/dt=2026-05-21/symbol=DOGEUSDT-05JUN26/object=69081e42e095ac886bf656a731082d7644382423eb92b77b8d12d3688a524503.csv.gz",
        "69081e42e095ac886bf656a731082d7644382423eb92b77b8d12d3688a524503",
        1550,
        "2026-05-21",
        "2026-05-01T00:00:00Z",
        "2026-06-01T00:00:00Z",
        vec![
            "timestamp".to_string(),
            "symbol".to_string(),
            "side".to_string(),
            "size".to_string(),
            "price".to_string(),
            "tickDirection".to_string(),
            "trdMatchID".to_string(),
        ],
    );
    let spec = BybitInstrumentSpec {
        instrument_id: "DOGEUSDT-05JUN26".to_string(),
        venue_symbol: "DOGEUSDT-05JUN26".to_string(),
        nt_instrument_id: "DOGEUSDT-05JUN26.BYBIT".to_string(),
        price_increment: "0.00001".to_string(),
        size_increment: "1".to_string(),
    };

    let table = normalize_bybit_deriv_tick_trades(&accepted, &spec, &csv_text).expect("normalize");
    let parsed_count = table.rows.len();
    assert!(parsed_count > 0, "fixture must carry trades");
    // First parsed row matches the first source row (ascending).
    assert_eq!(table.rows[0].event_time, 1_779_321_780_432_400_000);
    assert_eq!(table.rows[0].price, "0.10421");
    assert_eq!(table.rows[0].size, "51");

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection =
        project_bybit_trades_to_catalog(&table, &spec, dir.path()).expect("project trades");
    assert_eq!(projection.data_type, NT_DATA_TYPE_TRADE_TICK);
    assert_eq!(projection.record_count, parsed_count);
    assert_eq!(projection.nt_identifier, "DOGEUSDT-05JUN26.BYBIT");

    let loaded = read_back_trade_ticks(dir.path(), "DOGEUSDT-05JUN26.BYBIT").expect("read back");
    assert_eq!(loaded.len(), parsed_count, "round-trip count must match");
    for tick in &loaded {
        assert_eq!(tick.instrument_id.to_string(), "DOGEUSDT-05JUN26.BYBIT");
    }
    // Ordering: NT read-back is ascending by ts_init.
    for window in loaded.windows(2) {
        assert!(
            window[0].ts_init <= window[1].ts_init,
            "trade ticks must be ascending"
        );
    }
    // First/last round-tripped event time match the canonical table edges.
    assert_eq!(
        u64::from(loaded[0].ts_event),
        u64::try_from(table.rows[0].event_time).unwrap()
    );
    assert_eq!(
        u64::from(loaded[parsed_count - 1].ts_event),
        u64::try_from(table.rows[parsed_count - 1].event_time).unwrap()
    );
}

/// ETHMNT spot kline fixture: kline_1m JSON -> Bar -> NT catalog -> query back.
#[test]
fn kline_1m_bars_roundtrip_through_nt_catalog() {
    let json_text = std::fs::read_to_string(fixture_path("kline_1m.json")).expect("read json");
    // The committed fixture is the real staged object 2b6c4baa...426542f.json
    // (source=rest, family=kline_1m, category=spot, ETHMNT).
    let accepted = accepted_dataset(
        "bybit-kline-1m",
        "spot",
        "spot",
        "bars",
        "s3://bolt-parquet/backfill-staging/2026-06-01/bybit/raw/v1/source=rest/family=kline_1m/category=spot/symbol=ETHMNT/object=2b6c4baa56a5afadc31d0b78ca839686ebd19598e17839ea15e7344c1426542f.json",
        "2b6c4baa56a5afadc31d0b78ca839686ebd19598e17839ea15e7344c1426542f",
        4751,
        "2026-06-01",
        "2026-05-01T00:00:00Z",
        "2026-06-02T00:00:00Z",
        vec![
            "start".to_string(),
            "open".to_string(),
            "high".to_string(),
            "low".to_string(),
            "close".to_string(),
            "volume".to_string(),
            "turnover".to_string(),
        ],
    );
    let spec = BybitInstrumentSpec {
        instrument_id: "ETHMNT".to_string(),
        venue_symbol: "ETHMNT".to_string(),
        nt_instrument_id: "ETHMNT.BYBIT".to_string(),
        price_increment: "0.01".to_string(),
        size_increment: "0.00001".to_string(),
    };

    let table = normalize_bybit_kline_1m(&accepted, &spec, &json_text).expect("normalize kline");
    let parsed_count = table.rows.len();
    assert!(parsed_count > 0, "fixture must carry candles");
    // Sorted ascending: each open_time strictly increases by 60s.
    for window in table.rows.windows(2) {
        assert_eq!(
            window[1].open_time - window[0].open_time,
            60_000_000_000,
            "1-minute candles must be 60s apart and ascending"
        );
    }

    let bar_type = table.bar_type_string().expect("bar type string");
    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection =
        project_bybit_bars_to_catalog(&table, &spec, dir.path()).expect("project bars");
    assert_eq!(projection.data_type, NT_DATA_TYPE_BAR);
    assert_eq!(projection.record_count, parsed_count);
    assert_eq!(projection.nt_identifier, bar_type);

    let loaded = read_back_bars(dir.path(), &bar_type).expect("read back bars");
    assert_eq!(
        loaded.len(),
        parsed_count,
        "round-trip bar count must match"
    );
    for bar in &loaded {
        assert_eq!(bar.bar_type.to_string(), bar_type);
        // OHLC integrity survives the round trip.
        assert!(bar.high >= bar.open && bar.high >= bar.low && bar.high >= bar.close);
        assert!(bar.low <= bar.open && bar.low <= bar.close);
    }
    for window in loaded.windows(2) {
        assert!(
            window[0].ts_init <= window[1].ts_init,
            "bars must be ascending"
        );
    }
    // First/last round-tripped open time match the canonical table edges.
    assert_eq!(
        u64::from(loaded[0].ts_event),
        u64::try_from(table.rows[0].open_time).unwrap()
    );
    assert_eq!(
        u64::from(loaded[parsed_count - 1].ts_event),
        u64::try_from(table.rows[parsed_count - 1].open_time).unwrap()
    );
}

/// A symbol-mismatched instrument spec must be rejected, proving the converter
/// binds rows to the accepted instrument rather than trusting the file blindly.
#[test]
fn deriv_trades_reject_symbol_mismatch() {
    let csv_text =
        std::fs::read_to_string(fixture_path("tick_trades_linear.csv")).expect("read csv");
    let accepted = accepted_dataset(
        "bybit-linear-tick-trades",
        "linear",
        "linear",
        "trades",
        "s3://example/object.csv.gz",
        "69081e42e095ac886bf656a731082d7644382423eb92b77b8d12d3688a524503",
        1550,
        "2026-05-21",
        "2026-05-01T00:00:00Z",
        "2026-06-01T00:00:00Z",
        vec![
            "timestamp".to_string(),
            "symbol".to_string(),
            "side".to_string(),
            "size".to_string(),
            "price".to_string(),
            "tickDirection".to_string(),
            "trdMatchID".to_string(),
        ],
    );
    let wrong_spec = BybitInstrumentSpec {
        instrument_id: "BTCUSDT".to_string(),
        venue_symbol: "BTCUSDT".to_string(),
        nt_instrument_id: "BTCUSDT.BYBIT".to_string(),
        price_increment: "0.1".to_string(),
        size_increment: "0.001".to_string(),
    };
    let err = normalize_bybit_deriv_tick_trades(&accepted, &wrong_spec, &csv_text)
        .expect_err("symbol mismatch must be rejected");
    assert!(err.to_string().contains("symbol"), "{err}");
}

/// Gzip a byte slice the way the staged `.csv.gz` archive object is encoded, so
/// the bulk-append test exercises the real decompress path on the EXISTING
/// committed (plain-text) fixture without adding a redundant gz fixture.
fn gzip_bytes(plain: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(plain).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

/// Recursively collect every file under `root`.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Bulk path (derivatives trades): derive precision from the object's own rows
/// (Bybit stages no instrument universe), build identity/provenance from the S3
/// object key, append into a shared catalog with NO clean-root guard, and prove
/// the NautilusTrader round-trip is lossless.
#[test]
fn deriv_trades_data_derived_append_round_trips() {
    let csv_text =
        std::fs::read_to_string(fixture_path("tick_trades_linear.csv")).expect("read csv");
    let gz = gzip_bytes(csv_text.as_bytes());
    // The real staged trade-archive S3 object key (category=linear, dt=2026-05-21,
    // DOGEUSDT-05JUN26); symbol/category/date + provenance are read from it.
    let object_key = "backfill-staging/2026-06-01/bybit/raw/v1/source=public_archive/family=tick_trades/category=linear/symbol=DOGEUSDT-05JUN26/dt=2026-05-21/object=69081e42e095ac886bf656a731082d7644382423eb92b77b8d12d3688a524503.csv.gz";
    let nt_inst = "DOGEUSDT-05JUN26.BYBIT";

    // Independent expectation from the same source, via the data-derived spec.
    let accepted = accepted_dataset(
        "bybit-linear-tick-trades",
        "linear",
        "linear",
        "trades",
        object_key,
        "69081e42e095ac886bf656a731082d7644382423eb92b77b8d12d3688a524503",
        1550,
        "2026-05-21",
        "2026-05-01T00:00:00Z",
        "2026-06-01T00:00:00Z",
        vec![
            "timestamp".to_string(),
            "symbol".to_string(),
            "side".to_string(),
            "size".to_string(),
            "price".to_string(),
            "tickDirection".to_string(),
            "trdMatchID".to_string(),
        ],
    );
    // DOGE perp prints a 5-dp price tick and integer contract size; precision is
    // read from the data, not assumed.
    let derived = BybitInstrumentSpec {
        instrument_id: "DOGEUSDT-05JUN26".to_string(),
        venue_symbol: "DOGEUSDT-05JUN26".to_string(),
        nt_instrument_id: nt_inst.to_string(),
        price_increment: "0.00001".to_string(),
        size_increment: "1".to_string(),
    };
    let table =
        normalize_bybit_deriv_tick_trades(&accepted, &derived, &csv_text).expect("normalize");
    let expected = table.to_trade_ticks(&derived).expect("expected ticks");
    assert!(!expected.is_empty(), "fixture must carry trades");

    // Append into a freshly-opened (empty) catalog — no dirty-root refusal.
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries = append_bybit_deriv_tick_trades_archive(&gz, object_key, &mut catalog)
        .expect("append trades");
    assert_eq!(summaries.len(), 1, "a trade archive object is one symbol");
    assert_eq!(summaries[0].nt_instrument_id, nt_inst);
    assert_eq!(summaries[0].record_count, expected.len());
    // Precision is read from the data and self-consistent with the ticks.
    assert_eq!(summaries[0].price_precision, 5);
    assert_eq!(summaries[0].size_precision, 0);
    assert_eq!(summaries[0].price_precision, expected[0].price.precision);
    assert_eq!(summaries[0].size_precision, expected[0].size.precision);

    let loaded = read_back_trade_ticks(dir.path(), nt_inst).expect("read back ticks");
    assert_eq!(loaded.len(), expected.len(), "round-tripped tick count");
    assert!(
        loaded.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded ticks must be ascending"
    );
    assert_eq!(
        loaded, expected,
        "data-derived append must round-trip identically (count, ordering, payload, precision)"
    );
    assert!(
        walk(dir.path()).iter().any(|p| {
            p.to_string_lossy().contains("trade")
                && p.extension().map(|e| e == "parquet").unwrap_or(false)
        }),
        "catalog must contain a native trade-tick parquet file"
    );
}

/// Bulk path (mark-price klines): derive price precision from the object's own
/// OHLC columns, build identity/provenance from the S3 object key, append into a
/// shared catalog with NO clean-root guard, and prove the NautilusTrader
/// round-trip is lossless with the distinct `…-MARK-…` bar-type id.
#[test]
fn mark_price_kline_data_derived_append_round_trips() {
    let json_text =
        std::fs::read_to_string(fixture_path("mark_price_kline_1m.json")).expect("read json");
    let json_bytes = json_text.as_bytes();
    // The real staged mark-price-kline S3 object key (category=linear, BNBUSDT);
    // symbol/category/date + provenance are read from it.
    let object_key = "backfill-staging/2026-06-01/bybit/raw/v1/source=rest/family=mark_price_kline_1m/category=linear/page_end=2026-03-13T23_59_59Z/page_start=2026-03-13T12_00_00Z/symbol=BNBUSDT/window_end=2026-03-13T23_59_59Z/window_start=2026-03-01T00_00_00Z/object=d154952ffa87bc3cc3d6ee0cea053ef78d46465cb99c584a5e61dbd86509fe21.json";

    let accepted = accepted_dataset(
        "bybit-mark-price-kline-1m",
        "linear",
        "linear",
        "bars",
        object_key,
        "d154952ffa87bc3cc3d6ee0cea053ef78d46465cb99c584a5e61dbd86509fe21",
        37626,
        "2026-03-13",
        "2026-03-01T00:00:00Z",
        "2026-03-14T00:00:00Z",
        vec![
            "start".to_string(),
            "open".to_string(),
            "high".to_string(),
            "low".to_string(),
            "close".to_string(),
        ],
    );
    // BNBUSDT mark candles render a 2-dp price; a mark candle has no traded size,
    // so size precision is data-derived as 0.
    let derived = BybitInstrumentSpec {
        instrument_id: "BNBUSDT".to_string(),
        venue_symbol: "BNBUSDT".to_string(),
        nt_instrument_id: "BNBUSDT.BYBIT".to_string(),
        price_increment: "0.01".to_string(),
        size_increment: "1".to_string(),
    };
    let table = normalize_bybit_mark_price_kline_1m(&accepted, &derived, &json_text)
        .expect("normalize mark");
    let expected = table.to_bars(&derived).expect("expected bars");
    assert!(!expected.is_empty(), "fixture must carry candles");
    let bar_type = table.bar_type_string().expect("bar type string");
    assert!(bar_type.contains("MARK"), "mark bar type {bar_type:?}");

    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries = append_bybit_mark_price_kline_1m_archive(json_bytes, object_key, &mut catalog)
        .expect("append mark bars");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].nt_instrument_id, "BNBUSDT.BYBIT");
    assert_eq!(summaries[0].record_count, expected.len());
    assert_eq!(summaries[0].price_precision, 2);
    assert_eq!(summaries[0].size_precision, 0);
    assert_eq!(summaries[0].price_precision, expected[0].open.precision);

    let loaded = read_back_mark_bars(dir.path(), &bar_type).expect("read back mark bars");
    assert_eq!(loaded.len(), expected.len(), "round-tripped bar count");
    assert!(
        loaded.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded bars must be ascending"
    );
    assert_eq!(
        loaded, expected,
        "data-derived append must round-trip identically (count, ordering, payload, precision)"
    );
    for bar in &loaded {
        assert_eq!(bar.bar_type.to_string(), bar_type);
        assert!(bar.volume.is_zero(), "mark bar volume must be zero");
    }
    assert!(
        walk(dir.path()).iter().any(|p| {
            p.to_string_lossy().contains("bar")
                && p.extension().map(|e| e == "parquet").unwrap_or(false)
        }),
        "catalog must contain a native bar parquet file"
    );
}
