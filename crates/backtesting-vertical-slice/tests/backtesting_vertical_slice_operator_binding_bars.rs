//! End-to-end proof for the operator-binding slice over the bar families:
//! `run_operator_from_run_spec` dispatches the paged-JSON and JSONL
//! multi-interval bar adapters through the multi-table flow (one object -> N
//! per-table catalog subroots -> one bound N-input manifest -> ONE
//! `BacktestNode` run), writes the conversion tables index only when more than
//! one table was produced, and reuses completed output byte-identically.
//!
//! Fixtures are synthetic and venue-free, built through the public source-proof
//! gate with a synthetic source-binding registry (the committed registry rejects
//! synthetic bindings), mirroring `backtesting_vertical_slice_bar_format_families`.

use std::{collections::BTreeMap, fs, path::PathBuf};

use backtesting_vertical_slice::{
    canonical_bars::{
        BarIntervalToken, BarPriceSignPolicy, DeclaredBarInterval, JsonlBarMappingConfig,
        PagedJsonBarMappingConfig, PagedJsonRowShape,
    },
    canonical_trades::{
        CanonicalInstrumentIdentity, ConverterConfig, CsvTimestampUnit, CsvTradeMappingConfig,
        JSONL_MULTI_INTERVAL_BARS_ADAPTER, PAGED_JSON_BARS_ADAPTER, RawPayloadConfig,
        RawPayloadContainer, SourceAdapterDefinition,
    },
    catalog_projection::{CatalogInstrumentSpec, SpotInstrumentSpec},
    conversion_boundary::{
        CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_MANIFEST_FILE,
        CONVERSION_TABLES_FILE, ConversionTableRecord,
    },
    hashing::sha256_hex,
    operator::{
        MultiTableRunArtifacts, OperatorRunArtifacts, RESULT_CONTRACT_FILE, RunSpec,
        RunSpecInstrumentIdentities, RunSpecInstrumentSpecs, VerifiedSourceBindingRegistry,
        run_operator_from_run_spec, validate_durable_run_spec_preflight,
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
use nautilus_model::enums::BarAggregation;

const NT_INSTRUMENT_ID: &str = "BASEQUOTE.TESTVENUE";
const INSTRUMENT_ID: &str = "BASEQUOTE";
const SOURCE_URL: &str = "https://synthetic.invalid/data";
const ACCEPTED_AT: &str = "2026-06-02T00:00:00Z";
const OPERATOR: &str = "operator";
const CANONICAL_KEY: &str = "testvenue/prediction-market/BASEQUOTE";
const SOURCE_PROOF_ID: &str = "source-proof-synthetic-bars";
const SOURCE_BINDING: &str = "testvenue-bars";

const REGISTRY_TOML: &str = r#"[[source_binding]]
key = "testvenue-bars"
venue = "testvenue"
product_family = "prediction-market"
market_structure_fixture = "binary-option"
source_uri = "https://synthetic.invalid/data"
evidence_state = "owner_archive_backfillable"
table_families = ["bars"]
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
    schema_columns: &[&str],
) -> (SourceProofReport, IngestManifestObjectRecord) {
    let object = IngestManifestObjectRecord {
        s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.json".to_string(),
        source_url: SOURCE_URL.to_string(),
        sha256: sha256_hex(object_bytes),
        bytes: object_bytes.len() as u64,
        archive_date: "2026-05-22".to_string(),
        schema_columns: schema_columns.iter().map(ToString::to_string).collect(),
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
        granularity: RequiredCheck::passed("aggregated_bars"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("Bar"),
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
        table_family: "bars".to_string(),
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
        instrument_universe_id: "testvenue-bars-instruments-2026-05-22".to_string(),
        raw_sample_uri: object.s3_uri.clone(),
        raw_sample_hash: object.sha256.clone(),
        schema_sample_uri: "s3://synthetic-artifacts/source-proofs/schema.json".to_string(),
        schema_sample_hash: "bf26db".to_string(),
        license_ref: "https://synthetic.invalid/ (attestation)".to_string(),
        license_scope: LicenseScope::Public,
        retention_ref: "https://synthetic.invalid/".to_string(),
        cost_ref: "cost://free-public-archive".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::TradeBarReplay,
        l2_replay_evidence: L2ReplayEvidence {
            order_book_delta_ref: None,
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: None,
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
        leverages: None,
        margin_model: None,
        modules: None,
        fill_model: None,
        latency_model: None,
        fee_model: None,
        settlement_prices: None,
    }
}

fn bar_catalog_input(bar_spec: Option<&str>) -> ManifestCatalogInput {
    ManifestCatalogInput {
        catalog_path: "overridden-by-operator-at-binding".to_string(),
        catalog_fs_protocol: "NONE".to_string(),
        catalog_fs_storage_options: BTreeMap::new(),
        catalog_fs_rust_storage_options: BTreeMap::new(),
        data_type: "Bar".to_string(),
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        instrument_ids: None,
        start_time: None,
        end_time: None,
        filter_expr: None,
        client_id: None,
        metadata: None,
        bar_spec: bar_spec.map(ToString::to_string),
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
        nt_streaming_chunk_size: 128,
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

fn converter(adapter: &SourceAdapterDefinition, object_len: u64) -> ConverterConfig {
    ConverterConfig {
        identity: adapter.identity.to_string(),
        version: adapter.version.to_string(),
        raw_payload: RawPayloadConfig {
            container: RawPayloadContainer::JsonlText,
            max_object_bytes: object_len.max(1),
            max_decoded_bytes: object_len.max(1),
            zip_member: None,
            max_member_bytes: None,
            member_suffix: None,
        },
        csv: csv_filler_mapping(),
        bars: None,
        paged_json_bars: None,
        jsonl_bars: None,
        deltas: None,
        quotes: None,
        seeded_l2_quotes: None,
    }
}

fn jsonl_two_interval_mapping() -> JsonlBarMappingConfig {
    JsonlBarMappingConfig {
        instrument_field: None,
        interval_field: "interval".to_string(),
        timestamp_unit: CsvTimestampUnit::Milliseconds,
        open_time_field: "t".to_string(),
        close_time_field: Some("ct".to_string()),
        open_field: "o".to_string(),
        high_field: "h".to_string(),
        low_field: "l".to_string(),
        close_field: "c".to_string(),
        volume_field: "v".to_string(),
        interval_token_map: BTreeMap::from([
            (
                "1m".to_string(),
                BarIntervalToken {
                    step: 1,
                    aggregation: BarAggregation::Minute,
                },
            ),
            (
                "1h".to_string(),
                BarIntervalToken {
                    step: 1,
                    aggregation: BarAggregation::Hour,
                },
            ),
        ]),
        price_sign_policy: BarPriceSignPolicy::StrictlyPositive,
    }
}

fn run_spec(
    registry_path: PathBuf,
    proof: SourceProofReport,
    object: IngestManifestObjectRecord,
    instrument_spec: RunSpecInstrumentSpecs,
    converter: ConverterConfig,
    manifest: BacktestingRunManifest,
) -> RunSpec {
    let committed: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("committed run-spec parses");
    RunSpec {
        capture_time_utc: ACCEPTED_AT.to_string(),
        created_at_utc: ACCEPTED_AT.to_string(),
        accepted_by: OPERATOR.to_string(),
        accepted_at_utc: ACCEPTED_AT.to_string(),
        source_bindings_path: registry_path,
        accepted_object: object,
        source_proof: proof,
        instrument_spec,
        identity: RunSpecInstrumentIdentities::Single(identity()),
        converter,
        manifest,
        artifact_store: None,
        catalog_dispatch: committed.catalog_dispatch,
        selector_provenance: None,
    }
}

fn assert_multi(artifacts: OperatorRunArtifacts) -> MultiTableRunArtifacts {
    match artifacts {
        OperatorRunArtifacts::MultiTable(artifacts) => *artifacts,
        OperatorRunArtifacts::Trade(_) => {
            panic!("non-trade adapter kinds must dispatch through the multi-table flow")
        }
    }
}

fn read_artifact_bytes(dir: &std::path::Path, name: &str) -> Vec<u8> {
    fs::read(dir.join(name)).unwrap_or_else(|error| panic!("read artifact {name}: {error}"))
}

/// A fresh run must reject an occupied output directory without changing its
/// committed artifacts.
fn assert_fresh_rerun_rejected(spec: &RunSpec, object_bytes: &[u8], dir: &std::path::Path) {
    let mut before = BTreeMap::new();
    for name in [
        CONVERSION_MANIFEST_FILE,
        CONVERSION_CHECKPOINT_FILE,
        CATALOG_METADATA_FILE,
        RESULT_CONTRACT_FILE,
    ] {
        before.insert(name, read_artifact_bytes(dir, name));
    }
    let tables_before = dir
        .join(CONVERSION_TABLES_FILE)
        .exists()
        .then(|| read_artifact_bytes(dir, CONVERSION_TABLES_FILE));

    run_operator_from_run_spec(spec, object_bytes, dir)
        .expect_err("fresh rerun must reject occupied output");
    for (name, bytes) in before {
        assert_eq!(
            read_artifact_bytes(dir, name),
            bytes,
            "occupied artifact {name} must stay byte-identical after rejection"
        );
    }
    if let Some(tables_bytes) = tables_before {
        assert_eq!(
            read_artifact_bytes(dir, CONVERSION_TABLES_FILE),
            tables_bytes,
            "conversion tables index must stay byte-identical after rejection"
        );
    }
}

const TWO_INTERVAL_JSONL: &str = concat!(
    r#"{"interval":"1m","t":"1700000000000","ct":"1700000060000","o":"0.50","h":"0.55","l":"0.49","c":"0.52","v":"100"}"#,
    "\n",
    r#"{"interval":"1h","t":"1700000000000","ct":"1700003600000","o":"0.50","h":"0.60","l":"0.48","c":"0.59","v":"500"}"#,
    "\n",
    r#"{"interval":"1m","t":"1700000060000","ct":"1700000120000","o":"0.52","h":"0.58","l":"0.51","c":"0.57","v":"120"}"#,
    "\n",
    r#"{"interval":"1h","t":"1700003600000","ct":"1700007200000","o":"0.59","h":"0.65","l":"0.55","c":"0.62","v":"450"}"#,
);

fn two_interval_run_spec(fixture_dir: &std::path::Path, run_id: &str) -> (RunSpec, &'static [u8]) {
    let object_bytes = TWO_INTERVAL_JSONL.as_bytes();
    let (proof, object) = accepted_proof_and_object(
        object_bytes,
        &["interval", "t", "ct", "o", "h", "l", "c", "v"],
    );
    let registry_path = write_registry(fixture_dir);
    let mut converter = converter(
        &JSONL_MULTI_INTERVAL_BARS_ADAPTER,
        object_bytes.len() as u64,
    );
    converter.jsonl_bars = Some(jsonl_two_interval_mapping());
    let manifest = manifest(
        run_id,
        vec![
            bar_catalog_input(Some("1minute")),
            bar_catalog_input(Some("1hour")),
        ],
    );
    let spec = run_spec(
        registry_path,
        proof,
        object,
        RunSpecInstrumentSpecs::Keyed(BTreeMap::from([(
            CANONICAL_KEY.to_string(),
            CatalogInstrumentSpec::Spot(spot_spec()),
        )])),
        converter,
        manifest,
    );
    (spec, object_bytes)
}

#[test]
fn paged_json_bars_run_spec_end_to_end_single_table() {
    let json = concat!(
        r#"{"result":{"list":[["1700000060000","0.52","0.58","0.51","0.57","120"],["1700000000000","0.50","0.55","0.49","0.52","100"]]}}"#,
        "\n",
        r#"{"result":{"list":[["1700000120000","0.57","0.60","0.56","0.59","90"],["1700000060000","0.52","0.58","0.51","0.57","120"]]}}"#,
    );
    let object_bytes = json.as_bytes();
    let (proof, object) = accepted_proof_and_object(
        object_bytes,
        &["start", "open", "high", "low", "close", "volume"],
    );
    let temp = tempfile::TempDir::new().expect("temp dir");
    let registry_path = write_registry(temp.path());
    let mut converter = converter(&PAGED_JSON_BARS_ADAPTER, object_bytes.len() as u64);
    converter.paged_json_bars = Some(PagedJsonBarMappingConfig {
        rows_path: "result.list".to_string(),
        row_shape: PagedJsonRowShape::PositionalArray {
            open_time_index: 0,
            open_index: 1,
            high_index: 2,
            low_index: 3,
            close_index: 4,
            volume_index: 5,
            close_time_index: None,
        },
        timestamp_unit: CsvTimestampUnit::Milliseconds,
        interval: DeclaredBarInterval {
            step: 1,
            aggregation: BarAggregation::Minute,
        },
        price_sign_policy: BarPriceSignPolicy::StrictlyPositive,
    });
    let manifest = manifest(
        "operator-binding-paged-json-bars",
        vec![bar_catalog_input(None)],
    );
    let spec = run_spec(
        registry_path,
        proof,
        object,
        RunSpecInstrumentSpecs::Single(Box::new(CatalogInstrumentSpec::Spot(spot_spec()))),
        converter,
        manifest,
    );

    let output_dir = temp.path().join("out");
    let artifacts = assert_multi(
        run_operator_from_run_spec(&spec, object_bytes, &output_dir).expect("operator run"),
    );

    assert_eq!(artifacts.tables.len(), 1, "paged REST is per-instrument");
    let table = &artifacts.tables[0];
    assert_eq!(table.table_family, "bars");
    assert_eq!(table.data_type, "Bar");
    assert_eq!(table.bar_spec.as_deref(), Some("1minute"));
    assert_eq!(table.rows, 3, "boundary minute collapses across pages");
    assert_eq!(
        table.subroot_relative,
        "nt-catalogs/bars/BASEQUOTE_TESTVENUE/1minute"
    );
    assert!(table.subroot.is_dir(), "projected subroot exists");
    assert!(table.canonical_path.is_file(), "canonical parquet exists");
    assert_eq!(artifacts.nt_result.iterations, 3);
    assert!(
        artifacts.conversion_tables_path.is_none(),
        "single-table conversions never write the tables index"
    );
    assert!(
        !output_dir.join(CONVERSION_TABLES_FILE).exists(),
        "single-table conversions never write {CONVERSION_TABLES_FILE}"
    );
    assert!(artifacts.contract_path.is_file());
    assert_eq!(
        artifacts.contract.fidelity_class,
        SourceProofFidelityClass::TradeBarReplay
    );

    assert_fresh_rerun_rejected(&spec, object_bytes, &output_dir);
}

#[test]
fn jsonl_multi_interval_bars_run_spec_end_to_end_two_tables() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (spec, object_bytes) = two_interval_run_spec(temp.path(), "operator-binding-jsonl-bars");

    let output_dir = temp.path().join("out");
    let artifacts = assert_multi(
        run_operator_from_run_spec(&spec, object_bytes, &output_dir).expect("operator run"),
    );

    assert_eq!(artifacts.tables.len(), 2, "two intervals -> two tables");
    let minute = artifacts
        .tables
        .iter()
        .find(|table| table.bar_spec.as_deref() == Some("1minute"))
        .expect("minute table present");
    let hour = artifacts
        .tables
        .iter()
        .find(|table| table.bar_spec.as_deref() == Some("1hour"))
        .expect("hour table present");
    assert_eq!(minute.rows, 2);
    assert_eq!(hour.rows, 2);
    assert_ne!(
        minute.subroot, hour.subroot,
        "two intervals -> two subroots"
    );
    assert_eq!(
        minute.subroot_relative,
        "nt-catalogs/bars/BASEQUOTE_TESTVENUE/1minute"
    );
    assert_eq!(
        hour.subroot_relative,
        "nt-catalogs/bars/BASEQUOTE_TESTVENUE/1hour"
    );
    assert_eq!(artifacts.nt_result.iterations, 4);

    let tables_path = artifacts
        .conversion_tables_path
        .as_ref()
        .expect("multi-table conversions write the tables index");
    let records: Vec<ConversionTableRecord> =
        serde_json::from_slice(&fs::read(tables_path).expect("read tables index"))
            .expect("tables index parses");
    assert_eq!(records.len(), 2);
    for table in &artifacts.tables {
        assert!(
            records.iter().any(|record| {
                record.subroot_uri == table.subroot_relative
                    && record.catalog_hash == table.catalog_hash
                    && record.rows == table.rows
                    && record.bar_spec == table.bar_spec
            }),
            "tables index must record {}",
            table.subroot_relative
        );
    }

    assert_fresh_rerun_rejected(&spec, object_bytes, &output_dir);
}

#[test]
fn tampered_tables_index_fails_loud_on_reuse() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (spec, object_bytes) =
        two_interval_run_spec(temp.path(), "operator-binding-tampered-index");

    let output_dir = temp.path().join("out");
    let artifacts = assert_multi(
        run_operator_from_run_spec(&spec, object_bytes, &output_dir).expect("operator run"),
    );
    let tables_path = artifacts
        .conversion_tables_path
        .expect("multi-table conversions write the tables index");
    let mut records: Vec<ConversionTableRecord> =
        serde_json::from_slice(&fs::read(&tables_path).expect("read tables index"))
            .expect("tables index parses");
    records[0].rows += 1;
    fs::write(
        &tables_path,
        serde_json::to_vec_pretty(&records).expect("serialize tampered index"),
    )
    .expect("write tampered index");

    let err = run_operator_from_run_spec(&spec, object_bytes, &output_dir)
        .err()
        .expect("tampered tables index must fail loud on reuse");
    assert!(
        err.to_string().contains("conversion tables index")
            || err.to_string().contains(CONVERSION_TABLES_FILE),
        "{err}"
    );
}

#[test]
fn stray_parquet_in_one_multi_table_subroot_fails_loud_on_reuse() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (spec, object_bytes) =
        two_interval_run_spec(temp.path(), "operator-binding-stray-multi-catalog");
    let output_dir = temp.path().join("out");
    let artifacts = assert_multi(
        run_operator_from_run_spec(&spec, object_bytes, &output_dir).expect("operator run"),
    );
    let stray = artifacts.tables[0].subroot.join("stray.parquet");
    fs::write(&stray, b"not part of the committed exact set").expect("plant stray parquet");

    let error = run_operator_from_run_spec(&spec, object_bytes, &output_dir)
        .err()
        .expect("multi-table reuse must reject a stray catalog file");

    assert!(
        error.to_string().contains("unexpected file") || error.to_string().contains("exactly one"),
        "{error:#}"
    );
    assert!(
        stray.exists(),
        "verification must not mutate stray evidence"
    );
}

#[test]
fn source_universe_durable_preflight_rejects_stale_nt_revision_without_output() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (mut spec, _) = two_interval_run_spec(temp.path(), "source-universe-durable-stale-nt");
    let output = temp.path().join("must-not-exist");
    spec.manifest.resolved_nt_version = "stale-nt-revision".to_string();
    let registry = VerifiedSourceBindingRegistry::from_run_spec(&spec)
        .expect("freeze the bars source-binding registry");

    let error = validate_durable_run_spec_preflight(&spec, &registry)
        .expect_err("stale NT revision must fail before durable source bytes or output");

    assert!(
        error
            .to_string()
            .contains("NautilusTrader revision mismatch"),
        "{error:#}"
    );
    assert!(!output.exists());
}

#[test]
fn source_universe_durable_preflight_rejects_non_trade_family_without_output() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let (spec, _) = two_interval_run_spec(temp.path(), "source-universe-durable-bars");
    let output = temp.path().join("must-not-exist");
    let registry = VerifiedSourceBindingRegistry::from_run_spec(&spec)
        .expect("freeze the bars source-binding registry");

    let error = validate_durable_run_spec_preflight(&spec, &registry)
        .expect_err("bars must fail before durable source bytes or output");

    assert!(
        error
            .to_string()
            .contains("durable operator capability requires exactly one configured tuple"),
        "{error:#}"
    );
    assert!(error.to_string().contains("found 0"), "{error:#}");
    assert!(!output.exists());
}
