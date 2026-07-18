use std::{
    env,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::source_universe_durable_tracer::{
        SourceUniverseDurableTracerAggregateLimits, SourceUniverseDurableTracerArtifactPin,
        SourceUniverseDurableTracerCheckoutPolicy, SourceUniverseDurableTracerPackLimits,
        SourceUniverseDurableTracerGitExecutable, SourceUniverseDurableTracerRunPolicy,
        build_source_universe_durable_tracer_receipt_set,
        preflight_source_universe_durable_tracer_registry,
        read_and_validate_source_universe_durable_tracer_receipt_set,
        run_source_universe_durable_tracer_registry,
        verify_source_universe_durable_tracer_checkout,
        write_source_universe_durable_tracer_receipt_set,
};

const SOURCE_REVISION_ENV: &str = "BOLT_RA001A_SOURCE_REVISION";
const AWS_ROLE_ARN_ENV: &str = "BOLT_RA001A_AWS_ROLE_ARN";
const AWS_REGION_ENV: &str = "BOLT_RA001A_AWS_REGION";
const GIT_EXECUTABLE_ENV: &str = "BOLT_RA001A_GIT_EXECUTABLE";
const GIT_SHA256_ENV: &str = "BOLT_RA001A_GIT_SHA256";
const GIT_BYTES_ENV: &str = "BOLT_RA001A_GIT_BYTES";
const WORKER_SHA256_ENV: &str = "BOLT_RA001A_WORKER_SHA256";
const WORKER_BYTES_ENV: &str = "BOLT_RA001A_WORKER_BYTES";
const RECEIPT_PATH_ENV: &str = "BOLT_RA001A_RECEIPT_PATH";
const MAX_REGISTRY_PACKS_ENV: &str = "BOLT_RA001A_MAX_REGISTRY_PACKS";
const MAX_TOTAL_SELECTED_OBJECT_BYTES_ENV: &str = "BOLT_RA001A_MAX_TOTAL_SELECTED_OBJECT_BYTES";
const MAX_WORKER_EXECUTABLE_BYTES_ENV: &str = "BOLT_RA001A_MAX_WORKER_EXECUTABLE_BYTES";
const MAX_GIT_EXECUTABLE_BYTES_ENV: &str = "BOLT_RA001A_MAX_GIT_EXECUTABLE_BYTES";
const MAX_FETCH_TIMEOUT_SECONDS_ENV: &str = "BOLT_RA001A_MAX_FETCH_TIMEOUT_SECONDS";
const MAX_CONCURRENT_RECORDS_ENV: &str = "BOLT_RA001A_MAX_CONCURRENT_RECORDS";
const MAX_WORKER_VIRTUAL_MEMORY_BYTES_ENV: &str = "BOLT_RA001A_MAX_WORKER_VIRTUAL_MEMORY_BYTES";
const MIN_WORKER_RESERVED_OVERHEAD_BYTES_ENV: &str =
    "BOLT_RA001A_MIN_WORKER_RESERVED_OVERHEAD_BYTES";
const MAX_WORKER_TERMINATION_GRACE_SECONDS_ENV: &str =
    "BOLT_RA001A_MAX_WORKER_TERMINATION_GRACE_SECONDS";
const MAX_DECODED_BYTES_ENV: &str = "BOLT_RA001A_MAX_DECODED_BYTES";
const MAX_SOURCE_ROWS_ENV: &str = "BOLT_RA001A_MAX_SOURCE_ROWS";
const MAX_PROJECTED_ROW_GROUPS_ENV: &str = "BOLT_RA001A_MAX_PROJECTED_ROW_GROUPS";
const MAX_OPERATOR_WALL_SECONDS_ENV: &str = "BOLT_RA001A_MAX_OPERATOR_WALL_SECONDS";
const MAX_TERMINAL_COMMIT_TIMEOUT_SECONDS_ENV: &str =
    "BOLT_RA001A_MAX_TERMINAL_COMMIT_TIMEOUT_SECONDS";
const MAX_LAUNCH_ARTIFACT_BYTES_ENV: &str = "BOLT_RA001A_MAX_LAUNCH_ARTIFACT_BYTES";
const MAX_CONTROL_ARTIFACT_BYTES_ENV: &str = "BOLT_RA001A_MAX_CONTROL_ARTIFACT_BYTES";
const MAX_RETAINED_CONTROL_INPUT_BYTES_ENV: &str = "BOLT_RA001A_MAX_RETAINED_CONTROL_INPUT_BYTES";
const MAX_FINAL_OBJECT_BYTES_ENV: &str = "BOLT_RA001A_MAX_FINAL_OBJECT_BYTES";
const MAX_WORKSPACE_BYTES_ENV: &str = "BOLT_RA001A_MAX_WORKSPACE_BYTES";
const MAX_CACHE_BYTES_ENV: &str = "BOLT_RA001A_MAX_CACHE_BYTES";
const MIN_FREE_SPACE_RESERVE_BYTES_ENV: &str = "BOLT_RA001A_MIN_FREE_SPACE_RESERVE_BYTES";
const MIN_ONE_RECORD_WORST_CASE_BYTES_ENV: &str = "BOLT_RA001A_MIN_ONE_RECORD_WORST_CASE_BYTES";
const CACHE_RETENTION_AGE_SECONDS_ENV: &str = "BOLT_RA001A_CACHE_RETENTION_AGE_SECONDS";
const CANDIDATE_RETENTION_AGE_SECONDS_ENV: &str = "BOLT_RA001A_CANDIDATE_RETENTION_AGE_SECONDS";
const MAX_LIFECYCLE_CLEANUP_ENTRIES_ENV: &str = "BOLT_RA001A_MAX_LIFECYCLE_CLEANUP_ENTRIES";
const MAX_LIFECYCLE_CLEANUP_DEPTH_ENV: &str = "BOLT_RA001A_MAX_LIFECYCLE_CLEANUP_DEPTH";
const TRUSTED_POLICY_OUTPUT_SHA256_ENV: &str = "BOLT_RA001A_TRUSTED_POLICY_OUTPUT_SHA256";
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

fn required_pack_limits() -> SourceUniverseDurableTracerPackLimits {
    SourceUniverseDurableTracerPackLimits {
        max_concurrent_records: required_positive_u64_env(MAX_CONCURRENT_RECORDS_ENV),
        max_worker_virtual_memory_bytes: required_positive_u64_env(
            MAX_WORKER_VIRTUAL_MEMORY_BYTES_ENV,
        ),
        min_worker_reserved_overhead_bytes: required_positive_u64_env(
            MIN_WORKER_RESERVED_OVERHEAD_BYTES_ENV,
        ),
        max_fetch_timeout_seconds: required_positive_u64_env(MAX_FETCH_TIMEOUT_SECONDS_ENV),
        max_worker_termination_grace_seconds: required_positive_u64_env(
            MAX_WORKER_TERMINATION_GRACE_SECONDS_ENV,
        ),
        max_decoded_bytes: required_positive_u64_env(MAX_DECODED_BYTES_ENV),
        max_launch_artifact_bytes: required_positive_u64_env(MAX_LAUNCH_ARTIFACT_BYTES_ENV),
        max_control_artifact_bytes: required_positive_u64_env(MAX_CONTROL_ARTIFACT_BYTES_ENV),
        max_retained_control_input_bytes: required_positive_u64_env(
            MAX_RETAINED_CONTROL_INPUT_BYTES_ENV,
        ),
        max_final_object_bytes: required_positive_u64_env(MAX_FINAL_OBJECT_BYTES_ENV),
        max_workspace_bytes: required_positive_u64_env(MAX_WORKSPACE_BYTES_ENV),
        max_cache_bytes: required_positive_u64_env(MAX_CACHE_BYTES_ENV),
        min_free_space_reserve_bytes: required_positive_u64_env(MIN_FREE_SPACE_RESERVE_BYTES_ENV),
        min_one_record_worst_case_bytes: required_positive_u64_env(
            MIN_ONE_RECORD_WORST_CASE_BYTES_ENV,
        ),
        cache_retention_age_seconds: required_positive_u64_env(CACHE_RETENTION_AGE_SECONDS_ENV),
        candidate_retention_age_seconds: required_positive_u64_env(
            CANDIDATE_RETENTION_AGE_SECONDS_ENV,
        ),
        max_lifecycle_cleanup_entries: required_positive_u64_env(MAX_LIFECYCLE_CLEANUP_ENTRIES_ENV),
        max_lifecycle_cleanup_depth: required_positive_u64_env(MAX_LIFECYCLE_CLEANUP_DEPTH_ENV),
        max_source_rows: required_positive_u64_env(MAX_SOURCE_ROWS_ENV),
        max_projected_row_groups: required_positive_u64_env(MAX_PROJECTED_ROW_GROUPS_ENV),
        max_operator_wall_seconds: required_positive_u64_env(MAX_OPERATOR_WALL_SECONDS_ENV),
        max_terminal_commit_timeout_seconds: required_positive_u64_env(
            MAX_TERMINAL_COMMIT_TIMEOUT_SECONDS_ENV,
        ),
    }
}

fn required_run_policy(max_git_executable_bytes: u64) -> SourceUniverseDurableTracerRunPolicy {
    SourceUniverseDurableTracerRunPolicy {
        aggregate_limits: SourceUniverseDurableTracerAggregateLimits {
            max_registry_packs: required_positive_u64_env(MAX_REGISTRY_PACKS_ENV),
            max_total_selected_object_bytes: required_positive_u64_env(
                MAX_TOTAL_SELECTED_OBJECT_BYTES_ENV,
            ),
        },
        pack_limits: required_pack_limits(),
        aws_role_arn: required_utf8_env(AWS_ROLE_ARN_ENV),
        aws_region: required_utf8_env(AWS_REGION_ENV),
        max_git_executable_bytes,
        max_worker_executable_bytes: required_positive_u64_env(MAX_WORKER_EXECUTABLE_BYTES_ENV),
        trusted_policy_output_sha256: required_utf8_env(TRUSTED_POLICY_OUTPUT_SHA256_ENV),
    }
}

#[test]
#[ignore = "workflow-only exact-commit admission before AWS credential configuration"]
fn registry_complete_ra001a_preflight_runs_before_aws_credentials() {
    let source_revision = required_utf8_env(SOURCE_REVISION_ENV);
    let expected_git = SourceUniverseDurableTracerArtifactPin {
        bytes: required_positive_u64_env(GIT_BYTES_ENV),
        sha256: required_utf8_env(GIT_SHA256_ENV),
    };
    let max_git_executable_bytes = required_positive_u64_env(MAX_GIT_EXECUTABLE_BYTES_ENV);
    let git_executable = SourceUniverseDurableTracerGitExecutable::capture(
        &required_absolute_path_env(GIT_EXECUTABLE_ENV),
        &expected_git,
        max_git_executable_bytes,
    )
    .expect("capture the reviewed Git capability for pre-credential admission");
    let aggregate = preflight_source_universe_durable_tracer_registry(
        &repo_root(),
        &source_revision,
        &git_executable,
        &required_run_policy(max_git_executable_bytes),
    )
    .expect("complete exact-commit admission before AWS credential configuration");
    assert!(aggregate.registry_packs > 0);
    assert_eq!(aggregate.registry_packs, aggregate.total_selected_records);
}

#[test]
#[ignore = "requires the protected RA-001a AWS role and creates exact-version durable objects"]
fn registry_complete_ra001a_live_tracer_runs_every_committed_pack() {
    let source_revision = required_utf8_env(SOURCE_REVISION_ENV);
    let expected_git = SourceUniverseDurableTracerArtifactPin {
        bytes: required_positive_u64_env(GIT_BYTES_ENV),
        sha256: required_utf8_env(GIT_SHA256_ENV),
    };
    let max_git_executable_bytes = required_positive_u64_env(MAX_GIT_EXECUTABLE_BYTES_ENV);
    let git_executable = SourceUniverseDurableTracerGitExecutable::capture(
        &required_absolute_path_env(GIT_EXECUTABLE_ENV),
        &expected_git,
        max_git_executable_bytes,
    )
    .expect("capture the one reviewed Git execution capability");
    let expected_worker = SourceUniverseDurableTracerArtifactPin {
        bytes: required_positive_u64_env(WORKER_BYTES_ENV),
        sha256: required_utf8_env(WORKER_SHA256_ENV),
    };
    let receipt_path = required_absolute_path_env(RECEIPT_PATH_ENV);
    let repo_root = repo_root();
    let checkout_policy = required_checkout_policy();
    verify_source_universe_durable_tracer_checkout(
        &repo_root,
        &source_revision,
        &git_executable,
        &checkout_policy,
    )
    .expect("bind RA-001a proof to exact clean checkout before pack execution");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_source_universe_batch_execution"));
    let run_policy = required_run_policy(max_git_executable_bytes);

    let registry_run = run_source_universe_durable_tracer_registry(
        &repo_root,
        &source_revision,
        &git_executable,
        &binary,
        &expected_worker,
        run_policy.clone(),
    )
    .expect("admit and execute registry-complete RA-001a aggregate cost envelope");
    eprintln!(
        "RA-001a aggregate preflight: registry_packs={} selected_records={} selected_object_bytes={}",
        registry_run.aggregate.registry_packs,
        registry_run.aggregate.total_selected_records,
        registry_run.aggregate.total_selected_object_bytes
    );

    verify_source_universe_durable_tracer_checkout(
        &repo_root,
        &source_revision,
        &git_executable,
        &checkout_policy,
    )
    .expect("revalidate exact clean checkout before receipt generation");
    let receipt_set = build_source_universe_durable_tracer_receipt_set(
        &repo_root,
        &source_revision,
        &git_executable,
        &expected_worker,
        &registry_run,
    )
    .expect("build registry-complete RA-001a durable tracer receipt set");
    let artifact = write_source_universe_durable_tracer_receipt_set(
        &receipt_path,
        &repo_root,
        &source_revision,
        &git_executable,
        &expected_worker,
        &run_policy,
        &receipt_set,
    )
    .expect("publish create-only RA-001a durable tracer receipt set");
    let reparsed = read_and_validate_source_universe_durable_tracer_receipt_set(
        &repo_root,
        &source_revision,
        &git_executable,
        &expected_worker,
        &run_policy,
        &artifact,
    )
    .expect("reopen and validate exact RA-001a durable tracer receipt set");
    assert_eq!(reparsed, receipt_set);
}
