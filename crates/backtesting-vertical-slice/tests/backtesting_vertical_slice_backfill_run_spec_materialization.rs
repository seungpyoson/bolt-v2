use backtesting_vertical_slice::{
    backfill_accepted_tranche::{
        BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION, BackfillAcceptedTrancheManifest,
        BackfillAcceptedTrancheObject, BackfillAcceptedTrancheStatus,
    },
    backfill_execution_plan::{
        BackfillExecutionPlanStatus, BackfillExecutionRunBinding, BackfillExecutionWorkBudget,
    },
    backfill_run_spec_materialization::{
        BACKFILL_RUN_SPEC_MATERIALIZED_FILE, BackfillRunSpecMaterializationSpec,
        write_backfill_run_spec_from_materialization_spec,
    },
    operator::RunSpec,
    source_proof::SourceProofUsageScope,
};

#[test]
fn materialized_run_spec_binds_accepted_tranche_before_payload_fetch() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let tranche_path = dir.path().join("accepted-tranche.json");
    let template_path = dir.path().join("run-spec-template.toml");
    std::fs::write(
        &tranche_path,
        serde_json::to_vec_pretty(&accepted_tranche()).expect("accepted tranche"),
    )
    .expect("write tranche");
    std::fs::write(&template_path, run_spec_template()).expect("write template");

    let artifact =
        write_backfill_run_spec_from_materialization_spec(&BackfillRunSpecMaterializationSpec {
            materialization_id: "synthetic-materialization".to_string(),
            accepted_tranche_manifest_path: tranche_path,
            run_spec_template_path: template_path,
            output_dir: dir.path().join("materialized"),
            run_id: "synthetic-materialized-run".to_string(),
            output_prefix: "s3://synthetic-artifacts/backtests/synthetic-materialized-run"
                .to_string(),
        })
        .expect("materialized run spec");

    assert_eq!(
        artifact.path,
        dir.path()
            .join("materialized")
            .join(BACKFILL_RUN_SPEC_MATERIALIZED_FILE)
    );

    let materialized = std::fs::read_to_string(&artifact.path).expect("read materialized");
    assert!(!materialized.contains("stale-object-sha"));
    assert!(!materialized.contains("stale-source-binding"));
    assert!(!materialized.contains("stale-table-family"));

    let run_spec: RunSpec = toml::from_str(&materialized).expect("parse materialized run spec");
    let binding = BackfillExecutionRunBinding::from_run_spec(&run_spec);
    let plan =
        backtesting_vertical_slice::backfill_execution_plan::evaluate_backfill_execution_plan(
            "synthetic-plan",
            "synthetic-tranche-hash",
            &accepted_tranche(),
            artifact.content_hash.clone(),
            &binding,
            BackfillExecutionWorkBudget {
                max_source_rows: 128,
                max_projected_row_groups: 1,
                max_wall_seconds: 30,
                require_object_selection_metadata: false,
            },
        );

    assert_eq!(plan.status, BackfillExecutionPlanStatus::Ready);
    assert!(plan.blocking_issues.is_empty());
    assert_eq!(plan.operator_run_id, "synthetic-materialized-run");
    assert_eq!(
        plan.output_prefix,
        "s3://synthetic-artifacts/backtests/synthetic-materialized-run"
    );
    assert_eq!(plan.source_proof_id, "source-proof-synthetic");
    assert_eq!(plan.source_proof_version, 7);
    assert_eq!(plan.source_binding, "synthetic-source-binding");
    assert_eq!(plan.table_family, "trades");
    assert_eq!(
        plan.source_usage_scope,
        SourceProofUsageScope::CanonicalBackfillInput
    );
    assert_eq!(plan.objects.len(), 1);
    assert_eq!(plan.objects[0].sha256, "synthetic-object-sha");
    assert_eq!(plan.max_object_bytes, 17);
}

#[test]
fn materialized_run_spec_carries_accepted_tranche_usage_scope_before_payload_fetch() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let tranche_path = dir.path().join("accepted-tranche.json");
    let template_path = dir.path().join("run-spec-template.toml");
    let mut tranche = accepted_tranche();
    tranche.source_usage_scope = SourceProofUsageScope::OneOffBackfillData;
    std::fs::write(
        &tranche_path,
        serde_json::to_vec_pretty(&tranche).expect("accepted tranche"),
    )
    .expect("write tranche");
    std::fs::write(&template_path, run_spec_template()).expect("write template");

    let artifact =
        write_backfill_run_spec_from_materialization_spec(&BackfillRunSpecMaterializationSpec {
            materialization_id: "synthetic-materialization".to_string(),
            accepted_tranche_manifest_path: tranche_path,
            run_spec_template_path: template_path,
            output_dir: dir.path().join("materialized"),
            run_id: "synthetic-materialized-run".to_string(),
            output_prefix: "s3://synthetic-artifacts/backtests/synthetic-materialized-run"
                .to_string(),
        })
        .expect("materialized run spec");

    let materialized = std::fs::read_to_string(&artifact.path).expect("read materialized");
    assert!(materialized.contains(r#"usage_scope = "one_off_backfill_data""#));
    let run_spec: RunSpec = toml::from_str(&materialized).expect("parse materialized run spec");
    assert_eq!(
        run_spec.source_proof.usage_scope,
        SourceProofUsageScope::OneOffBackfillData
    );
    let binding = BackfillExecutionRunBinding::from_run_spec(&run_spec);
    assert_eq!(binding.source_usage_scope, tranche.source_usage_scope);
}

fn accepted_tranche() -> BackfillAcceptedTrancheManifest {
    BackfillAcceptedTrancheManifest {
        schema_version: BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION.to_string(),
        tranche_id: "synthetic-tranche".to_string(),
        status: BackfillAcceptedTrancheStatus::Accepted,
        source_proof_scope_report_id: "synthetic-source-proof-scope".to_string(),
        source_proof_scope_report_hash: "synthetic-source-proof-scope-hash".to_string(),
        source_proof_id: "source-proof-synthetic".to_string(),
        source_proof_version: 7,
        source_binding: "synthetic-source-binding".to_string(),
        table_family: "trades".to_string(),
        source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        parent_manifest_id: "synthetic-parent-manifest".to_string(),
        object_level_tranche_required: true,
        object_count: 1,
        accepted_bytes: 17,
        objects: vec![BackfillAcceptedTrancheObject {
            s3_uri: "s3://synthetic-artifacts/raw/object=synthetic-object-sha.csv.gz".to_string(),
            source_url: "https://data.example.invalid/synthetic-object.csv.gz".to_string(),
            sha256: "synthetic-object-sha".to_string(),
            bytes: 17,
            archive_date: "2026-03-01".to_string(),
            source_row_groups: Vec::new(),
            predicate_ref: None,
        }],
        blocking_issues: Vec::new(),
    }
}

fn run_spec_template() -> String {
    r#"
capture_time_utc = "2026-06-02T04:27:02Z"
created_at_utc = "2026-06-02T00:00:00Z"
accepted_by = "synthetic-operator"
accepted_at_utc = "2026-06-02T00:00:00Z"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[accepted_object]
s3_uri = "s3://synthetic-artifacts/raw/object=stale-object-sha.csv.gz"
source_url = "https://data.example.invalid/stale-object.csv.gz"
sha256 = "stale-object-sha"
bytes = 99
archive_date = "2026-01-01"
schema_columns = ["id", "timestamp", "price", "volume", "side"]

[source_proof]
source_proof_id = "stale-source-proof"
source_proof_version = 1
contract_version = "backfill-table-contract.v1"
schema_version = "backfill-source-proof.v1"
status = "accepted"
acceptance_mode = "manual"
accepted_by = "synthetic-operator"
accepted_at = "2026-06-02T00:00:00Z"
source_binding = "stale-source-binding"
venue = "synthetic"
product_family = "spot"
product_category = "spot"
table_family = "stale-table-family"
evidence_state = "owner_archive_backfillable"
source_candidate_class = "official_free"
source_selection_status = "ACCEPTED_LOWER_FIDELITY"
fixture_type = "perps-spot"
instrument_universe_id = "synthetic-instrument-universe"
raw_sample_uri = "s3://synthetic-artifacts/raw/object=stale-object-sha.csv.gz"
raw_sample_hash = "stale-object-sha"
schema_sample_uri = "s3://synthetic-artifacts/manifests/synthetic-manifest.json"
schema_sample_hash = "synthetic-schema-hash"
license_ref = "https://data.example.invalid/license"
license_scope = "public"
retention_ref = "https://data.example.invalid/retention"
cost_ref = "cost://synthetic"
nt_mapping_status = "accepted"
fidelity_class = "TRADE_REPLAY"
gap_policy_id = ""
forbidden_claims = []

[source_proof.l2_replay_evidence]

[source_proof.acceptance_scope]
planned_objects = 1
completed_objects = 1
failed_objects = 0
skipped_objects = 0
accepted_bytes = 99
selector_scope_violations = 0

[source_proof.requested_time_range]
start_utc = "2026-03-01T00:00:00Z"
end_utc = "2026-03-02T00:00:00Z"

[source_proof.coverage_time_range]
start_utc = "2026-03-01T00:00:00Z"
end_utc = "2026-03-02T00:00:00Z"

[source_proof.required_checks.source_access]
outcome = "passed"
evidence_ref = "synthetic source access"

[source_proof.required_checks.license]
outcome = "passed"
evidence_ref = "synthetic license"

[source_proof.required_checks.schema]
outcome = "passed"
evidence_ref = "synthetic schema"

[source_proof.required_checks.time_semantics]
outcome = "passed"
evidence_ref = "synthetic time semantics"

[source_proof.required_checks.instrument_universe]
outcome = "passed"
evidence_ref = "synthetic instrument universe"

[source_proof.required_checks.coverage]
outcome = "passed"
evidence_ref = "synthetic coverage"

[source_proof.required_checks.retention_freshness]
outcome = "passed"
evidence_ref = "synthetic retention"

[source_proof.required_checks.granularity]
outcome = "passed"
evidence_ref = "synthetic granularity"

[source_proof.required_checks.completeness]
outcome = "passed"
evidence_ref = "synthetic completeness"

[source_proof.required_checks.nt_mapping]
outcome = "passed"
evidence_ref = "synthetic NT mapping"

[source_proof.required_checks.cost]
outcome = "passed"
evidence_ref = "synthetic cost"

[source_proof.required_checks.storage]
outcome = "passed"
evidence_ref = "synthetic storage"

[instrument_spec]
nt_instrument_id = "SYNTHETIC.SIM"
raw_symbol = "SYNTHETIC"
base_currency = "BTC"
quote_currency = "USDT"
price_increment = "0.1"
size_increment = "0.0001"
min_quantity = "0.0001"
max_quantity = "100"
min_notional = "5"
max_notional = "100000"

[identity]
instrument_id = "SYNTHETIC"
venue_symbol = "SYNTHETIC"
nt_instrument_id = "SYNTHETIC.SIM"

[converter]
identity = "csv-native-trades-to-canonical-trades.v1"
version = "1"

[converter.raw_payload]
container = "csv_gzip"
max_object_bytes = 99
max_decoded_bytes = 4096

[converter.csv]
has_headers = true
trade_id_column = "id"
timestamp_column = "timestamp"
timestamp_unit = "milliseconds"
price_column = "price"
size_column = "volume"
side_column = "side"
buyer_side_values = ["buy"]
seller_side_values = ["sell"]

[manifest]
manifest_schema_version = "backtesting-run-manifest.v1"
run_id = "stale-run"
target_bolt_v2_branch = "main"
target_bolt_v2_ref = "refs/heads/main"
resolved_nt_version = "__NT_REVISION__"
market_structure_fixture = "perps-spot"
venue_binding_key = "stale-source-binding"
run_purpose = "normal"
source_proof_id = "stale-source-proof"
source_proof_version = 1
pins_non_latest_proof = false
strategy_config_hash = "a99e8a42bfa6df1f790ccc1a3a2c0a5ea7dd122e3ffab73e685be4132bbef396"
catalog_hash = "530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f"
execution_model = "nt_backtest_node"
artifact_root = "s3://synthetic-artifacts"
output_prefix = "s3://synthetic-artifacts/backtests/stale-run"

[manifest.artifact_store]
storage_options = {}
rust_storage_options = { region = "us-east-1", conditional_put = "etag" }

[manifest.strategy]
source_kind = "compiled_rust_registry"
registry_key = "hurst_vpin_directional"

[manifest.strategy.parameters]
trade_size = "0.01"
bar_type = "SYNTHETIC.SIM-1-MINUTE-LAST-INTERNAL"

[manifest.venue]
nt_venue = "SIM"
oms_type = "NETTING"
account_type = "CASH"
book_type = "L1_MBP"
starting_balances = ["1_000_000 USDT"]
routing = false
frozen_account = false
reject_stop_orders = true
support_gtd_orders = true
support_contingent_orders = true
use_position_ids = true
use_random_ids = false
use_reduce_only = true
bar_execution = true
bar_adaptive_high_low_ordering = false
trade_execution = true
use_market_order_acks = false
liquidity_consumption = false
allow_cash_borrowing = false
queue_position = false
oto_trigger_mode = "PARTIAL"
base_currency = "NONE"
default_leverage = "1"
price_protection_points = 0

[[manifest.catalog_inputs]]
catalog_path = "overridden-by-binary-at-runtime"
catalog_fs_protocol = "NONE"
catalog_fs_storage_options = {}
catalog_fs_rust_storage_options = {}
data_type = "TradeTick"
nt_instrument_id = "SYNTHETIC.SIM"
"#
    .replace(
        "__NT_REVISION__",
        &backtesting_vertical_slice::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
            .expect("BVS NautilusTrader dependency provenance"),
    )
}
