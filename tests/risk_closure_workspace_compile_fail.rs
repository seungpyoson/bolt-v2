use std::{fs, process::Command};

fn assert_compile_fails(case_name: &str, body: &str, expected_diagnostic: &str) {
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

    let output = Command::new("rustc")
        .args([
            "--edition=2024",
            "--crate-name",
            case_name,
            "--emit=metadata",
            "-o",
        ])
        .arg(&output_path)
        .arg(&source_path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "{case_name} unexpectedly compiled"
    );
    assert!(
        stderr.contains(expected_diagnostic),
        "{case_name} failed for the wrong reason:\n{stderr}"
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
        "no method named `clone`",
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
        "borrow of moved value: `reservation`",
    );
}

#[test]
fn reservation_private_state_cannot_be_replaced() {
    assert_compile_fails(
        "reservation_private_state_cannot_be_replaced",
        r#"
fn forge(reservation: RiskClosureWorkspaceReservation) {
    let _forged = RiskClosureWorkspaceReservation {
        active: true,
        ..reservation
    };
}
"#,
        "field `active` of struct `workspace::RiskClosureWorkspaceReservation` is private",
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
        "no method named `clone`",
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
        "borrow of moved value: `lease`",
    );
}

#[test]
fn recovery_lease_private_state_cannot_be_replaced() {
    assert_compile_fails(
        "recovery_lease_private_state_cannot_be_replaced",
        r#"
fn forge(lease: RiskClosureWorkspaceLease) {
    let _forged = RiskClosureWorkspaceLease {
        active: true,
        ..lease
    };
}
"#,
        "field `active` of struct `workspace::RiskClosureWorkspaceLease` is private",
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
        "no method named `clone`",
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
        "use of moved value: `permit`",
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
        "field `closure_identity` of struct `workspace::TerminalReleasePermit` is private",
    );
}
