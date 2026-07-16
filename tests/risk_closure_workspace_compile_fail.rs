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

fn diagnostic_codes(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|diagnostic| {
            diagnostic
                .get("code")?
                .get("code")?
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

fn assert_compile_fails(case_name: &str, body: &str, expected_error_code: &str) {
    let output = compile_snippet(case_name, body);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "{case_name} unexpectedly compiled"
    );
    let codes = diagnostic_codes(&stderr);
    assert!(
        codes.iter().any(|code| code == expected_error_code),
        "{case_name} failed with {codes:?}, expected {expected_error_code}:\n{stderr}"
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
        "E0599",
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
        "E0382",
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
        "E0616",
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
        "E0599",
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
        "E0382",
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
        "E0616",
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
        "E0599",
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
        "E0382",
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
        "E0451",
    );
}
