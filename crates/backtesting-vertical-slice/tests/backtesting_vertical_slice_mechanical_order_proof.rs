//! Mechanical order-producing proof for the NautilusTrader backtesting vertical slice.
//!
//! The end-to-end test is the negative control: the quote-driven production
//! strategy over trade-only TRADE_REPLAY data places zero orders by
//! construction. This test is the positive control. It runs the same pipeline
//! over the same deterministic 3-trade accepted dataset, but selects the
//! bolt-owned `mechanical_trade_replay_probe`, which submits a market entry on
//! the first delivered trade and a reduce-only market close on the second.
//!
//! NautilusTrader's simulated venue seeds both bid and ask from the first
//! delivered trade and the backtest engine routes each data point to the
//! exchange before running the strategy callbacks, so the market orders fill
//! immediately. This proves the `data -> strategy -> orders -> positions ->
//! result-contract` path end to end. The orders carry zero execution-quality
//! meaning under TRADE_REPLAY fidelity; the contract's claim limits still
//! forbid execution-quality claims, which this test asserts.

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
        ManifestVenueConfig, MarketStructureFixture, RunPurpose,
        STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE, StrategySource, StrategySourceKind,
    },
    runner::{BacktestRunInputs, OrderTerminalRecord, run_backtest},
    source_proof::{
        AcceptanceMode, AcceptanceScope, AcceptedDataset, EvidenceState, FixtureType,
        IngestManifestObjectRecord, L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck,
        RequiredChecks, SourceCandidateClass, SourceProofClaimLimit, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, SourceProofUsageScope, SourceSelectionStatus,
        TimeRange, select_accepted_dataset,
    },
};
use nautilus_model::enums::OrderStatus;

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
        run_id: "backtesting-vertical-slice-mechanical-order-proof".to_string(),
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
            registry_key: STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE.to_string(),
            parameters: BTreeMap::from([
                ("trade_size".to_string(), "0.01".to_string()),
                ("entry_after_trades".to_string(), "1".to_string()),
                ("exit_after_trades".to_string(), "1".to_string()),
                ("side".to_string(), "buy".to_string()),
            ]),
            typed_config_uri: None,
            typed_config_hash: None,
            experiment_result_uri: None,
            experiment_result_hash: None,
            config_overlay: None,
        },
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
        output_prefix: "s3://bolt-parquet/nt-research-analytics/backtests/mechanical-order-proof"
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
fn mechanical_probe_produces_orders_and_positions_through_result_contract() {
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
    let work_budget =
        backtesting_vertical_slice::operator_work_budget::OperatorWorkBudgetGuard::unbounded();

    let output = run_backtest(BacktestRunInputs {
        work_budget: &work_budget,
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
                "s3://bolt-parquet/nt-research-analytics/backtests/probe/catalog-metadata.json"
                    .to_string(),
            result_contract_uri:
                "s3://bolt-parquet/nt-research-analytics/backtests/probe/result.json".to_string(),
        },
    })
    .expect("mechanical probe backtest run");

    // The engine consumed every accepted trade (same read-back proof as the
    // negative control), and the probe turned that consumption into real orders.
    assert_eq!(output.read_back_count, 3);
    assert_eq!(
        output.nt_result.iterations, 3,
        "engine must iterate exactly once per accepted trade"
    );

    // Positive control: orders and positions were produced and filled. The entry
    // market order fills against the trade-seeded book on the first trade, and
    // the reduce-only close fills on the second.
    assert!(
        output.nt_result.total_orders > 0,
        "probe must produce orders: {}",
        output.nt_result.total_orders
    );

    // Self-evidencing order-terminal proof: pull the post-run cache's view of
    // every order and require each to be FILLED. The summary `total_orders`
    // counts denied/rejected/canceled orders too, so a non-zero order count is
    // necessary but not sufficient — a denial at the risk or matching engine
    // would still register an order while producing zero positions. On any
    // non-fill terminal state, the panic dumps each order's full event trail so
    // the CI log carries the exact denial/rejection/cancel reason instead of an
    // opaque "0 positions".
    assert!(
        !output.order_terminals.is_empty(),
        "probe must record order terminal states in the post-run cache"
    );
    let unfilled: Vec<&OrderTerminalRecord> = output
        .order_terminals
        .iter()
        .filter(|order| order.status != OrderStatus::Filled)
        .collect();
    assert!(
        unfilled.is_empty(),
        "every submitted order must reach FILLED; unfilled orders (with full event trail):\n{}",
        unfilled
            .iter()
            .map(|order| format!(
                "  {} {} {} status={:?} qty={} filled_qty={} events={:#?}",
                order.client_order_id,
                order.order_side,
                order.order_type,
                order.status,
                order.quantity,
                order.filled_qty,
                order.events_debug,
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // Both legs filled (entry + reduce-only close), so both orders are FILLED.
    assert_eq!(
        output.order_terminals.len(),
        2,
        "probe submits exactly two orders (entry + reduce-only close): {:#?}",
        output.order_terminals
    );

    assert!(
        output.nt_result.total_positions > 0,
        "probe must produce positions: {}",
        output.nt_result.total_positions
    );

    let contract = &output.contract;
    contract
        .validate()
        .expect("contract is objective and complete");
    assert_eq!(
        contract.fidelity_class,
        SourceProofFidelityClass::TradeReplay
    );

    // The TRADE_REPLAY no-execution-quality claim limit is preserved: the probe's
    // orders must never be read as execution-quality evidence.
    assert!(
        !contract.claim_limits.is_empty(),
        "contract must carry claim limits"
    );
    assert!(
        contract
            .claim_limits
            .iter()
            .any(|limit| limit.contains("execution-quality")),
        "contract must keep the no-execution-quality claim limit: {:?}",
        contract.claim_limits
    );

    // The zero-order warning must NOT fire: the probe produced orders, so the
    // honest negative-control rationale does not apply here.
    assert!(
        contract.warnings.is_empty(),
        "no zero-order warning may fire when the probe places orders: {:?}",
        contract.warnings
    );
}
