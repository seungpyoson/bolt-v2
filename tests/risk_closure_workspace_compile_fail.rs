use std::{
    fs,
    process::{Command, Output},
};

fn compile_snippet(case_name: &str, body: &str) -> Output {
    let temporary = tempfile::tempdir().unwrap();
    let source_path = temporary.path().join(format!("{case_name}.rs"));
    let output_path = temporary.path().join(case_name);
    let module_path = format!(
        "{}/src/bolt_v3_risk_closure_workspace.rs",
        env!("CARGO_MANIFEST_DIR")
    );
    let source = format!(
        "#[path = {module_path:?}]\nmod workspace;\nuse workspace::*;\nfn main() {{\n{body}\n}}\n"
    );
    fs::write(&source_path, source).unwrap();

    Command::new("rustc")
        .args([
            "--edition=2024",
            "--crate-name",
            case_name,
            "--emit=metadata",
            "--error-format=json",
            "-o",
        ])
        .arg(&output_path)
        .arg(&source_path)
        .output()
        .unwrap()
}

fn compile_ledger_snippet(case_name: &str, body: &str) -> Output {
    let temporary = tempfile::tempdir().unwrap();
    let source_path = temporary.path().join(format!("{case_name}.rs"));
    let output_path = temporary.path().join(case_name);
    let module_path = format!(
        "{}/src/bolt_v3_application_resource_ledger.rs",
        env!("CARGO_MANIFEST_DIR")
    );
    let source = format!(
        "#[path = {module_path:?}]\nmod application_resource_ledger;\n\
         use application_resource_ledger::*;\nfn main() {{\n{body}\n}}\n"
    );
    fs::write(&source_path, source).unwrap();

    Command::new("rustc")
        .args([
            "--edition=2024",
            "--crate-name",
            case_name,
            "--emit=metadata",
            "--error-format=json",
            "-o",
        ])
        .arg(&output_path)
        .arg(&source_path)
        .output()
        .unwrap()
}

enum ExpectedDiagnostic<'a> {
    ErrorCode(&'a str),
    PrivateStructConstruction,
}

fn diagnostics(stderr: &str) -> Vec<serde_json::Value> {
    stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect()
}

fn matches_expected_diagnostic(
    diagnostic: &serde_json::Value,
    expected: &ExpectedDiagnostic<'_>,
) -> bool {
    match expected {
        ExpectedDiagnostic::ErrorCode(expected_code) => diagnostic
            .get("code")
            .and_then(|code| code.get("code"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|code| code == *expected_code),
        ExpectedDiagnostic::PrivateStructConstruction => {
            let message = diagnostic
                .get("message")
                .and_then(serde_json::Value::as_str);
            let has_private_field_span = diagnostic
                .get("spans")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|spans| {
                    spans.iter().any(|span| {
                        span.get("label").and_then(serde_json::Value::as_str)
                            == Some("private field")
                    })
                });

            diagnostic.get("level").and_then(serde_json::Value::as_str) == Some("error")
                && message.is_some_and(|message| {
                    message.starts_with("cannot construct ")
                        && message.ends_with("with struct literal syntax due to private fields")
                })
                && has_private_field_span
        }
    }
}

fn assert_compile_fails(case_name: &str, body: &str, expected: ExpectedDiagnostic<'_>) {
    let output = compile_snippet(case_name, body);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "{case_name} unexpectedly compiled"
    );
    let diagnostics = diagnostics(&stderr);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| matches_expected_diagnostic(diagnostic, &expected)),
        "{case_name} failed without the expected diagnostic:\n{stderr}"
    );
}

fn assert_compiles(case_name: &str, body: &str) {
    let output = compile_snippet(case_name, body);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{case_name} did not compile:\n{stderr}"
    );
}

fn assert_ledger_compile_fails(case_name: &str, body: &str, expected: ExpectedDiagnostic<'_>) {
    let output = compile_ledger_snippet(case_name, body);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "{case_name} unexpectedly compiled"
    );
    let diagnostics = diagnostics(&stderr);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| matches_expected_diagnostic(diagnostic, &expected)),
        "{case_name} failed without the expected diagnostic:\n{stderr}"
    );
}

fn assert_ledger_compiles(case_name: &str, body: &str) {
    let output = compile_ledger_snippet(case_name, body);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{case_name} did not compile:\n{stderr}"
    );
}

#[test]
fn compile_fail_harness_positive_control() {
    assert_compiles(
        "compile_fail_harness_positive_control",
        r#"
fn accepts(identity: ClosureIdentity) {
    let _ = identity.as_str();
}
"#,
    );
}

#[test]
fn reservation_cannot_be_cloned() {
    assert_compile_fails(
        "reservation_cannot_be_cloned",
        r#"
fn duplicate(reservation: RiskClosureWorkspaceReservation) {
    let _duplicate = reservation.clone();
}
"#,
        ExpectedDiagnostic::ErrorCode("E0599"),
    );
}

#[test]
fn committed_reservation_cannot_be_reused() {
    assert_compile_fails(
        "committed_reservation_cannot_be_reused",
        r#"
fn commit(reservation: RiskClosureWorkspaceReservation, identity: ClosureIdentity) {
    reservation.commit(identity).unwrap();
    let _length = reservation.workspace_len();
}
"#,
        ExpectedDiagnostic::ErrorCode("E0382"),
    );
}

#[test]
fn reservation_private_state_cannot_be_replaced() {
    assert_compile_fails(
        "reservation_private_state_cannot_be_replaced",
        r#"
fn forge(mut reservation: RiskClosureWorkspaceReservation) {
    reservation.active = false;
}
"#,
        ExpectedDiagnostic::ErrorCode("E0616"),
    );
}

#[test]
fn recovery_lease_cannot_be_cloned() {
    assert_compile_fails(
        "recovery_lease_cannot_be_cloned",
        r#"
fn duplicate(lease: RiskClosureWorkspaceLease) {
    let _duplicate = lease.clone();
}
"#,
        ExpectedDiagnostic::ErrorCode("E0599"),
    );
}

#[test]
fn terminal_release_lease_cannot_be_reused() {
    assert_compile_fails(
        "terminal_release_lease_cannot_be_reused",
        r#"
fn release(lease: RiskClosureWorkspaceLease, permit: TerminalReleasePermit) {
    lease.release_terminal(permit).unwrap();
    let _length = lease.workspace_len();
}
"#,
        ExpectedDiagnostic::ErrorCode("E0382"),
    );
}

#[test]
fn recovery_lease_private_state_cannot_be_replaced() {
    assert_compile_fails(
        "recovery_lease_private_state_cannot_be_replaced",
        r#"
fn forge(mut lease: RiskClosureWorkspaceLease) {
    lease.active = false;
}
"#,
        ExpectedDiagnostic::ErrorCode("E0616"),
    );
}

#[test]
fn terminal_release_permit_cannot_be_cloned() {
    assert_compile_fails(
        "terminal_release_permit_cannot_be_cloned",
        r#"
fn duplicate(permit: TerminalReleasePermit) {
    let _duplicate = permit.clone();
}
"#,
        ExpectedDiagnostic::ErrorCode("E0599"),
    );
}

#[test]
fn consumed_terminal_release_permit_cannot_be_reused() {
    assert_compile_fails(
        "consumed_terminal_release_permit_cannot_be_reused",
        r#"
fn release(lease: RiskClosureWorkspaceLease, permit: TerminalReleasePermit) {
    lease.release_terminal(permit).unwrap();
    let _reuse = permit;
}
"#,
        ExpectedDiagnostic::ErrorCode("E0382"),
    );
}

#[test]
fn terminal_release_permit_cannot_be_forged() {
    assert_compile_fails(
        "terminal_release_permit_cannot_be_forged",
        r#"
let _permit = TerminalReleasePermit {
    closure_identity: ClosureIdentity::new("closure").unwrap(),
};
"#,
        ExpectedDiagnostic::PrivateStructConstruction,
    );
}

#[test]
fn ledger_capability_harness_positive_control() {
    assert_ledger_compiles(
        "ledger_capability_harness_positive_control",
        r#"
fn accepts(
    new_risk: NewRiskWorkspaceHandle,
    recovery: RecoveryWorkspaceHandle,
    identity: ClosureIdentity,
) {
    let _ = new_risk.reserve_new_risk_workspace();
    let _ = recovery.checkout_retained_recovery_workspace(&identity);
}
"#,
    );
}

#[test]
fn new_risk_handle_cannot_checkout_recovery() {
    assert_ledger_compile_fails(
        "new_risk_handle_cannot_checkout_recovery",
        r#"
fn bypass(handle: NewRiskWorkspaceHandle, identity: ClosureIdentity) {
    let _ = handle.checkout_retained_recovery_workspace(&identity);
}
"#,
        ExpectedDiagnostic::ErrorCode("E0599"),
    );
}

#[test]
fn recovery_handle_cannot_reserve_new_risk() {
    assert_ledger_compile_fails(
        "recovery_handle_cannot_reserve_new_risk",
        r#"
fn bypass(handle: RecoveryWorkspaceHandle) {
    let _ = handle.reserve_new_risk_workspace();
}
"#,
        ExpectedDiagnostic::ErrorCode("E0599"),
    );
}

#[test]
fn raw_workspace_authority_cannot_be_imported() {
    assert_ledger_compile_fails(
        "raw_workspace_authority_cannot_be_imported",
        r#"
use application_resource_ledger::risk_closure_workspace::RiskClosureWorkspaceAuthority;
fn bypass(_: RiskClosureWorkspaceAuthority) {}
"#,
        ExpectedDiagnostic::ErrorCode("E0603"),
    );
}

#[test]
fn application_resource_ledger_cannot_be_constructed() {
    assert_ledger_compile_fails(
        "application_resource_ledger_cannot_be_constructed",
        r#"
let _ledger = ApplicationResourceLedger {
    risk_closure_authority: panic!(),
};
"#,
        ExpectedDiagnostic::ErrorCode("E0451"),
    );
}

#[test]
fn application_resource_ledger_has_no_production_constructor() {
    assert_ledger_compile_fails(
        "application_resource_ledger_has_no_production_constructor",
        "let _ledger = ApplicationResourceLedger::new_disabled();",
        ExpectedDiagnostic::ErrorCode("E0599"),
    );
}

#[test]
fn application_resource_ledger_has_no_alternate_new_constructor() {
    assert_ledger_compile_fails(
        "application_resource_ledger_has_no_alternate_new_constructor",
        "let _ledger = ApplicationResourceLedger::new();",
        ExpectedDiagnostic::ErrorCode("E0599"),
    );
}

#[test]
fn application_resource_ledger_has_no_default_constructor() {
    assert_ledger_compile_fails(
        "application_resource_ledger_has_no_default_constructor",
        "let _ledger = ApplicationResourceLedger::default();",
        ExpectedDiagnostic::ErrorCode("E0599"),
    );
}
