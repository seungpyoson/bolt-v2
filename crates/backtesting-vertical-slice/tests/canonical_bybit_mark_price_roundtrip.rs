//! Round-trip proof for the Bybit mark-price kline converter.
//!
//! Parses a committed hermetic fixture (downsampled from a real staged Bybit
//! `mark_price_kline_1m` object) into NautilusTrader `Bar`s with
//! `PriceType::Mark`, writes them into a temporary NautilusTrader
//! `ParquetDataCatalog`, queries them back, and asserts the round-tripped count,
//! ascending order, and per-record payload equality (loaded == expected). That
//! proves the venue's mark-price candles land in an NT catalog NautilusTrader can
//! replay, with a distinct `…-MARK-…` `BarType` that never collides with the
//! `…-LAST-…` trade-kline projection.
//!
//! Hermetic: the test reads the committed fixture, never S3.

use std::path::PathBuf;

use backtesting_vertical_slice::{
    canonical_bybit::{
        BybitInstrumentSpec, NT_DATA_TYPE_MARK_BAR, normalize_bybit_mark_price_kline_1m,
        project_bybit_mark_bars_to_catalog, read_back_mark_bars,
    },
    source_proof::{
        AcceptanceMode, AcceptedDataset, EvidenceState, FixtureType, IngestManifestObjectRecord,
        NtMappingStatus, RequiredCheck, RequiredChecks, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
    },
};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("bybit")
        .join(name)
}

/// Build an accepted dataset whose provenance matches a real staged object and
/// whose coverage window brackets `archive_date`.
#[allow(clippy::too_many_arguments)]
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

/// BNBUSDT linear-perp mark-price kline fixture: `mark_price_kline_1m` JSON ->
/// Bar (PriceType::Mark) -> NT catalog -> query back.
#[test]
fn mark_price_kline_bars_roundtrip_through_nt_catalog() {
    let json_text =
        std::fs::read_to_string(fixture_path("mark_price_kline_1m.json")).expect("read json");
    // The committed fixture is downsampled (newest 40 candles, exact envelope
    // shape preserved) from the real staged object d154952f…09fe21 (source=rest,
    // family=mark_price_kline_1m, category=linear, BNBUSDT, dt window 2026-03-13).
    let accepted = accepted_dataset(
        "bybit-mark-price-kline-1m",
        "linear",
        "linear",
        "bars",
        "s3://bolt-parquet/backfill-staging/2026-06-01/bybit/raw/v1/source=rest/family=mark_price_kline_1m/category=linear/page_end=2026-03-13T23_59_59Z/page_start=2026-03-13T12_00_00Z/symbol=BNBUSDT/window_end=2026-03-13T23_59_59Z/window_start=2026-03-01T00_00_00Z/object=d154952ffa87bc3cc3d6ee0cea053ef78d46465cb99c584a5e61dbd86509fe21.json",
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
    let spec = BybitInstrumentSpec {
        instrument_id: "BNBUSDT".to_string(),
        venue_symbol: "BNBUSDT".to_string(),
        nt_instrument_id: "BNBUSDT.BYBIT".to_string(),
        price_increment: "0.01".to_string(),
        size_increment: "0.001".to_string(),
    };

    let table =
        normalize_bybit_mark_price_kline_1m(&accepted, &spec, &json_text).expect("normalize mark");
    let parsed_count = table.rows.len();
    assert!(parsed_count > 0, "fixture must carry candles");
    // Sorted ascending: each open_time strictly increases by 60s.
    for window in table.rows.windows(2) {
        assert_eq!(
            window[1].open_time - window[0].open_time,
            60_000_000_000,
            "1-minute mark candles must be 60s apart and ascending"
        );
    }

    // Expected NT bars computed directly from the canonical table, so the
    // round-trip assertion is per-record payload equality (loaded == expected).
    let expected = table.to_bars(&spec).expect("expected bars");
    assert_eq!(expected.len(), parsed_count);

    let bar_type = table.bar_type_string().expect("bar type string");
    // The mark projection must carry NautilusTrader's native `MARK` price type,
    // distinct from the trade kline's `LAST` id.
    assert!(
        bar_type.contains("MARK"),
        "mark bar type {bar_type:?} must carry the MARK price type"
    );
    assert!(
        !bar_type.contains("LAST"),
        "mark bar type {bar_type:?} must not collide with the trade-kline LAST id"
    );

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection =
        project_bybit_mark_bars_to_catalog(&table, &spec, dir.path()).expect("project mark bars");
    assert_eq!(projection.data_type, NT_DATA_TYPE_MARK_BAR);
    assert_eq!(projection.record_count, parsed_count);
    assert_eq!(projection.nt_identifier, bar_type);

    let loaded = read_back_mark_bars(dir.path(), &bar_type).expect("read back mark bars");
    assert_eq!(
        loaded.len(),
        parsed_count,
        "round-trip bar count must match"
    );

    // Per-record payload equality: NautilusTrader returns the exact same bars.
    assert_eq!(
        loaded, expected,
        "round-tripped mark bars must equal expected"
    );

    for bar in &loaded {
        assert_eq!(bar.bar_type.to_string(), bar_type);
        // OHLC integrity survives the round trip.
        assert!(bar.high >= bar.open && bar.high >= bar.low && bar.high >= bar.close);
        assert!(bar.low <= bar.open && bar.low <= bar.close);
        // Mark-price candle carries a zero traded volume by convention.
        assert!(bar.volume.is_zero(), "mark bar volume must be zero");
    }
    for window in loaded.windows(2) {
        assert!(
            window[0].ts_init <= window[1].ts_init,
            "mark bars must be ascending"
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
fn mark_price_kline_reject_symbol_mismatch() {
    let json_text =
        std::fs::read_to_string(fixture_path("mark_price_kline_1m.json")).expect("read json");
    let accepted = accepted_dataset(
        "bybit-mark-price-kline-1m",
        "linear",
        "linear",
        "bars",
        "s3://example/object.json",
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
    let wrong_spec = BybitInstrumentSpec {
        instrument_id: "BTCUSDT".to_string(),
        venue_symbol: "BTCUSDT".to_string(),
        nt_instrument_id: "BTCUSDT.BYBIT".to_string(),
        price_increment: "0.1".to_string(),
        size_increment: "0.001".to_string(),
    };
    let err = normalize_bybit_mark_price_kline_1m(&accepted, &wrong_spec, &json_text)
        .expect_err("symbol mismatch must be rejected");
    assert!(err.to_string().contains("symbol"), "{err}");
}
