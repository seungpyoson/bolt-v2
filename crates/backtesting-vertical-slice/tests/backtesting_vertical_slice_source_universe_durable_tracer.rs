use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use backtesting_vertical_slice::{
    operator_work_budget::OperatorWorkBudgetGuard,
    path_resolution::resolve_output_dir,
    source_universe_batch_execution::SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE,
    source_universe_batch_launch::discover_committed_source_universe_execution_packs,
    source_universe_durable_tracer::{
        SourceUniverseDurableTracerAggregateLimits, SourceUniverseDurableTracerReportInput,
        build_source_universe_durable_tracer_receipt_set,
        read_and_validate_source_universe_durable_tracer_receipt_set,
        validate_source_universe_durable_tracer_aggregate_limits,
        verify_source_universe_durable_tracer_checkout,
        write_source_universe_durable_tracer_receipt_set,
    },
};

const SOURCE_REVISION_ENV: &str = "BOLT_RA001A_SOURCE_REVISION";
const WORKER_SHA256_ENV: &str = "BOLT_RA001A_WORKER_SHA256";
const RECEIPT_PATH_ENV: &str = "BOLT_RA001A_RECEIPT_PATH";
const MAX_REGISTRY_PACKS_ENV: &str = "BOLT_RA001A_MAX_REGISTRY_PACKS";
const MAX_TOTAL_SELECTED_OBJECT_BYTES_ENV: &str = "BOLT_RA001A_MAX_TOTAL_SELECTED_OBJECT_BYTES";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is nested below repository root")
        .to_path_buf()
}

fn required_utf8_env(name: &str) -> String {
    let value =
        env::var(name).unwrap_or_else(|_| panic!("required {name} is missing or non-UTF-8"));
    assert!(!value.is_empty(), "required {name} must not be empty");
    value
}

fn required_absolute_path_env(name: &str) -> PathBuf {
    let path =
        PathBuf::from(env::var_os(name).unwrap_or_else(|| panic!("required {name} is missing")));
    assert!(
        path.is_absolute(),
        "required {name} must be an absolute path"
    );
    let parent = path
        .parent()
        .unwrap_or_else(|| panic!("required {name} has no parent directory"));
    assert!(
        parent.is_dir(),
        "required {name} parent must already be a directory"
    );
    path
}

fn required_positive_u64_env(name: &str) -> u64 {
    let raw = required_utf8_env(name);
    let value = raw
        .parse::<u64>()
        .unwrap_or_else(|error| panic!("required {name} must be an unsigned integer: {error}"));
    assert!(value > 0, "required {name} must be positive");
    value
}

#[test]
#[ignore = "requires the protected RA-001a AWS role and creates exact-version durable objects"]
fn registry_complete_ra001a_live_tracer_runs_every_committed_pack() {
    let source_revision = required_utf8_env(SOURCE_REVISION_ENV);
    let expected_worker_sha256 = required_utf8_env(WORKER_SHA256_ENV);
    let receipt_path = required_absolute_path_env(RECEIPT_PATH_ENV);
    let repo_root = repo_root();
    verify_source_universe_durable_tracer_checkout(&repo_root, &source_revision)
        .expect("bind RA-001a proof to exact clean checkout before pack execution");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_source_universe_batch_execution"));
    assert!(
        binary.is_absolute() && binary.is_file(),
        "Cargo-provided source-universe batch executable must be one absolute file"
    );

    let committed = discover_committed_source_universe_execution_packs(&repo_root)
        .expect("discover complete committed execution-pack registry");
    let aggregate = validate_source_universe_durable_tracer_aggregate_limits(
        &committed,
        SourceUniverseDurableTracerAggregateLimits {
            max_registry_packs: required_positive_u64_env(MAX_REGISTRY_PACKS_ENV),
            max_total_selected_object_bytes: required_positive_u64_env(
                MAX_TOTAL_SELECTED_OBJECT_BYTES_ENV,
            ),
        },
    )
    .expect("preflight registry-complete RA-001a aggregate cost envelope");
    eprintln!(
        "RA-001a aggregate preflight: registry_packs={} selected_object_bytes={}",
        aggregate.registry_packs, aggregate.total_selected_object_bytes
    );
    let mut report_inputs = Vec::with_capacity(committed.len());
    for pack in &committed {
        let status = Command::new(&binary)
            .arg("--spec")
            .arg(&pack.launch_path)
            .current_dir(&repo_root)
            .status()
            .unwrap_or_else(|error| {
                panic!(
                    "start source-universe batch process for committed pack {}: {error}",
                    pack.pack_id
                )
            });
        assert!(
            status.success(),
            "source-universe batch process failed for committed pack {} with {}; stdout/stderr were streamed to the workflow log",
            pack.pack_id,
            status,
        );

        let launch_parent = pack
            .launch_path
            .parent()
            .expect("committed launch path has a parent");
        let declared_output = resolve_output_dir(launch_parent, &pack.launch_spec.output_dir);
        let canonical_output = declared_output.canonicalize().unwrap_or_else(|error| {
            panic!(
                "canonicalize completed output for committed pack {} at {}: {error}",
                pack.pack_id,
                declared_output.display()
            )
        });
        let report_path = canonical_output.join(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE);
        assert!(
            report_path.is_file(),
            "completed report is absent for committed pack {} at {}",
            pack.pack_id,
            report_path.display()
        );
        report_inputs.push(SourceUniverseDurableTracerReportInput {
            pack_id: pack.pack_id.clone(),
            report_path,
        });
    }

    verify_source_universe_durable_tracer_checkout(&repo_root, &source_revision)
        .expect("revalidate exact clean checkout before receipt generation");
    let receipt_set = build_source_universe_durable_tracer_receipt_set(
        &repo_root,
        &source_revision,
        &expected_worker_sha256,
        &report_inputs,
    )
    .expect("build registry-complete RA-001a durable tracer receipt set");
    let artifact = write_source_universe_durable_tracer_receipt_set(
        &receipt_path,
        &repo_root,
        &source_revision,
        &expected_worker_sha256,
        &receipt_set,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .expect("publish create-only RA-001a durable tracer receipt set");
    let reparsed = read_and_validate_source_universe_durable_tracer_receipt_set(
        &repo_root,
        &source_revision,
        &expected_worker_sha256,
        &artifact,
    )
    .expect("reopen and validate exact RA-001a durable tracer receipt set");
    assert_eq!(reparsed, receipt_set);
}
