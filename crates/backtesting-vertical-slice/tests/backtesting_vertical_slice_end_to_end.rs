//! End-to-end proof for the NautilusTrader backtesting vertical slice.
//!
//! Drives gates 1 through 6 through [`run_backtest`] with a small deterministic
//! accepted dataset: source-proof acceptance, the accepted-data ledger,
//! canonical normalization + Parquet artifact, NautilusTrader catalog projection
//! + read-back, manifest validation + NautilusTrader config mapping, a
//! `BacktestNode` run of a compiled Rust strategy, and the objective result
//! contract. CI-safe (no network): this test exercises the pipeline with
//! committed synthetic data only. Real-object verification is operator-run-only
//! via the `backtesting_vertical_slice` binary (`--run-spec`, `--object-gz`,
//! `--output-dir`) and is not asserted here.

use std::collections::BTreeMap;

use backtesting_vertical_slice::{
    canonical_trades::{
        CanonicalInstrumentIdentity, ConverterConfig, CsvTimestampUnit, CsvTradeMappingConfig,
        TRANSFORM_IDENTITY,
    },
    catalog_projection::SpotInstrumentSpec,
    result_contract::ResultArtifactUris,
    run_manifest::{
        BacktestingRunManifest, ManifestCatalogInput, ManifestVenueConfig, MarketStructureFixture,
        RunPurpose, STRATEGY_HURST_VPIN_DIRECTIONAL, StrategySource,
    },
    runner::{BacktestRunInputs, run_backtest},
    source_proof::{
        AcceptanceMode, AcceptedDataset, EvidenceState, FixtureType, IngestManifestObjectRecord,
        NtMappingStatus, RequiredCheck, RequiredChecks, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
    },
};

const SAMPLE_CSV: &str = "id,timestamp,price,volume,side,rpi\n\
    1,1772323201665,617.2,0.3,buy,0\n\
    2,1772323312219,617.9,0.1456,sell,0\n\
    3,1772323312236,617,0.1544,sell,0\n";

fn csv_mapping() -> CsvTradeMappingConfig {
    CsvTradeMappingConfig {
        trade_id_column: "id".to_string(),
        timestamp_column: "timestamp".to_string(),
        timestamp_unit: CsvTimestampUnit::Milliseconds,
        price_column: "price".to_string(),
        size_column: "volume".to_string(),
        side_column: "side".to_string(),
        buyer_side_values: vec!["buy".to_string()],
        seller_side_values: vec!["sell".to_string()],
    }
}

fn converter_config() -> ConverterConfig {
    ConverterConfig {
        identity: TRANSFORM_IDENTITY.to_string(),
        version: "1".to_string(),
        csv: csv_mapping(),
    }
}

const OBJECT_SHA256: &str = "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598";
const SOURCE_PROOF_ID: &str = "source-proof-bybit-spot-tick-trades";

fn passing_checks() -> RequiredChecks {
    let evidence = "manifest://bybit-backfill-run-fdcc0758bbd03113";
    RequiredChecks {
        source_access: RequiredCheck::passed(evidence),
        license: RequiredCheck::passed("attestation://bybit-public-archive"),
        schema: RequiredCheck::passed("schema://id,timestamp,price,volume,side,rpi"),
        time_semantics: RequiredCheck::passed("unix_ms_to_unix_nanos"),
        instrument_universe: RequiredCheck::passed("universe://bybit-spot-2026-03-01"),
        coverage: RequiredCheck::passed(evidence),
        granularity: RequiredCheck::passed("native_trade_prints"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("nt://TradeTick"),
        storage: RequiredCheck::passed("s3://bolt-parquet/.../source-proofs/"),
    }
}

fn accepted_dataset() -> AcceptedDataset {
    let object = IngestManifestObjectRecord {
        s3_uri: "s3://bolt-parquet/backfill-staging/2026-06-01/bybit/raw/v1/source=public_archive/family=tick_trades/category=spot/dt=2026-03-01/symbol=BNBUSDC/object=d6af93.csv.gz".to_string(),
        source_url: "https://public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz".to_string(),
        sha256: OBJECT_SHA256.to_string(),
        bytes: 8505,
        archive_date: "2026-03-01".to_string(),
        schema_columns: ["id", "timestamp", "price", "volume", "side", "rpi"]
            .iter()
            .map(ToString::to_string)
            .collect(),
    };
    let proof = SourceProofReport {
        source_proof_id: SOURCE_PROOF_ID.to_string(),
        source_proof_version: 1,
        contract_version: "backfill-table-contract.v1".to_string(),
        schema_version: "backfill-source-proof.v1".to_string(),
        status: SourceProofStatus::Pending,
        source_binding: "bybit-spot-tick-trades".to_string(),
        venue: "bybit".to_string(),
        product_family: "spot".to_string(),
        product_category: "spot".to_string(),
        table_family: "trades".to_string(),
        evidence_state: EvidenceState::OwnerArchiveBackfillable,
        fixture_type: FixtureType::PerpsSpot,
        requested_time_range: TimeRange {
            start_utc: "2025-06-01T00:00:00Z".to_string(),
            end_utc: "2026-06-01T00:00:00Z".to_string(),
        },
        coverage_time_range: TimeRange {
            start_utc: "2026-03-01T00:00:00Z".to_string(),
            end_utc: "2026-03-02T00:00:00Z".to_string(),
        },
        instrument_universe_id: "bybit-spot-instruments-2026-03-01".to_string(),
        raw_sample_uri: object.s3_uri.clone(),
        raw_sample_hash: object.sha256.clone(),
        schema_sample_uri: "s3://bolt-parquet/.../schema-sample.json".to_string(),
        schema_sample_hash: "bf26db0b8fb8b62746b5724dccfb26a408d581f5598cb6be95c9173c8b1b5eed"
            .to_string(),
        license_ref: "https://public.bybit.com/ (attestation 2026-06-02)".to_string(),
        retention_ref: "https://public.bybit.com/ (retention reviewed 2026-06-02)".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        // Mirrors the committed reference source proof exactly so the fixture
        // carries the full set of fidelity constraints through to the contract.
        forbidden_claims: vec![
            "No execution-quality, queue-position, or order-book-liquidity claims.".to_string(),
            "No L2/L3 order-book replay claims from trade prints.".to_string(),
            "Coverage is limited to BNBUSDC spot 2026-03-01; no multi-day or multi-instrument claims."
                .to_string(),
        ],
        gap_policy_id: String::new(),
        required_checks: passing_checks(),
        acceptance_mode: None,
        accepted_by: None,
        accepted_at: None,
        supersedes_source_proof_id: None,
    }
    .accept(
        AcceptanceMode::Manual,
        "vertical-slice-operator",
        "2026-06-02T00:00:00Z",
    )
    .expect("accept source proof");
    select_accepted_dataset(&proof, &object, OBJECT_SHA256).expect("select accepted dataset")
}

fn instrument_spec() -> SpotInstrumentSpec {
    SpotInstrumentSpec {
        nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        raw_symbol: "BNBUSDC".to_string(),
        base_currency: "BNB".to_string(),
        quote_currency: "USDC".to_string(),
        price_increment: "0.1".to_string(),
        size_increment: "0.0001".to_string(),
        min_quantity: "0.0001".to_string(),
        max_quantity: "1400".to_string(),
        min_notional: "5".to_string(),
        max_notional: "200000".to_string(),
    }
}

fn manifest(catalog_path: &str) -> BacktestingRunManifest {
    BacktestingRunManifest {
        run_id: "backtesting-vertical-slice-end-to-end".to_string(),
        market_structure_fixture: MarketStructureFixture::PerpsSpot,
        venue_binding_key: "bybit-spot-tick-trades".to_string(),
        run_purpose: RunPurpose::Normal,
        source_proof_id: SOURCE_PROOF_ID.to_string(),
        source_proof_version: 1,
        pins_non_latest_proof: false,
        proof_pin_reason_code: None,
        proof_pin_reason_detail: None,
        strategy: StrategySource {
            registry_key: STRATEGY_HURST_VPIN_DIRECTIONAL.to_string(),
            parameters: BTreeMap::from([
                ("trade_size".to_string(), "0.01".to_string()),
                (
                    "bar_type".to_string(),
                    "BNBUSDC.BYBIT-1-MINUTE-LAST-INTERNAL".to_string(),
                ),
            ]),
        },
        venue: ManifestVenueConfig {
            nt_venue: "BYBIT".to_string(),
            oms_type: "NETTING".to_string(),
            account_type: "CASH".to_string(),
            book_type: "L1_MBP".to_string(),
            starting_balances: vec!["1_000_000 USDC".to_string()],
            routing: false,
            frozen_account: false,
            reject_stop_orders: true,
            support_gtd_orders: true,
            support_contingent_orders: true,
            use_position_ids: true,
            use_random_ids: false,
            use_reduce_only: true,
            bar_execution: true,
            bar_adaptive_high_low_ordering: false,
            trade_execution: true,
            use_market_order_acks: false,
            liquidity_consumption: false,
            allow_cash_borrowing: false,
            queue_position: false,
            oto_trigger_mode: "PARTIAL".to_string(),
            base_currency: "NONE".to_string(),
            default_leverage: "1".to_string(),
            price_protection_points: 0,
        },
        catalog_input: ManifestCatalogInput {
            catalog_path: catalog_path.to_string(),
            catalog_fs_protocol: "NONE".to_string(),
            catalog_fs_storage_options: BTreeMap::new(),
            catalog_fs_rust_storage_options: BTreeMap::new(),
            data_type: "TradeTick".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        },
        artifact_root: "s3://bolt-parquet/nt-research-analytics".to_string(),
        output_prefix:
            "s3://bolt-parquet/nt-research-analytics/backtests/backtesting-vertical-slice-end-to-end"
                .to_string(),
        artifact_store: backtesting_vertical_slice::run_manifest::ManifestArtifactStore {
            storage_options: BTreeMap::new(),
            rust_storage_options: BTreeMap::new(),
            ssm_parameters: None,
        },
        start_time: None,
        end_time: None,
    }
}

#[test]
fn accepted_data_flows_through_to_objective_result_contract() {
    let accepted = accepted_dataset();
    let identity = CanonicalInstrumentIdentity {
        instrument_id: "BNBUSDC".to_string(),
        venue_symbol: "BNBUSDC".to_string(),
        nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
    };
    let temp = tempfile::TempDir::new().expect("temp dir");
    let canonical_path = temp.path().join("canonical-trades.parquet");
    let catalog_root = temp.path().join("nt-catalog");
    let catalog_path = catalog_root.to_str().unwrap().to_string();
    let manifest = manifest(&catalog_path);
    let contract_manifest_hash = manifest.manifest_hash();

    let output = run_backtest(BacktestRunInputs {
        accepted: &accepted,
        identity: &identity,
        instrument_spec: &instrument_spec(),
        csv_text: SAMPLE_CSV,
        capture_time_nanos: 1_772_512_022_000_000_000,
        manifest: &manifest,
        contract_manifest_hash: &contract_manifest_hash,
        converter: &converter_config(),
        canonical_artifact_path: &canonical_path,
        catalog_root: &catalog_root,
        created_at: "2026-06-02T00:00:00Z",
        artifact_uris: ResultArtifactUris {
            source_proof_uri: "s3://bolt-parquet/nt-research-analytics/source-proofs/p.json"
                .to_string(),
            canonical_table_uri: canonical_path.to_string_lossy().to_string(),
            nt_catalog_uri: catalog_path.clone(),
            catalog_metadata_uri:
                "s3://bolt-parquet/nt-research-analytics/backtests/end-to-end/catalog-metadata.json"
                    .to_string(),
            result_contract_uri:
                "s3://bolt-parquet/nt-research-analytics/backtests/end-to-end/result.json"
                    .to_string(),
        },
    })
    .expect("end-to-end backtest run");

    // Gate 2: canonical normalization.
    assert_eq!(output.canonical_table.rows.len(), 3);
    assert!(
        canonical_path.exists(),
        "canonical artifact must be written"
    );

    // Gate 3: catalog projection + read-back proof.
    assert_eq!(output.projection.trade_count, 3);
    assert_eq!(output.read_back_count, 3);
    assert!(!output.projection.catalog_hash.is_empty());

    // Gate 5: NautilusTrader BacktestNode produced a result over the trades.
    assert_eq!(
        output.nt_result.run_config_id.as_deref(),
        Some("backtesting-vertical-slice-end-to-end")
    );
    // Trade-only TRADE_REPLAY data carries no quotes, so NautilusTrader's
    // order/fill counters stay at zero; `total_events` counts execution events,
    // not trade ticks fed, so it is 0 here even though the engine consumed the
    // accepted trades. The proof that the engine ran over the accepted data is
    // the gate-3 read-back (3 ticks) bound to the same instrument id the engine
    // queries — not these counters.
    assert_eq!(
        output.nt_result.total_orders, 0,
        "trade-only TRADE_REPLAY data is quote-free, so the quote-driven strategy places no orders"
    );
    assert_eq!(output.nt_result.total_positions, 0);
    // The engine itself — not just the read-back loader — consumed every accepted
    // trade: NautilusTrader increments `iterations` once per data point delivered.
    assert_eq!(
        output.nt_result.iterations, 3,
        "engine must iterate exactly once per accepted trade"
    );

    // Gate 6: objective result contract.
    let contract = &output.contract;
    contract
        .validate()
        .expect("contract is objective and complete");
    assert_eq!(contract.source_proof_id, SOURCE_PROOF_ID);
    assert_eq!(contract.source_proof_version, 1);
    assert_eq!(contract.nt_version.len(), 40);
    assert_eq!(
        contract.fidelity_class,
        SourceProofFidelityClass::TradeReplay
    );
    // The zero-orders warning is emitted with the honest TRADE_REPLAY rationale,
    // and the claim limits carry the full reference set forward.
    assert_eq!(contract.warnings.len(), 1);
    assert!(
        contract.warnings[0].contains("TRADE_REPLAY"),
        "warning must explain the trade-only fidelity: {:?}",
        contract.warnings[0]
    );
    assert_eq!(contract.claim_limits.len(), 3);
    assert_eq!(contract.catalog_hash, output.projection.catalog_hash);
}

#[test]
fn partial_time_window_gate_admits_only_in_window_trades() {
    // The manifest's optional `[start_time, end_time]` window maps into
    // NautilusTrader's `BacktestRunConfig` start/end. NautilusTrader only delivers
    // (and counts in `iterations`) trades whose `ts_init` falls inside that window,
    // so the iterations gate must compare the engine's count to the *in-window*
    // accepted trades — not the full projected set. A window that legitimately
    // trims the data must still run, not spuriously fail the gate.
    let accepted = accepted_dataset();
    let identity = CanonicalInstrumentIdentity {
        instrument_id: "BNBUSDC".to_string(),
        venue_symbol: "BNBUSDC".to_string(),
        nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
    };

    // A full-window run first, only to learn the real normalized event times of the
    // accepted trades (so the windowed run below carries no hardcoded nanosecond
    // literal and stays faithful to the canonical normalizer).
    let full_temp = tempfile::TempDir::new().expect("temp dir");
    let full_canonical = full_temp.path().join("canonical-trades.parquet");
    let full_catalog = full_temp.path().join("nt-catalog");
    let full_catalog_path = full_catalog.to_str().unwrap().to_string();
    let full_manifest = manifest(&full_catalog_path);
    let full_contract_manifest_hash = full_manifest.manifest_hash();
    let full = run_backtest(BacktestRunInputs {
        accepted: &accepted,
        identity: &identity,
        instrument_spec: &instrument_spec(),
        csv_text: SAMPLE_CSV,
        capture_time_nanos: 1_772_512_022_000_000_000,
        manifest: &full_manifest,
        contract_manifest_hash: &full_contract_manifest_hash,
        converter: &converter_config(),
        canonical_artifact_path: &full_canonical,
        catalog_root: &full_catalog,
        created_at: "2026-06-02T00:00:00Z",
        artifact_uris: ResultArtifactUris {
            source_proof_uri: "s3://bolt-parquet/nt-research-analytics/source-proofs/p.json"
                .to_string(),
            canonical_table_uri: full_canonical.to_string_lossy().to_string(),
            nt_catalog_uri: full_catalog_path.clone(),
            catalog_metadata_uri:
                "s3://bolt-parquet/nt-research-analytics/backtests/win/catalog-metadata.json"
                    .to_string(),
            result_contract_uri: "s3://bolt-parquet/nt-research-analytics/backtests/win/r.json"
                .to_string(),
        },
    })
    .expect("full-window run");
    assert_eq!(full.canonical_table.rows.len(), 3);
    let first_event = full.canonical_table.rows[0].event_time;
    let last_event = full.canonical_table.rows[2].event_time;
    assert!(
        first_event < last_event,
        "fixture must span more than one event timestamp"
    );

    // Second run: an `end_time` at the first trade's event time admits exactly one
    // trade (NautilusTrader's end bound is inclusive). The gate must accept the run
    // with `iterations == 1`; under a full-dataset gate it would spuriously bail.
    let temp = tempfile::TempDir::new().expect("temp dir");
    let canonical_path = temp.path().join("canonical-trades.parquet");
    let catalog_root = temp.path().join("nt-catalog");
    let catalog_path = catalog_root.to_str().unwrap().to_string();
    let mut windowed_manifest = manifest(&catalog_path);
    windowed_manifest.end_time = Some(first_event);
    let windowed_contract_manifest_hash = windowed_manifest.manifest_hash();

    let windowed = run_backtest(BacktestRunInputs {
        accepted: &accepted,
        identity: &identity,
        instrument_spec: &instrument_spec(),
        csv_text: SAMPLE_CSV,
        capture_time_nanos: 1_772_512_022_000_000_000,
        manifest: &windowed_manifest,
        contract_manifest_hash: &windowed_contract_manifest_hash,
        converter: &converter_config(),
        canonical_artifact_path: &canonical_path,
        catalog_root: &catalog_root,
        created_at: "2026-06-02T00:00:00Z",
        artifact_uris: ResultArtifactUris {
            source_proof_uri: "s3://bolt-parquet/nt-research-analytics/source-proofs/p.json"
                .to_string(),
            canonical_table_uri: canonical_path.to_string_lossy().to_string(),
            nt_catalog_uri: catalog_path.clone(),
            catalog_metadata_uri:
                "s3://bolt-parquet/nt-research-analytics/backtests/win/catalog-metadata-2.json"
                    .to_string(),
            result_contract_uri: "s3://bolt-parquet/nt-research-analytics/backtests/win/r2.json"
                .to_string(),
        },
    })
    .expect("partial-window run must pass the iterations gate");

    // The catalog still projects all three accepted trades; only the engine run is
    // windowed, so the read-back proof is unchanged while the engine iterates once.
    assert_eq!(windowed.read_back_count, 3);
    assert_eq!(
        windowed.nt_result.iterations, 1,
        "engine must iterate once per in-window trade (end bound inclusive)"
    );
}
