//! End-to-end proof for the operator-binding slice over the dual-emit Parquet
//! event-stream adapter: one accepted parquet object emits BOTH an
//! order-book-delta table and a trades table, projected into two per-table
//! catalog subroots, bound by a two-input manifest, executed by ONE
//! `BacktestNode` run, and indexed in `conversion-tables.json`. Also proves the
//! selector-provenance gate: an L2 run-spec without provenance fails loud.
//!
//! Fixtures are synthetic and venue-free, built through the public source-proof
//! gate with a synthetic source-binding registry (the committed registry rejects
//! synthetic bindings), mirroring `backtesting_vertical_slice_parquet_event_adapter`.

use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

use arrow::{
    array::{ArrayRef, StringArray},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use backtesting_vertical_slice::{
    canonical_order_book_deltas::{
        DeltaMappingConfig, DeltaPriceSignPolicy, DeltaSourceFormat, EmptyBookPolicy,
        EventStreamMappingFields, InstrumentKeySpec, OrderingAuthority,
    },
    canonical_trades::{
        CanonicalInstrumentIdentity, ConverterConfig, CsvTimestampUnit, CsvTradeMappingConfig,
        PARQUET_EVENT_STREAM_DELTAS_ADAPTER, RawPayloadConfig, RawPayloadContainer,
    },
    catalog_projection::{CatalogInstrumentSpec, SpotInstrumentSpec},
    hashing::sha256_hex,
    operator::{
        MultiTableRunArtifacts, OperatorRunArtifacts, RunSpec, RunSpecInstrumentIdentities,
        RunSpecInstrumentSpecs, RunSpecSelectorProvenance, run_operator_from_run_spec,
    },
    run_manifest::{
        BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION, BacktestingRunManifest, ManifestArtifactStore,
        ManifestCatalogInput, ManifestVenueConfig, MarketStructureFixture, RunPurpose,
        STRATEGY_HURST_VPIN_DIRECTIONAL, StrategySource, StrategySourceKind,
    },
    source_proof::{
        AcceptanceMode, AcceptanceScope, EvidenceState, FixtureType, IngestManifestObjectRecord,
        L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck, RequiredChecks,
        SourceBindingRegistry, SourceCandidateClass, SourceProofClaimLimit,
        SourceProofFidelityClass, SourceProofReport, SourceProofStatus, SourceProofUsageScope,
        SourceSelectionStatus, TimeRange,
    },
};
use parquet::arrow::ArrowWriter;

const NT_INSTRUMENT_ID: &str = "BASEQUOTE.TESTVENUE";
const INSTRUMENT_ID: &str = "BASEQUOTE";
const SOURCE_URL: &str = "https://synthetic.invalid/data";
const ACCEPTED_AT: &str = "2026-06-02T00:00:00Z";
const OPERATOR: &str = "operator";
const CANONICAL_KEY: &str = "testvenue/prediction-market/BASEQUOTE";
const SOURCE_PROOF_ID: &str = "source-proof-synthetic-deltas";
const SOURCE_BINDING: &str = "testvenue-deltas";

const REGISTRY_TOML: &str = r#"[[source_binding]]
key = "testvenue-deltas"
venue = "testvenue"
product_family = "prediction-market"
market_structure_fixture = "binary-option"
source_uri = "https://synthetic.invalid/data"
evidence_state = "owner_archive_backfillable"
table_families = ["order_book_snapshot_deltas"]
"#;

const COMMITTED_RUN_SPEC: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
);

fn registry() -> SourceBindingRegistry {
    SourceBindingRegistry::from_toml_str(REGISTRY_TOML)
        .expect("synthetic source binding registry parses")
}

fn write_registry(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("source-bindings.toml");
    fs::write(&path, REGISTRY_TOML).expect("write registry");
    path
}

fn spot_spec() -> SpotInstrumentSpec {
    SpotInstrumentSpec {
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        raw_symbol: INSTRUMENT_ID.to_string(),
        base_currency: "BTC".to_string(),
        quote_currency: "USDC".to_string(),
        price_increment: "0.01".to_string(),
        size_increment: "0.001".to_string(),
        min_quantity: "0.001".to_string(),
        max_quantity: "1000000".to_string(),
        min_notional: "1".to_string(),
        max_notional: "100000000".to_string(),
    }
}

fn identity() -> CanonicalInstrumentIdentity {
    CanonicalInstrumentIdentity {
        instrument_id: INSTRUMENT_ID.to_string(),
        venue_symbol: INSTRUMENT_ID.to_string(),
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
    }
}

fn claim_limits_for(claims: &[String]) -> Vec<SourceProofClaimLimit> {
    claims
        .iter()
        .enumerate()
        .map(|(index, claim)| SourceProofClaimLimit {
            id: format!("claim-limit-{}", index + 1),
            severity: "blocking".to_string(),
            claim: claim.clone(),
            reason: "source fidelity does not prove this claim".to_string(),
            evidence_ref: "source-proof://fidelity-class".to_string(),
        })
        .collect()
}

fn accepted_proof_and_object(
    object_bytes: &[u8],
) -> (SourceProofReport, IngestManifestObjectRecord) {
    let object = IngestManifestObjectRecord {
        s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.parquet".to_string(),
        source_url: SOURCE_URL.to_string(),
        sha256: sha256_hex(object_bytes),
        bytes: object_bytes.len() as u64,
        archive_date: "2026-05-22".to_string(),
        schema_columns: vec!["event_stream_parquet".to_string()],
    };
    let forbidden_claims = vec!["No execution-quality claims.".to_string()];
    let checks = |evidence: &str| RequiredChecks {
        source_access: RequiredCheck::passed(evidence),
        license: RequiredCheck::passed("attestation"),
        schema: RequiredCheck::passed("schema"),
        time_semantics: RequiredCheck::passed("ms_to_nanos"),
        instrument_universe: RequiredCheck::passed("universe"),
        coverage: RequiredCheck::passed(evidence),
        retention_freshness: RequiredCheck::passed("retention"),
        granularity: RequiredCheck::passed("event_stream"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("OrderBookDelta"),
        cost: RequiredCheck::passed("free"),
        storage: RequiredCheck::passed("artifact_root"),
    };
    let proof = SourceProofReport {
        source_proof_id: SOURCE_PROOF_ID.to_string(),
        source_proof_version: 1,
        contract_version: "backfill-table-contract.v1".to_string(),
        schema_version: "backfill-source-proof.v1".to_string(),
        status: SourceProofStatus::Pending,
        source_binding: SOURCE_BINDING.to_string(),
        venue: "testvenue".to_string(),
        product_family: "prediction-market".to_string(),
        product_category: "binary".to_string(),
        table_family: "order_book_snapshot_deltas".to_string(),
        evidence_state: EvidenceState::OwnerArchiveBackfillable,
        source_candidate_class: SourceCandidateClass::OfficialFree,
        source_selection_status: SourceSelectionStatus::AcceptedLowerFidelity,
        usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        official_free_gap_ref: None,
        paid_vendor_gap_ref: None,
        fixture_type: FixtureType::BinaryOption,
        requested_time_range: TimeRange {
            start_utc: "2025-06-01T00:00:00Z".to_string(),
            end_utc: "2026-06-01T00:00:00Z".to_string(),
        },
        coverage_time_range: TimeRange {
            start_utc: "2026-05-22T00:00:00Z".to_string(),
            end_utc: "2026-05-23T00:00:00Z".to_string(),
        },
        instrument_universe_id: "testvenue-deltas-instruments-2026-05-22".to_string(),
        raw_sample_uri: object.s3_uri.clone(),
        raw_sample_hash: object.sha256.clone(),
        schema_sample_uri: "s3://synthetic-artifacts/source-proofs/schema.json".to_string(),
        schema_sample_hash: "bf26db".to_string(),
        license_ref: "https://synthetic.invalid/ (attestation)".to_string(),
        license_scope: LicenseScope::Public,
        retention_ref: "https://synthetic.invalid/".to_string(),
        cost_ref: "cost://free-public-archive".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::L2Replay,
        l2_replay_evidence: L2ReplayEvidence {
            order_book_delta_ref: Some("source-proof://order-book-deltas".to_string()),
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: Some(
                "source-proof://no-tick-size-change-universe".to_string(),
            ),
            timed_instrument_epoch_replay_ref: None,
        },
        forbidden_claims: forbidden_claims.clone(),
        claim_limits: claim_limits_for(&forbidden_claims),
        cross_market_components: Vec::new(),
        acceptance_scope: Some(AcceptanceScope {
            planned_objects: 1,
            completed_objects: 1,
            failed_objects: 0,
            skipped_objects: 0,
            accepted_bytes: object.bytes,
            selector_scope_violations: 0,
        }),
        gap_policy_id: String::new(),
        required_checks: checks("manifest://synthetic"),
        acceptance_mode: None,
        accepted_by: None,
        accepted_at: None,
        supersedes_source_proof_id: None,
    }
    .accept_with_registry(&registry(), AcceptanceMode::Manual, OPERATOR, ACCEPTED_AT)
    .expect("accept source proof");
    (proof, object)
}

fn venue_config() -> ManifestVenueConfig {
    ManifestVenueConfig {
        nt_venue: "TESTVENUE".to_string(),
        oms_type: "NETTING".to_string(),
        account_type: "CASH".to_string(),
        book_type: "L2_MBP".to_string(),
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
        leverages: None,
        margin_model: None,
        modules: None,
        fill_model: None,
        latency_model: None,
        fee_model: None,
        settlement_prices: None,
    }
}

fn catalog_input(data_type: &str) -> ManifestCatalogInput {
    ManifestCatalogInput {
        catalog_path: "overridden-by-operator-at-binding".to_string(),
        catalog_fs_protocol: "NONE".to_string(),
        catalog_fs_storage_options: BTreeMap::new(),
        catalog_fs_rust_storage_options: BTreeMap::new(),
        data_type: data_type.to_string(),
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        instrument_ids: None,
        start_time: None,
        end_time: None,
        filter_expr: None,
        client_id: None,
        metadata: None,
        bar_spec: None,
        bar_types: None,
        optimize_file_loading: None,
    }
}

fn manifest(run_id: &str, catalog_inputs: Vec<ManifestCatalogInput>) -> BacktestingRunManifest {
    BacktestingRunManifest {
        manifest_schema_version: BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION.to_string(),
        run_id: run_id.to_string(),
        target_bolt_v2_branch: "main".to_string(),
        target_bolt_v2_ref: "refs/heads/main".to_string(),
        resolved_nt_version: backtesting_vertical_slice::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
            .expect("BVS NautilusTrader dependency provenance"),
        market_structure_fixture: MarketStructureFixture::BinaryOption,
        venue_binding_key: SOURCE_BINDING.to_string(),
        run_purpose: RunPurpose::Normal,
        source_proof_id: SOURCE_PROOF_ID.to_string(),
        source_proof_version: 1,
        pins_non_latest_proof: false,
        proof_pin_reason_code: None,
        proof_pin_reason_detail: None,
        strategy: StrategySource {
            source_kind: StrategySourceKind::CompiledRustRegistry,
            registry_key: STRATEGY_HURST_VPIN_DIRECTIONAL.to_string(),
            parameters: BTreeMap::from([
                ("trade_size".to_string(), "0.01".to_string()),
                (
                    "bar_type".to_string(),
                    format!("{NT_INSTRUMENT_ID}-1-MINUTE-LAST-INTERNAL"),
                ),
            ]),
            typed_config_uri: None,
            typed_config_hash: None,
            experiment_result_uri: None,
            experiment_result_hash: None,
            config_overlay: None,
        },
        strategy_config_hash: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        venue: venue_config(),
        additional_venues: Vec::new(),
        catalog_inputs,
        reconstructed_reference_current_price: Vec::new(),
        instrument_settlements: Vec::new(),
        catalog_hash: "1111111111111111111111111111111111111111111111111111111111111111"
            .to_string(),
        execution_model: "nt_backtest_node".to_string(),
        artifact_root: "s3://synthetic-artifacts/nt-research-analytics".to_string(),
        output_prefix: format!("s3://synthetic-artifacts/nt-research-analytics/backtests/{run_id}"),
        artifact_store: ManifestArtifactStore {
            storage_options: BTreeMap::new(),
            rust_storage_options: BTreeMap::new(),
            ssm_parameters: None,
        },
        domain_metrics: Vec::new(),
        start_time: None,
        end_time: None,
    }
}

fn csv_filler_mapping() -> CsvTradeMappingConfig {
    CsvTradeMappingConfig {
        has_headers: true,
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

fn parquet_converter(object_len: u64) -> ConverterConfig {
    ConverterConfig {
        identity: PARQUET_EVENT_STREAM_DELTAS_ADAPTER.identity.to_string(),
        version: PARQUET_EVENT_STREAM_DELTAS_ADAPTER.version.to_string(),
        raw_payload: RawPayloadConfig {
            container: RawPayloadContainer::ParquetFile,
            max_object_bytes: object_len.max(1),
            max_decoded_bytes: 1_048_576,
            zip_member: None,
            max_member_bytes: None,
            member_suffix: None,
        },
        csv: csv_filler_mapping(),
        bars: None,
        paged_json_bars: None,
        jsonl_bars: None,
        deltas: Some(event_stream_mapping()),
        quotes: None,
        seeded_l2_quotes: None,
    }
}

fn event_stream_mapping() -> DeltaMappingConfig {
    DeltaMappingConfig {
        format: DeltaSourceFormat::EventStream(Box::new(EventStreamMappingFields {
            event_type_field: "event_type".to_string(),
            snapshot_event_value: "book".to_string(),
            level_change_event_value: "price_change".to_string(),
            trade_event_value: "last_trade".to_string(),
            dropped_event_values: vec!["tick_size_change".to_string()],
            side_field: "side".to_string(),
            buy_side_values: vec!["BUY".to_string()],
            sell_side_values: vec!["SELL".to_string()],
            price_field: "price".to_string(),
            size_field: "size".to_string(),
            bids_field: "bids".to_string(),
            asks_field: "asks".to_string(),
            capture_time_field: "capture_time".to_string(),
            capture_time_unit: CsvTimestampUnit::Milliseconds,
            tiebreak_is_row_index: true,
            trade_price_field: "trade_price".to_string(),
            trade_size_field: "trade_size".to_string(),
            trade_id_field: None,
            event_time_field: None,
            event_time_unit: None,
            trade_forbidden_claims: vec!["No execution-quality claims.".to_string()],
        })),
        instrument_key: InstrumentKeySpec {
            key_field: None,
            exclusion_filter: None,
        },
        ordering: OrderingAuthority::CaptureTime,
        price_sign_policy: DeltaPriceSignPolicy::StrictlyPositive,
        empty_book_policy: EmptyBookPolicy::LoneClearLast,
    }
}

struct EventRow {
    event_type: &'static str,
    capture_time: Option<&'static str>,
    bids: Option<&'static str>,
    asks: Option<&'static str>,
    price: Option<&'static str>,
    size: Option<&'static str>,
    side: Option<&'static str>,
    trade_price: Option<&'static str>,
    trade_size: Option<&'static str>,
}

fn build_event_parquet(rows: &[EventRow]) -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("event_type", DataType::Utf8, true),
        Field::new("capture_time", DataType::Utf8, true),
        Field::new("bids", DataType::Utf8, true),
        Field::new("asks", DataType::Utf8, true),
        Field::new("price", DataType::Utf8, true),
        Field::new("size", DataType::Utf8, true),
        Field::new("side", DataType::Utf8, true),
        Field::new("trade_price", DataType::Utf8, true),
        Field::new("trade_size", DataType::Utf8, true),
    ]));
    let column = |pick: fn(&EventRow) -> Option<&'static str>| -> ArrayRef {
        Arc::new(StringArray::from(rows.iter().map(pick).collect::<Vec<_>>()))
    };
    let event_type_col: ArrayRef = Arc::new(StringArray::from(
        rows.iter()
            .map(|row| Some(row.event_type))
            .collect::<Vec<_>>(),
    ));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            event_type_col,
            column(|row| row.capture_time),
            column(|row| row.bids),
            column(|row| row.asks),
            column(|row| row.price),
            column(|row| row.size),
            column(|row| row.side),
            column(|row| row.trade_price),
            column(|row| row.trade_size),
        ],
    )
    .expect("synthetic event record batch");
    let mut buffer = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buffer, schema, None).expect("parquet writer");
    writer.write(&batch).expect("write parquet batch");
    writer.close().expect("finalize parquet");
    buffer
}

fn event_fixture_parquet() -> Vec<u8> {
    build_event_parquet(&[
        EventRow {
            event_type: "book",
            capture_time: Some("1700000000000"),
            bids: Some("[[\"0.49\",\"10\"]]"),
            asks: Some("[[\"0.51\",\"12\"]]"),
            price: None,
            size: None,
            side: None,
            trade_price: None,
            trade_size: None,
        },
        EventRow {
            event_type: "price_change",
            capture_time: Some("1700000001000"),
            bids: None,
            asks: None,
            price: Some("0.50"),
            size: Some("11"),
            side: Some("BUY"),
            trade_price: None,
            trade_size: None,
        },
        EventRow {
            event_type: "last_trade",
            capture_time: Some("1700000002000"),
            bids: None,
            asks: None,
            price: None,
            size: None,
            side: Some("BUY"),
            trade_price: Some("0.50"),
            trade_size: Some("3"),
        },
        EventRow {
            event_type: "last_trade",
            capture_time: Some("1700000003000"),
            bids: None,
            asks: None,
            price: None,
            size: None,
            side: Some("SELL"),
            trade_price: Some("0.49"),
            trade_size: Some("2"),
        },
    ])
}

fn dual_emission_run_spec(
    fixture_dir: &std::path::Path,
    run_id: &str,
    selector_provenance: Option<RunSpecSelectorProvenance>,
) -> (RunSpec, Vec<u8>) {
    let object_bytes = event_fixture_parquet();
    let (proof, object) = accepted_proof_and_object(&object_bytes);
    let registry_path = write_registry(fixture_dir);
    let manifest = manifest(
        run_id,
        vec![catalog_input("OrderBookDelta"), catalog_input("TradeTick")],
    );
    let committed: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("committed run-spec parses");
    let spec = RunSpec {
        capture_time_utc: ACCEPTED_AT.to_string(),
        created_at_utc: ACCEPTED_AT.to_string(),
        accepted_by: OPERATOR.to_string(),
        accepted_at_utc: ACCEPTED_AT.to_string(),
        source_bindings_path: registry_path,
        accepted_object: object,
        source_proof: proof,
        instrument_spec: RunSpecInstrumentSpecs::Keyed(BTreeMap::from([(
            CANONICAL_KEY.to_string(),
            CatalogInstrumentSpec::Spot(spot_spec()),
        )])),
        identity: RunSpecInstrumentIdentities::Single(identity()),
        converter: parquet_converter(object_bytes.len() as u64),
        manifest,
        artifact_store: committed.artifact_store,
        catalog_dispatch: committed.catalog_dispatch,
        selector_provenance,
    };
    (spec, object_bytes)
}

fn l2_provenance() -> Option<RunSpecSelectorProvenance> {
    Some(RunSpecSelectorProvenance {
        event_count_ledger_hash: "7777777777777777777777777777777777777777777777777777777777777777"
            .to_string(),
        selected_asset_ids_hash: "8888888888888888888888888888888888888888888888888888888888888888"
            .to_string(),
    })
}

fn assert_multi(artifacts: OperatorRunArtifacts) -> MultiTableRunArtifacts {
    match artifacts {
        OperatorRunArtifacts::MultiTable(artifacts) => *artifacts,
        OperatorRunArtifacts::Trade(_) => {
            panic!("non-trade adapter kinds must dispatch through the multi-table flow")
        }
    }
}

#[test]
fn parquet_event_stream_run_spec_end_to_end_dual_emission() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (spec, object_bytes) = dual_emission_run_spec(
        temp.path(),
        "operator-binding-parquet-dual",
        l2_provenance(),
    );

    let output_dir = temp.path().join("out");
    let artifacts = assert_multi(
        run_operator_from_run_spec(&spec, &object_bytes, &output_dir).expect("operator run"),
    );

    assert_eq!(artifacts.tables.len(), 2, "dual emission -> two tables");
    let deltas = artifacts
        .tables
        .iter()
        .find(|table| table.data_type == "OrderBookDelta")
        .expect("delta table present");
    let trades = artifacts
        .tables
        .iter()
        .find(|table| table.data_type == "TradeTick")
        .expect("trade table present");
    assert_eq!(
        deltas.subroot_relative,
        "nt-catalogs/order_book_snapshot_deltas/BASEQUOTE_TESTVENUE/default"
    );
    assert_eq!(
        trades.subroot_relative,
        "nt-catalogs/trades/BASEQUOTE_TESTVENUE/default"
    );
    assert_eq!(trades.rows, 2, "two last_trade events -> two trades");
    assert!(deltas.canonical_path.is_file(), "delta canonical exists");
    assert!(trades.canonical_path.is_file(), "trade canonical exists");
    assert_eq!(artifacts.nt_result.iterations, deltas.rows + trades.rows);
    assert!(
        artifacts.conversion_tables_path.is_some(),
        "dual emission writes the tables index"
    );
    assert_eq!(
        artifacts.contract.fidelity_class,
        SourceProofFidelityClass::L2Replay,
        "primary catalog input is the delta table"
    );

    // Second run reuses the completed output and re-verifies the tables index.
    let contract_bytes = fs::read(&artifacts.contract_path).expect("read contract");
    let rerun = assert_multi(
        run_operator_from_run_spec(&spec, &object_bytes, &output_dir).expect("idempotent rerun"),
    );
    assert!(rerun.conversion_tables_path.is_some());
    assert_eq!(
        fs::read(&rerun.contract_path).expect("read rerun contract"),
        contract_bytes,
        "result contract must stay byte-identical across reruns"
    );
}

#[test]
fn parquet_event_stream_without_selector_provenance_fails_loud() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (spec, object_bytes) = dual_emission_run_spec(
        temp.path(),
        "operator-binding-parquet-dual-no-provenance",
        None,
    );

    let err = run_operator_from_run_spec(&spec, &object_bytes, &temp.path().join("out"))
        .err()
        .expect("L2 run-spec without selector provenance must fail loud");
    assert!(err.to_string().contains("selector_provenance"), "{err}");
}
