use std::{fs, process::Command};

use backtesting_vertical_slice::research_experiment::{
    CallerIdentity, EvidenceState, ExperimentError, load_and_validate_experiment, match_caller_role,
};

fn fixture_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/research/pump-research-synthetic.toml")
}

fn production_text() -> String {
    let mut text = fs::read_to_string(fixture_path())
        .expect("fixture")
        .replace("authority = \"test_fixture\"", "authority = \"aws_sts\"")
        .replace(
            "s3://example-bucket/pump-research-fixture",
            "s3://research-bucket/pump-research",
        )
        .replace(
            "[timestamp_policy.verifier]\nkind = \"test_fixture\"\nfixture_id = \"fixture-independent-timestamp\"",
            "[timestamp_policy.verifier]\nkind = \"registered\"\nregistry_key = \"independent-timestamp-registry\"",
        );
    for (fixture_account, fixture_arn, fixture_user, role_name, principal_id) in [
        (
            "fixture-account-ingestion",
            "fixture://ingestion/",
            "fixture-ingestion",
            "pump-ingestion",
            "AROA0000000000000001:",
        ),
        (
            "fixture-account-disclosure",
            "fixture://disclosure/",
            "fixture-disclosure",
            "pump-disclosure",
            "AROA0000000000000002:",
        ),
        (
            "fixture-account-canonical",
            "fixture://canonical/",
            "fixture-canonical",
            "pump-canonical",
            "AROA0000000000000003:",
        ),
        (
            "fixture-account-verification",
            "fixture://verification/",
            "fixture-verification",
            "pump-verification",
            "AROA0000000000000004:",
        ),
        (
            "fixture-account-custody",
            "fixture://custody/",
            "fixture-custody",
            "pump-custody",
            "AROA0000000000000005:",
        ),
        (
            "fixture-account-decision",
            "fixture://decision/",
            "fixture-decision",
            "pump-decision",
            "AROA0000000000000006:",
        ),
        (
            "fixture-account-governance",
            "fixture://governance/",
            "fixture-governance",
            "pump-governance",
            "AROA0000000000000007:",
        ),
    ] {
        text = text
            .replace(fixture_account, "123456789012")
            .replace(
                fixture_arn,
                &format!("arn:aws:sts::123456789012:assumed-role/{role_name}/"),
            )
            .replace(fixture_user, principal_id);
    }
    text
}

#[test]
fn non_test_loader_rejects_fixture_roles_without_external_access() {
    let error = load_and_validate_experiment(&fixture_path()).unwrap_err();
    assert!(error.to_string().contains("aws_sts authority"), "{error}");
}

#[test]
fn non_test_loader_rejects_typed_fixture_timestamp_verifier() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("fixture-verifier.toml");
    let text = production_text().replace(
        "[timestamp_policy.verifier]\nkind = \"registered\"\nregistry_key = \"independent-timestamp-registry\"",
        "[timestamp_policy.verifier]\nkind = \"test_fixture\"\nfixture_id = \"fixture-independent-timestamp\"",
    );
    fs::write(&path, text).expect("write");
    let error = load_and_validate_experiment(&path).unwrap_err();
    assert!(
        error.to_string().contains("fixture timestamp verifier"),
        "{error}"
    );
}

#[test]
fn strict_toml_rejects_unknown_top_level_field() {
    let original = fs::read_to_string(fixture_path()).expect("fixture");
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("unknown.toml");
    fs::write(
        &path,
        format!("{original}\n[unknown_table]\nvalue = true\n"),
    )
    .expect("write");
    assert!(matches!(
        load_and_validate_experiment(&path).unwrap_err(),
        ExperimentError::Parse(_)
    ));
}

#[test]
fn definition_cannot_self_assert_a_derived_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("advanced-state.toml");
    fs::write(
        &path,
        production_text().replace("state = \"draft\"", "state = \"genesis_committed\""),
    )
    .expect("write");
    let error = load_and_validate_experiment(&path).unwrap_err();
    assert!(error.to_string().contains("starts in draft"), "{error}");
}

#[test]
fn role_separation_is_symmetric_for_semantic_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let first_path = temp.path().join("first.toml");
    let second_path = temp.path().join("second.toml");
    let first = production_text();
    let second = first.replace(
        "left_role = \"custody\"\nright_role = \"experiment-decision\"",
        "left_role = \"experiment-decision\"\nright_role = \"custody\"",
    );
    fs::write(&first_path, first).expect("write first");
    fs::write(&second_path, second).expect("write second");
    let first = load_and_validate_experiment(&first_path).expect("first");
    let second = load_and_validate_experiment(&second_path).expect("second");
    assert_eq!(first.semantic_hash, second.semantic_hash);
}

#[test]
fn duplicate_symmetric_role_separation_fails_closed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("duplicate-separation.toml");
    let duplicate = "\n[[roles.required_separations]]\nleft_role = \"experiment-decision\"\nright_role = \"custody\"\n";
    let text = production_text().replace("\n[storage]\n", &format!("{duplicate}\n[storage]\n"));
    fs::write(&path, text).expect("write");
    let error = load_and_validate_experiment(&path).unwrap_err();
    assert!(error.to_string().contains("duplicate role pair"), "{error}");
}

#[test]
fn append_role_must_match_the_registration_credential_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("scope-mismatch.toml");
    let text = production_text().replacen(
        "credential_scope_ref = \"fixture-scope-decision\"",
        "credential_scope_ref = \"different-scope\"",
        1,
    );
    fs::write(&path, text).expect("write");
    let error = load_and_validate_experiment(&path).unwrap_err();
    assert!(error.to_string().contains("credential scope"), "{error}");
}

#[test]
fn separated_roles_cannot_share_a_credential_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("shared-scope.toml");
    let text = production_text().replace(
        "credential_scope_ref = \"fixture-scope-disclosure\"",
        "credential_scope_ref = \"fixture-scope-ingestion\"",
    );
    fs::write(&path, text).expect("write");
    let error = load_and_validate_experiment(&path).unwrap_err();
    assert!(
        error.to_string().contains("share a credential scope"),
        "{error}"
    );
}

#[test]
fn decimal_thresholds_have_one_semantic_encoding_and_invalid_values_fail() {
    let temp = tempfile::tempdir().expect("tempdir");
    let first_path = temp.path().join("first.toml");
    let equivalent_path = temp.path().join("equivalent.toml");
    let invalid_path = temp.path().join("invalid.toml");
    let first = production_text();
    fs::write(&first_path, &first).expect("write first");
    fs::write(
        &equivalent_path,
        first.replace(
            "abnormal_return_threshold = \"3.0\"",
            "abnormal_return_threshold = \"3.00\"",
        ),
    )
    .expect("write equivalent");
    fs::write(
        &invalid_path,
        first.replace(
            "giveback_threshold = \"0.5\"",
            "giveback_threshold = \"not-a-decimal\"",
        ),
    )
    .expect("write invalid");
    let first = load_and_validate_experiment(&first_path).expect("first");
    let equivalent = load_and_validate_experiment(&equivalent_path).expect("equivalent");
    assert_eq!(first.semantic_hash, equivalent.semantic_hash);
    assert!(load_and_validate_experiment(&invalid_path).is_err());
}

#[test]
fn primary_cell_must_resolve_to_the_detector_grid() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("unknown-primary-cell.toml");
    fs::write(
        &path,
        production_text().replace(
            "[confirmation.primary_cell]\nkind = \"trigger_cell\"\ntrigger_cell_id = \"synthetic-primary-cell\"",
            "[confirmation.primary_cell]\nkind = \"trigger_cell\"\ntrigger_cell_id = \"unknown-cell\"",
        ),
    )
    .expect("write");
    let error = load_and_validate_experiment(&path).unwrap_err();
    assert!(error.to_string().contains("unknown detector"), "{error}");
}

#[test]
fn ordered_diagnostics_and_release_schedule_reject_blank_or_duplicate_values() {
    let temp = tempfile::tempdir().expect("tempdir");
    for (name, text, expected_field) in [
        (
            "blank-diagnostic",
            production_text().replace(
                "ordered_diagnostics = [\"coverage\", \"balance\", \"attrition\"]",
                "ordered_diagnostics = [\"coverage\", \"\", \"attrition\"]",
            ),
            "analysis.ordered_diagnostics",
        ),
        (
            "duplicate-diagnostic",
            production_text().replace(
                "ordered_diagnostics = [\"coverage\", \"balance\", \"attrition\"]",
                "ordered_diagnostics = [\"coverage\", \"coverage\", \"attrition\"]",
            ),
            "analysis.ordered_diagnostics",
        ),
        (
            "blank-release",
            production_text().replace(
                "release_schedule = [\"after-discovery-commitment\"]",
                "release_schedule = [\"\"]",
            ),
            "disclosure.release_schedule",
        ),
        (
            "duplicate-release",
            production_text()
                .replace(
                    "release_schedule = [\"after-discovery-commitment\"]",
                    "release_schedule = [\"after-discovery-commitment\", \"after-discovery-commitment\"]",
                )
                .replace("maximum_release_count = 1", "maximum_release_count = 2"),
            "disclosure.release_schedule",
        ),
    ] {
        let path = temp.path().join(format!("{name}.toml"));
        fs::write(&path, text).expect("write invalid fixture");
        let error = load_and_validate_experiment(&path).unwrap_err();
        assert!(error.to_string().contains(expected_field), "{error}");
    }
}

#[test]
fn unmatched_authenticated_identity_is_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("production.toml");
    fs::write(&path, production_text()).expect("write");
    let experiment = load_and_validate_experiment(&path).expect("production definition");
    let error = match_caller_role(
        &experiment.definition,
        &CallerIdentity {
            account_id: "unregistered".to_string(),
            arn: "arn:aws:sts::unregistered:assumed-role/unregistered/session".to_string(),
            user_id: "unregistered".to_string(),
        },
    )
    .unwrap_err();
    assert!(matches!(error, ExperimentError::UnauthorizedPrincipal));
}

#[test]
fn sts_role_match_requires_bounded_role_and_session_components() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("production.toml");
    fs::write(&path, production_text()).expect("write");
    let experiment = load_and_validate_experiment(&path).expect("production definition");
    let role = experiment
        .definition
        .roles
        .bindings
        .iter()
        .find(|role| role.can_append_versions)
        .expect("append role");
    let exact = CallerIdentity {
        account_id: role.account_id.clone(),
        arn: format!("{}session", role.principal_arn_prefix),
        user_id: format!("{}session", role.user_id_prefix),
    };
    assert_eq!(
        match_caller_role(&experiment.definition, &exact)
            .expect("bounded identity")
            .role_id,
        role.role_id
    );

    let confused = CallerIdentity {
        account_id: role.account_id.clone(),
        arn: format!(
            "{}-other/session",
            role.principal_arn_prefix.trim_end_matches('/')
        ),
        user_id: format!("{}session", role.user_id_prefix),
    };
    assert!(matches!(
        match_caller_role(&experiment.definition, &confused),
        Err(ExperimentError::UnauthorizedPrincipal)
    ));
}

#[test]
fn malformed_lineage_and_missing_required_value_fail_closed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let malformed = temp.path().join("malformed.toml");
    fs::write(
        &malformed,
        production_text().replace(
            "1111111111111111111111111111111111111111111111111111111111111111",
            "not-a-hash",
        ),
    )
    .expect("write malformed");
    assert!(load_and_validate_experiment(&malformed).is_err());

    let missing = temp.path().join("missing.toml");
    fs::write(
        &missing,
        production_text().replace(
            "detector_id = \"synthetic-pump-giveback\"",
            "detector_id = \"\"",
        ),
    )
    .expect("write missing");
    assert!(load_and_validate_experiment(&missing).is_err());
}

#[test]
fn semantic_hash_ignores_set_order_and_recorded_creation_time() {
    let temp = tempfile::tempdir().expect("tempdir");
    let first_path = temp.path().join("first.toml");
    let second_path = temp.path().join("second.toml");
    let first = production_text();
    let second = first
        .replace(
            "venue_keys = [\"synthetic-venue-a\", \"synthetic-venue-b\"]",
            "venue_keys = [\"synthetic-venue-b\", \"synthetic-venue-a\"]",
        )
        .replace(
            "created_at = \"2026-08-20T00:00:00Z\"",
            "created_at = \"2026-08-20T00:00:01Z\"",
        );
    fs::write(&first_path, first).expect("write first");
    fs::write(&second_path, second).expect("write second");
    let first = load_and_validate_experiment(&first_path).expect("first");
    let second = load_and_validate_experiment(&second_path).expect("second");
    assert_ne!(first.original_hash, second.original_hash);
    assert_eq!(first.semantic_hash, second.semantic_hash);
    assert_eq!(
        first.canonical_semantic_bytes,
        second.canonical_semantic_bytes
    );
}

#[test]
fn terminal_evidence_state_rejects_promotion() {
    assert!(
        EvidenceState::Expired
            .validate_transition(EvidenceState::Active)
            .is_err()
    );
}

#[test]
fn cli_has_no_provider_or_alternate_replay_surface() {
    let binary = env!("CARGO_BIN_EXE_pump_research");
    let help = Command::new(binary).arg("--help").output().expect("help");
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("UTF-8");
    assert!(help.contains("validate") && help.contains("register-version"));
    for forbidden in ["provider-login", "quote", "purchase", "query", "replay"] {
        assert!(!help.contains(forbidden), "unexpected command {forbidden}");
    }
    let rejected = Command::new(binary)
        .arg("provider-query")
        .output()
        .expect("rejected command");
    assert!(!rejected.status.success());
    let rejected: serde_json::Value =
        serde_json::from_slice(&rejected.stderr).expect("structured command failure");
    assert_eq!(rejected["status"], "error");
    assert_eq!(rejected["reason_code"], "invalid_command");

    let invalid_definition = Command::new(binary)
        .args(["validate", "--spec"])
        .arg(fixture_path())
        .output()
        .expect("validation failure");
    assert!(!invalid_definition.status.success());
    let invalid_definition: serde_json::Value =
        serde_json::from_slice(&invalid_definition.stderr).expect("structured validation failure");
    assert_eq!(invalid_definition["status"], "error");
    assert_eq!(invalid_definition["reason_code"], "validation_failed");
}
