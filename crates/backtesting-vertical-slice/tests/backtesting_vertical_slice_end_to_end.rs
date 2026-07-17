//! End-to-end proof for the NautilusTrader backtesting vertical slice.
//!
//! Drives gates 1 through 6 through [`run_backtest`] with a small deterministic
//! accepted dataset: source-proof acceptance, the accepted-data ledger,
//! canonical normalization and Parquet artifact, NautilusTrader catalog projection
//! and read-back, manifest validation and NautilusTrader config mapping, a
//! `BacktestNode` run of a compiled Rust strategy, and the objective result
//! contract. CI-safe (no network): this test exercises the pipeline with
//! committed synthetic data only. Real-object verification is operator-run-only
//! via the `backtesting_vertical_slice` binary (`--run-spec`, `--object`,
//! `--output-dir`) and is not asserted here.

use std::collections::BTreeMap;

use backtesting_vertical_slice::{
    canonical_trades::{
        CanonicalInstrumentIdentity, ConverterConfig, CsvTimestampUnit, CsvTradeMappingConfig,
        RawPayloadConfig, RawPayloadContainer, TRANSFORM_IDENTITY,
    },
    catalog_projection::SpotInstrumentSpec,
    result_contract::ResultArtifactUris,
    run_manifest::{
        BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION, BacktestingRunManifest, ManifestCatalogInput,
        ManifestVenueConfig, MarketStructureFixture, RunPurpose, STRATEGY_HURST_VPIN_DIRECTIONAL,
        StrategySource, StrategySourceKind,
    },
    runner::{BacktestRunInputs, run_backtest},
    source_proof::{
        AcceptanceMode, AcceptanceScope, AcceptedDataset, EvidenceState, FixtureType,
        IngestManifestObjectRecord, L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck,
        RequiredChecks, SourceCandidateClass, SourceProofClaimLimit, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, SourceProofUsageScope, SourceSelectionStatus,
        TimeRange, select_accepted_dataset,
    },
};

const SAMPLE_CSV: &str = "id,timestamp,price,volume,side,rpi\n\
    1,1772323201665,617.2,0.3,buy,0\n\
    2,1772323312219,617.9,0.1456,sell,0\n\
    3,1772323312236,617,0.1544,sell,0\n";

fn csv_mapping() -> CsvTradeMappingConfig {
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

fn converter_config() -> ConverterConfig {
    ConverterConfig {
        identity: TRANSFORM_IDENTITY.to_string(),
        version: "1".to_string(),
        raw_payload: RawPayloadConfig {
            container: RawPayloadContainer::CsvGzip,
            max_object_bytes: 4096,
            max_decoded_bytes: 4096,
            zip_member: None,
            max_member_bytes: None,
            member_suffix: None,
        },
        csv: csv_mapping(),
        bars: None,
        paged_json_bars: None,
        jsonl_bars: None,
        deltas: None,
        quotes: None,
        seeded_l2_quotes: None,
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
        retention_freshness: RequiredCheck::passed("retention://bybit-public-archive-reviewed"),
        granularity: RequiredCheck::passed("native_trade_prints"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("nt://TradeTick"),
        cost: RequiredCheck::passed("cost://free-public-archive"),
        storage: RequiredCheck::passed("s3://bolt-parquet/.../source-proofs/"),
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
    let forbidden_claims = vec![
        "No execution-quality, queue-position, or order-book-liquidity claims.".to_string(),
        "No L2/L3 order-book replay claims from trade prints.".to_string(),
        "Coverage is limited to BNBUSDC spot 2026-03-01; no multi-day or multi-instrument claims."
            .to_string(),
    ];
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
        source_candidate_class: SourceCandidateClass::OfficialFree,
        source_selection_status: SourceSelectionStatus::AcceptedLowerFidelity,
        usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        official_free_gap_ref: None,
        paid_vendor_gap_ref: None,
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
        license_scope: LicenseScope::Public,
        retention_ref: "https://public.bybit.com/ (retention reviewed 2026-06-02)".to_string(),
        cost_ref: "cost://free-public-archive".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        l2_replay_evidence: L2ReplayEvidence {
            order_book_delta_ref: None,
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: None,
            timed_instrument_epoch_replay_ref: None,
        },
        // Mirrors the committed reference source proof exactly so the fixture
        // carries the full set of fidelity constraints through to the contract.
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
        manifest_schema_version: BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION.to_string(),
        run_id: "backtesting-vertical-slice-end-to-end".to_string(),
        target_bolt_v2_branch: "main".to_string(),
        target_bolt_v2_ref: "refs/heads/main".to_string(),
        resolved_nt_version: backtesting_vertical_slice::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
            .expect("BVS NautilusTrader dependency provenance"),
        market_structure_fixture: MarketStructureFixture::PerpsSpot,
        venue_binding_key: "bybit-spot-tick-trades".to_string(),
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
                    "BNBUSDC.BYBIT-1-MINUTE-LAST-INTERNAL".to_string(),
                ),
            ]),
            typed_config_uri: None,
            typed_config_hash: None,
            experiment_result_uri: None,
            experiment_result_hash: None,
            config_overlay: None,
        },
        economics_snapshots: Vec::new(),
        strategy_config_hash: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
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
            leverages: None,
            margin_model: None,
            modules: None,
            fill_model: None,
            latency_model: None,
            fee_model: None,
            settlement_prices: None,
        },
        additional_venues: Vec::new(),
        catalog_inputs: vec![ManifestCatalogInput {
            catalog_path: catalog_path.to_string(),
            catalog_fs_protocol: "NONE".to_string(),
            catalog_fs_storage_options: BTreeMap::new(),
            catalog_fs_rust_storage_options: BTreeMap::new(),
            data_type: "TradeTick".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
            instrument_ids: None,
            start_time: None,
            end_time: None,
            filter_expr: None,
            client_id: None,
            metadata: None,
            bar_spec: None,
            bar_types: None,
            optimize_file_loading: None,
        }],
        reconstructed_reference_current_price: Vec::new(),
        instrument_settlements: Vec::new(),
        catalog_hash: "1111111111111111111111111111111111111111111111111111111111111111"
            .to_string(),
        execution_model: "nt_backtest_node".to_string(),
        artifact_root: "s3://bolt-parquet/nt-research-analytics".to_string(),
        output_prefix:
            "s3://bolt-parquet/nt-research-analytics/backtests/backtesting-vertical-slice-end-to-end"
                .to_string(),
        artifact_store: backtesting_vertical_slice::run_manifest::ManifestArtifactStore {
            storage_options: BTreeMap::new(),
            rust_storage_options: BTreeMap::new(),
            ssm_parameters: None,
        },
        domain_metrics: Vec::new(),
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
        selector_provenance: None,
        created_at: "2026-06-02T00:00:00Z",
        artifact_uris: ResultArtifactUris {
            source_proof_uri: "s3://bolt-parquet/nt-research-analytics/source-proofs/p.json"
                .to_string(),
            canonical_table_uri: canonical_path.to_string_lossy().to_string(),
            nt_catalog_uri: catalog_path.clone(),
            nt_catalog_manifest_uri: None,
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
    assert_eq!(contract.execution_model, manifest.execution_model);
    assert_eq!(
        contract.venue_queue_position,
        Some(manifest.venue.queue_position)
    );
    assert_eq!(
        contract.catalog_data_types,
        manifest
            .catalog_inputs
            .iter()
            .map(|input| input.data_type.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(contract.warnings.len(), 1);
    assert!(
        contract.warnings[0].contains("TRADE_REPLAY"),
        "warning must explain the trade-only fidelity: {:?}",
        contract.warnings[0]
    );
    assert!(
        contract.claim_limits.iter().any(|limit| {
            limit.contains("NT defaulted surface run.chunk_size")
                && limit.contains("resolved_value=None")
        }),
        "contract must record resolved NT defaults in claim limits: {:?}",
        contract.claim_limits
    );
    assert!(
        contract.claim_limits.iter().any(|limit| {
            limit.contains("source_proof_claim_limit id=claim-limit-1")
                && limit.contains("severity=blocking")
                && limit.contains(
                    "claim=No execution-quality, queue-position, or order-book-liquidity claims.",
                )
                && limit.contains("reason=source fidelity does not prove this claim")
                && limit.contains("evidence_ref=source-proof://fidelity-class")
        }),
        "contract must preserve structured source-proof claim-limit evidence: {:?}",
        contract.claim_limits
    );
    assert!(
        contract.claim_limits.iter().any(|limit| {
            limit.contains("NT pass_through surface run.id")
                && limit.contains("backtesting-vertical-slice-end-to-end")
        }),
        "contract must record TOML-to-NT pass-through surfaces: {:?}",
        contract.claim_limits
    );
    assert!(
        contract.claim_limits.iter().any(|limit| {
            limit.contains("NT unsupported_for_now surface venue.settlement_prices")
        }),
        "contract must record unsupported NT surfaces: {:?}",
        contract.claim_limits
    );
    assert!(
        contract.claim_limits.len() > 3,
        "contract must retain source limits and add NT surface limits"
    );
    assert_eq!(contract.catalog_hash, output.projection.catalog_hash);
}

#[test]
fn time_window_gate_admits_by_ts_init_receipt_clock() {
    // The manifest's optional `[start_time, end_time]` window maps into
    // NautilusTrader's `BacktestRunConfig` start/end, which NautilusTrader applies
    // to each tick's `ts_init` — the availability-or-capture receipt clock — never
    // the exchange event clock (#677). Native CSV trades carry no per-row
    // availability and a single batch `capture_time`, so every projected trade
    // shares one `ts_init == capture_time`: a window covering that capture instant
    // admits the whole table (the end bound is inclusive), and a window that ends
    // before it is rejected by the overlap gate as excluding all accepted data.
    // Trade-replay windows are therefore all-or-nothing while one capture clock
    // governs the table; giving native CSV trades a per-row receipt clock (so a
    // sub-window can trim a trade table) is tracked as spine follow-up.
    let accepted = accepted_dataset();
    let identity = CanonicalInstrumentIdentity {
        instrument_id: "BNBUSDC".to_string(),
        venue_symbol: "BNBUSDC".to_string(),
        nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
    };
    // The single batch receipt clock stamped on every native CSV trade; both
    // windows below are expressed against it so the test proves windowing on
    // `ts_init`, not the strictly-earlier event clock.
    let capture_time_nanos = 1_772_512_022_000_000_000_i64;

    // A window whose `end_time` sits exactly on the capture instant admits all
    // three trades (NautilusTrader's end bound is inclusive on `ts_init`), so the
    // iterations gate sees the full set rather than spuriously bailing.
    let admit_temp = tempfile::TempDir::new().expect("temp dir");
    let admit_canonical = admit_temp.path().join("canonical-trades.parquet");
    let admit_catalog = admit_temp.path().join("nt-catalog");
    let admit_catalog_path = admit_catalog.to_str().unwrap().to_string();
    let mut admit_manifest = manifest(&admit_catalog_path);
    admit_manifest.end_time = Some(capture_time_nanos);
    let admit_contract_manifest_hash = admit_manifest.manifest_hash();
    let admitted = run_backtest(BacktestRunInputs {
        accepted: &accepted,
        identity: &identity,
        instrument_spec: &instrument_spec(),
        csv_text: SAMPLE_CSV,
        capture_time_nanos,
        manifest: &admit_manifest,
        contract_manifest_hash: &admit_contract_manifest_hash,
        converter: &converter_config(),
        canonical_artifact_path: &admit_canonical,
        catalog_root: &admit_catalog,
        selector_provenance: None,
        created_at: "2026-06-02T00:00:00Z",
        artifact_uris: ResultArtifactUris {
            source_proof_uri: "s3://bolt-parquet/nt-research-analytics/source-proofs/p.json"
                .to_string(),
            canonical_table_uri: admit_canonical.to_string_lossy().to_string(),
            nt_catalog_uri: admit_catalog_path.clone(),
            nt_catalog_manifest_uri: None,
            catalog_metadata_uri:
                "s3://bolt-parquet/nt-research-analytics/backtests/win/catalog-metadata.json"
                    .to_string(),
            result_contract_uri: "s3://bolt-parquet/nt-research-analytics/backtests/win/r.json"
                .to_string(),
        },
    })
    .expect("window covering the capture instant must admit all trades");

    // Every native CSV trade shares one `ts_init == capture_time` while its event
    // clock is strictly earlier, so the admitting window above is unambiguously on
    // the receipt clock, not the event clock.
    assert_eq!(admitted.canonical_table.rows.len(), 3);
    for row in &admitted.canonical_table.rows {
        assert_eq!(row.availability_time, None);
        assert_eq!(row.capture_time, capture_time_nanos);
        assert!(
            row.event_time < capture_time_nanos,
            "fixture event clock must be earlier than the receipt clock"
        );
    }
    // The catalog still projects all three accepted trades; the window only governs
    // the engine run, which iterates over every in-window (here, all) trade.
    assert_eq!(admitted.read_back_count, 3);
    assert_eq!(
        admitted.nt_result.iterations, 3,
        "a window covering the shared ts_init admits every trade (end bound inclusive)"
    );

    // A window that ends one nanosecond before the shared receipt instant excludes
    // every trade by `ts_init`: the overlap gate must reject the run rather than
    // silently report a zero-iteration backtest against the accepted source.
    let reject_temp = tempfile::TempDir::new().expect("temp dir");
    let reject_canonical = reject_temp.path().join("canonical-trades.parquet");
    let reject_catalog = reject_temp.path().join("nt-catalog");
    let reject_catalog_path = reject_catalog.to_str().unwrap().to_string();
    let mut reject_manifest = manifest(&reject_catalog_path);
    reject_manifest.end_time = Some(capture_time_nanos - 1);
    let reject_contract_manifest_hash = reject_manifest.manifest_hash();
    let rejected = run_backtest(BacktestRunInputs {
        accepted: &accepted,
        identity: &identity,
        instrument_spec: &instrument_spec(),
        csv_text: SAMPLE_CSV,
        capture_time_nanos,
        manifest: &reject_manifest,
        contract_manifest_hash: &reject_contract_manifest_hash,
        converter: &converter_config(),
        canonical_artifact_path: &reject_canonical,
        catalog_root: &reject_catalog,
        selector_provenance: None,
        created_at: "2026-06-02T00:00:00Z",
        artifact_uris: ResultArtifactUris {
            source_proof_uri: "s3://bolt-parquet/nt-research-analytics/source-proofs/p.json"
                .to_string(),
            canonical_table_uri: reject_canonical.to_string_lossy().to_string(),
            nt_catalog_uri: reject_catalog_path.clone(),
            nt_catalog_manifest_uri: None,
            catalog_metadata_uri:
                "s3://bolt-parquet/nt-research-analytics/backtests/win/catalog-metadata-2.json"
                    .to_string(),
            result_contract_uri: "s3://bolt-parquet/nt-research-analytics/backtests/win/r2.json"
                .to_string(),
        },
    });
    // `let Err(..) else` extracts the error without requiring the Ok type
    // (`BacktestRunOutput`, which holds NT's non-Debug `BacktestResult`) to
    // implement `Debug` as `Result::expect_err` would.
    let Err(error) = rejected else {
        panic!("window before the receipt instant must be rejected");
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("end_time") && message.contains("excludes all accepted data"),
        "overlap gate must explain the empty window: {message}"
    );
}
