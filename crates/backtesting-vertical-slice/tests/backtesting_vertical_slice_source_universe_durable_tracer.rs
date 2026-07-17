use std::{
    env,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::{
    operator_work_budget::OperatorWorkBudgetGuard,
    source_universe_durable_tracer::{
        SourceUniverseDurableTracerAggregateLimits, SourceUniverseDurableTracerCheckoutPolicy,
        build_source_universe_durable_tracer_receipt_set,
        read_and_validate_source_universe_durable_tracer_receipt_set,
        run_source_universe_durable_tracer_registry,
        verify_source_universe_durable_tracer_checkout,
        write_source_universe_durable_tracer_receipt_set,
    },
};

const SOURCE_REVISION_ENV: &str = "BOLT_RA001A_SOURCE_REVISION";
const WORKER_SHA256_ENV: &str = "BOLT_RA001A_WORKER_SHA256";
const RECEIPT_PATH_ENV: &str = "BOLT_RA001A_RECEIPT_PATH";
const MAX_REGISTRY_PACKS_ENV: &str = "BOLT_RA001A_MAX_REGISTRY_PACKS";
const MAX_TOTAL_SELECTED_OBJECT_BYTES_ENV: &str = "BOLT_RA001A_MAX_TOTAL_SELECTED_OBJECT_BYTES";
const MAX_WORKER_EXECUTABLE_BYTES_ENV: &str = "BOLT_RA001A_MAX_WORKER_EXECUTABLE_BYTES";
const ALLOWED_IGNORED_RUNTIME_ROOTS_ENV: &str = "BOLT_RA001A_ALLOWED_IGNORED_RUNTIME_ROOTS";
const MAX_IGNORED_ENTRY_BYTES_ENV: &str = "BOLT_RA001A_MAX_IGNORED_ENTRY_BYTES";
const MAX_IGNORED_ENTRIES_ENV: &str = "BOLT_RA001A_MAX_IGNORED_ENTRIES";

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

fn required_checkout_policy() -> SourceUniverseDurableTracerCheckoutPolicy {
    let roots = required_utf8_env(ALLOWED_IGNORED_RUNTIME_ROOTS_ENV)
        .split(',')
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(
        roots.iter().all(|root| !root.is_empty()),
        "required {ALLOWED_IGNORED_RUNTIME_ROOTS_ENV} must not contain empty roots"
    );
    SourceUniverseDurableTracerCheckoutPolicy {
        allowed_ignored_runtime_roots: roots,
        max_ignored_entry_bytes: required_positive_u64_env(MAX_IGNORED_ENTRY_BYTES_ENV),
        max_ignored_entries: required_positive_u64_env(MAX_IGNORED_ENTRIES_ENV),
    }
}

#[test]
#[ignore = "requires the protected RA-001a AWS role and creates exact-version durable objects"]
fn registry_complete_ra001a_live_tracer_runs_every_committed_pack() {
    let source_revision = required_utf8_env(SOURCE_REVISION_ENV);
    let expected_worker_sha256 = required_utf8_env(WORKER_SHA256_ENV);
    let receipt_path = required_absolute_path_env(RECEIPT_PATH_ENV);
    let repo_root = repo_root();
    let checkout_policy = required_checkout_policy();
    verify_source_universe_durable_tracer_checkout(&repo_root, &source_revision, &checkout_policy)
        .expect("bind RA-001a proof to exact clean checkout before pack execution");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_source_universe_batch_execution"));

    let registry_run = run_source_universe_durable_tracer_registry(
        &repo_root,
        &source_revision,
        &binary,
        &expected_worker_sha256,
        required_positive_u64_env(MAX_WORKER_EXECUTABLE_BYTES_ENV),
        SourceUniverseDurableTracerAggregateLimits {
            max_registry_packs: required_positive_u64_env(MAX_REGISTRY_PACKS_ENV),
            max_total_selected_object_bytes: required_positive_u64_env(
                MAX_TOTAL_SELECTED_OBJECT_BYTES_ENV,
            ),
        },
    )
    .expect("admit and execute registry-complete RA-001a aggregate cost envelope");
    eprintln!(
        "RA-001a aggregate preflight: registry_packs={} selected_records={} selected_object_bytes={}",
        registry_run.aggregate.registry_packs,
        registry_run.aggregate.total_selected_records,
        registry_run.aggregate.total_selected_object_bytes
    );

    verify_source_universe_durable_tracer_checkout(&repo_root, &source_revision, &checkout_policy)
        .expect("revalidate exact clean checkout before receipt generation");
    let receipt_set = build_source_universe_durable_tracer_receipt_set(
        &repo_root,
        &source_revision,
        &expected_worker_sha256,
        &registry_run,
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
