use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::{
    backfill_accepted_tranche::{
        BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION, BackfillAcceptedTrancheManifest,
        BackfillAcceptedTrancheStatus, write_backfill_accepted_tranche_manifest,
        write_backfill_accepted_tranche_manifest_from_spec_file,
    },
    backfill_conversion_batch::write_backfill_conversion_batch_plan_from_spec_file,
    backfill_conversion_completion::write_backfill_conversion_completion_ledger_from_spec_file,
    backfill_coverage::write_coverage_ledger_artifact_from_spec_file,
    backfill_execution_plan::write_backfill_execution_plan_from_spec_file,
    backfill_execution_readiness::write_backfill_execution_readiness_report_from_spec_file,
    backfill_object_staging::stage_backfill_object_from_spec_file_with_resolver,
    backfill_run_spec_materialization::write_backfill_run_spec_from_materialization_spec_file,
    backfill_source_proof_scope::write_backfill_source_proof_scope_report_from_spec_file,
    reference_fixture_index::repo_root_from_manifest_dir,
    research_analytics::{read_accepted_object_for_run_spec, read_run_spec_with_hash},
    source_proof::SourceProofUsageScope,
    retired_backfill_evidence::{
        RetiredBackfillEvidenceInventory, is_retired_backfill_runtime_path,
    },
    source_catalog_mapping_readiness::write_source_catalog_mapping_readiness_report_from_spec_file,
};

const REFERENCE_ROOT: &str = "specs/023-nt-research-analytics-platform/reference";
const RETIRED_GATE_ROOT: &str = "specs/023-nt-research-analytics-platform/reference/backfill-gates";
const ACTIVE_GOLDEN_RUN_SPECS: &[&str] = &[
    "backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-03-01.toml",
    "backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml",
];

#[test]
fn retired_backfill_inventory_preserves_exact_typed_evidence_and_tombstones() {
    let repo_root = repo_root_from_manifest_dir();
    let inventory = RetiredBackfillEvidenceInventory::load(&repo_root)
        .expect("retired backfill evidence inventory loads and validates");

    assert_eq!(inventory.records.len(), 185);
    assert_eq!(
        inventory
            .records
            .iter()
            .filter(|record| record.venue.as_str() == "binance")
            .count(),
        92
    );
    assert_eq!(
        inventory
            .records
            .iter()
            .filter(|record| record.venue.as_str() == "bybit")
            .count(),
        93
    );
    let tombstones = inventory.tombstoned_paths();
    assert_eq!(tombstones.len(), 2_971);
    for path in tombstones {
        assert!(
            is_retired_backfill_runtime_path(Path::new(path)),
            "exact inventory tombstone {path:?} must remain runtime-retired"
        );
        assert!(
            is_retired_backfill_runtime_path(&repo_root.join(path)),
            "absolute form of inventory tombstone {path:?} must remain runtime-retired"
        );
    }
    inventory
        .verify_retained_evidence(&repo_root)
        .expect("all retained evidence bytes and typed identities remain exact");
}

#[test]
fn retired_backfill_roots_and_daily_profiles_cannot_regrow() {
    let repo_root = repo_root_from_manifest_dir();
    assert!(
        !repo_root.join(RETIRED_GATE_ROOT).exists(),
        "the retired per-day backfill-gates tree must stay absent"
    );

    for retired_root in [
        "backfill-conversion-batches/binance-bnbusdc-2026-03-01-2026-05-31",
        "backfill-conversion-batches/bybit-bnbusdc-2026-03-01-2026-06-01",
        "backfill-coverage-ledgers/binance-bnbusdc-2026-03-01-2026-05-31",
        "backfill-coverage-ledgers/bybit-bnbusdc-2026-03-01-2026-06-01",
        "backfill-conversion-completion-ledgers/binance-bnbusdc-2026-03-01-2026-05-31",
        "backfill-conversion-completion-ledgers/bybit-bnbusdc-2026-03-01-2026-06-01",
    ] {
        assert!(
            !repo_root.join(REFERENCE_ROOT).join(retired_root).exists(),
            "retired aggregate root {retired_root} must stay absent"
        );
    }

    let reference_root = repo_root.join(REFERENCE_ROOT);
    let mut active_daily_profiles = fs::read_dir(&reference_root)
        .expect("read reference root")
        .map(|entry| entry.expect("read reference entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("backtesting-vertical-slice-run-spec.")
                        && name.contains("bnbusdc-2026-")
                        && name.ends_with(".toml")
                })
        })
        .map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .expect("UTF-8 run-spec file name")
                .to_string()
        })
        .collect::<Vec<_>>();
    active_daily_profiles.sort();
    assert_eq!(active_daily_profiles, ACTIVE_GOLDEN_RUN_SPECS);
}

#[test]
fn every_retained_repo_reference_to_the_retired_lane_has_one_tombstone() {
    let repo_root = repo_root_from_manifest_dir();
    let inventory = RetiredBackfillEvidenceInventory::load(&repo_root)
        .expect("retired backfill evidence inventory loads");
    let tombstones = inventory.tombstoned_paths();
    let mut observed = BTreeSet::new();

    for path in files_under(&repo_root) {
        if path.starts_with(repo_root.join(".git")) || path.starts_with(repo_root.join("target")) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for reference in repo_references_to_retired_lane(&text) {
            assert!(
                tombstones.contains(reference),
                "retained reference {reference:?} in {} lacks an exact tombstone",
                path.display()
            );
            observed.insert(reference.to_string());
        }
    }

    assert!(
        !observed.is_empty(),
        "the retained publication history must exercise tombstone resolution"
    );
}

#[test]
fn runtime_loaders_reject_retired_paths_before_filesystem_access() {
    for path in [
        "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02/materialized-run-spec/backfill-run-spec.toml",
        "specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-03-02.toml",
        "specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bybit-bnbusdc-2026-03-02.toml",
        "specs/023-nt-research-analytics-platform/reference/backfill-conversion-batches/binance-bnbusdc-2026-03-01-2026-05-31/backfill-conversion-batch-plan.toml",
        "specs/023-nt-research-analytics-platform/reference/backfill-coverage-ledgers/bybit-bnbusdc-2026-03-01-2026-06-01/backfill-coverage-ledger.toml",
        "specs/023-nt-research-analytics-platform/reference/backfill-conversion-completion-ledgers/bybit-bnbusdc-2026-03-01-2026-06-01/backfill-conversion-completion-ledger.toml",
    ] {
        assert!(is_retired_backfill_runtime_path(Path::new(path)), "{path}");
        let error = read_run_spec_with_hash(Path::new(path))
            .expect_err("retired path must reject before a filesystem read");
        assert!(error.to_string().contains("retired backfill"), "{error:#}");
        assert!(
            !error.to_string().contains("No such file"),
            "retirement policy must reject before absence happens to reject: {error:#}"
        );
    }

    for active in ACTIVE_GOLDEN_RUN_SPECS {
        assert!(
            !is_retired_backfill_runtime_path(Path::new(active)),
            "active golden profile {active} must remain loadable"
        );
    }
}

#[test]
fn public_legacy_loaders_reject_retired_specs_before_filesystem_access() {
    let daily_root = Path::new(
        "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02",
    );
    let aggregate_root = Path::new(
        "specs/023-nt-research-analytics-platform/reference/backfill-conversion-batches/binance-bnbusdc-2026-03-01-2026-05-31",
    );
    let coverage_root = Path::new(
        "specs/023-nt-research-analytics-platform/reference/backfill-coverage-ledgers/binance-bnbusdc-2026-03-01-2026-05-31",
    );
    let completion_root = Path::new(
        "specs/023-nt-research-analytics-platform/reference/backfill-conversion-completion-ledgers/binance-bnbusdc-2026-03-01-2026-05-31",
    );

    assert_retired_error_precedes_absence(
        write_backfill_accepted_tranche_manifest_from_spec_file(
            &daily_root.join("backfill-accepted-tranche.toml"),
        )
        .expect_err("accepted-tranche loader must reject retired spec"),
    );
    assert_retired_error_precedes_absence(
        write_backfill_execution_plan_from_spec_file(
            &daily_root.join("backfill-execution-plan.toml"),
        )
        .expect_err("execution-plan loader must reject retired spec"),
    );
    assert_retired_error_precedes_absence(
        write_backfill_execution_readiness_report_from_spec_file(
            &daily_root.join("backfill-execution-readiness.toml"),
        )
        .expect_err("execution-readiness loader must reject retired spec"),
    );
    assert_retired_error_precedes_absence(
        write_backfill_run_spec_from_materialization_spec_file(
            &daily_root.join("backfill-run-spec-materialization.toml"),
        )
        .expect_err("run-spec materialization loader must reject retired spec"),
    );
    let mut resolver = |_: &str, _: &str| -> Result<String, String> {
        panic!("retired object-staging spec must reject before secret resolution")
    };
    assert_retired_error_precedes_absence(
        stage_backfill_object_from_spec_file_with_resolver(
            &daily_root.join("object-staging/backfill-object-staging-manifest.json"),
            &mut resolver,
        )
        .expect_err("object-staging loader must reject retired spec"),
    );
    assert_retired_error_precedes_absence(
        write_backfill_source_proof_scope_report_from_spec_file(
            &daily_root.join("backfill-source-proof-scope.toml"),
        )
        .expect_err("source-proof-scope loader must reject retired spec"),
    );
    assert_retired_error_precedes_absence(
        write_source_catalog_mapping_readiness_report_from_spec_file(
            &daily_root.join("source-catalog-mapping-readiness.toml"),
        )
        .expect_err("catalog-mapping loader must reject retired spec"),
    );
    assert_retired_error_precedes_absence(
        write_backfill_conversion_batch_plan_from_spec_file(
            &aggregate_root.join("backfill-conversion-batch-plan.toml"),
        )
        .expect_err("conversion-batch loader must reject retired spec"),
    );
    assert_retired_error_precedes_absence(
        write_coverage_ledger_artifact_from_spec_file(
            &coverage_root.join("backfill-coverage-ledger.toml"),
        )
        .expect_err("coverage-ledger loader must reject retired spec"),
    );
    assert_retired_error_precedes_absence(
        write_backfill_conversion_completion_ledger_from_spec_file(
            &completion_root.join("backfill-conversion-completion-ledger.toml"),
        )
        .expect_err("conversion-completion loader must reject retired spec"),
    );
}

#[test]
fn public_legacy_loader_rejects_retired_nested_input_before_filesystem_access() {
    let temp = tempfile::tempdir().expect("create temp root");
    let active_spec_root = temp.path().join(
        "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02",
    );
    fs::create_dir_all(&active_spec_root).expect("create isolated retired-root shape");
    let retired_report = Path::new(
        "source-proof-scope/backfill-source-proof-scope-report.json",
    );
    let spec_path = active_spec_root.join("active-accepted-tranche.toml");
    fs::write(
        &spec_path,
        format!(
            "tranche_id = \"nested-retirement-guard\"\nsource_proof_scope_report_path = {:?}\noutput_dir = {:?}\n",
            retired_report,
            temp.path().join("output"),
        ),
    )
    .expect("write active spec with retired nested input");

    let error = write_backfill_accepted_tranche_manifest_from_spec_file(&spec_path)
        .expect_err("retired nested input must reject before absence");
    assert_retired_error_precedes_absence(error);
}

#[test]
fn public_legacy_loader_rejects_existing_retired_spec_before_toml_parsing() {
    let temp = tempfile::tempdir().expect("create temp root");
    let retired_spec = temp.path().join(
        "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02/backfill-accepted-tranche.toml",
    );
    fs::create_dir_all(retired_spec.parent().expect("retired spec parent"))
        .expect("create isolated retired-root shape");
    fs::write(&retired_spec, "this is not valid TOML = [")
        .expect("write deliberately malformed retired spec");

    let error = write_backfill_accepted_tranche_manifest_from_spec_file(&retired_spec)
        .expect_err("retired spec must reject before TOML parsing");
    let error = error.to_string();
    assert!(error.contains("retired backfill"), "{error}");
    assert!(
        !error.contains("parse backfill accepted-tranche spec"),
        "retirement policy must reject before TOML parsing: {error}"
    );
}

#[test]
fn public_legacy_writer_rejects_retired_final_path_before_directory_creation() {
    let temp = tempfile::tempdir().expect("create temp root");
    let output_dir = temp.path().join(
        "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02/accepted-tranche",
    );
    let manifest = BackfillAcceptedTrancheManifest {
        schema_version: BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION.to_string(),
        tranche_id: "retired-output-guard".to_string(),
        status: BackfillAcceptedTrancheStatus::Blocked,
        source_proof_scope_report_id: String::new(),
        source_proof_scope_report_hash: String::new(),
        source_proof_id: String::new(),
        source_proof_version: 0,
        source_binding: String::new(),
        table_family: "trades".to_string(),
        source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        parent_manifest_id: String::new(),
        object_level_tranche_required: true,
        object_count: 0,
        accepted_bytes: 0,
        objects: Vec::new(),
        blocking_issues: Vec::new(),
    };

    let error = write_backfill_accepted_tranche_manifest(&output_dir, &manifest)
        .expect_err("retired final path must reject before output mutation");
    assert!(error.to_string().contains("retired backfill"), "{error}");
    assert!(
        !output_dir.exists(),
        "retirement rejection must precede directory creation"
    );
}

#[test]
fn public_object_loader_rejects_retired_nested_object_before_filesystem_access() {
    let repo_root = repo_root_from_manifest_dir();
    let (run_spec, _) = read_run_spec_with_hash(
        &repo_root
            .join(REFERENCE_ROOT)
            .join(ACTIVE_GOLDEN_RUN_SPECS[0]),
    )
    .expect("active golden RunSpec loads");
    let retired_object = Path::new(
        "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02/object-staging/backfill-object-staging-manifest.json",
    );

    let error = read_accepted_object_for_run_spec(retired_object, &run_spec)
        .expect_err("retired object input must reject before absence");
    assert_retired_error_precedes_absence(error);
}

fn assert_retired_error_precedes_absence(error: impl std::fmt::Display) {
    let error = error.to_string();
    assert!(error.contains("retired backfill"), "{error}");
    assert!(
        !error.contains("No such file"),
        "retirement policy must reject before absence happens to reject: {error}"
    );
}

#[test]
fn runtime_classifier_does_not_claim_future_or_unrelated_paths() {
    for path in [
        "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-06-01/materialized-run-spec/backfill-run-spec.toml",
        "specs/023-nt-research-analytics-platform/reference/backfill-gates/bybit-bnbusdc-2026-06-02/materialized-run-spec/backfill-run-spec.toml",
        "specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-06-01.toml",
        "specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bybit-bnbusdc-2026-06-02.toml",
        "specs/023-nt-research-analytics-platform/reference/backfill-gates/okx-btcusdt-2026-03-02/materialized-run-spec/backfill-run-spec.toml",
        "specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02/new-runtime-control.toml",
        "specs/023-nt-research-analytics-platform/reference/backfill-conversion-batches/binance-bnbusdc-2026-03-01-2026-05-31/new-runtime-control.toml",
        "unrelated/backfill-gates/binance-bnbusdc-2026-03-02/materialized-run-spec/backfill-run-spec.toml",
        "unrelated/specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02/materialized-run-spec/backfill-run-spec.toml",
    ] {
        assert!(
            !is_retired_backfill_runtime_path(Path::new(path)),
            "non-inventory path {path:?} must remain available for future or unrelated work"
        );
    }
}

fn repo_references_to_retired_lane(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices("repo://").filter_map(|(start, _)| {
        let candidate = &text[start + "repo://".len()..];
        let end = candidate
            .find(|character: char| {
                character.is_ascii_whitespace()
                    || matches!(character, '"' | '\'' | ',' | ')' | ']' | '}')
            })
            .unwrap_or(candidate.len());
        let path = &candidate[..end];
        is_retired_backfill_runtime_path(Path::new(path)).then_some(path)
    })
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(path) = pending.pop() {
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("read repository entry").path();
            if path.is_dir() {
                let name = path.file_name().and_then(|name| name.to_str());
                if !matches!(name, Some(".git" | "target" | ".worktrees")) {
                    pending.push(path);
                }
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    files
}
