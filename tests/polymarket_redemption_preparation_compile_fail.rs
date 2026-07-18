use std::{fs, process::Command};

fn cargo_check(case_name: &str, source: &str) -> std::process::Output {
    let temporary = tempfile::tempdir().expect("temporary compile-fail crate must allocate");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(manifest_dir).join("target"));
    fs::create_dir(temporary.path().join("src")).expect("source directory must create");
    fs::write(
        temporary.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "{case_name}"
version = "0.0.0"
edition = "2024"

[workspace]

[dependencies]
bolt-v2 = {{ path = {manifest_dir:?} }}
serde_json = "1"
"#,
        ),
    )
    .expect("temporary manifest must write");
    fs::copy(
        std::path::Path::new(manifest_dir).join("Cargo.lock"),
        temporary.path().join("Cargo.lock"),
    )
    .expect("repository lockfile must seed the offline compile-fail crate");
    fs::write(temporary.path().join("src/main.rs"), source).expect("temporary source must write");

    Command::new("cargo")
        .args(["check", "--offline", "--quiet"])
        .env("CARGO_TARGET_DIR", target_dir)
        .current_dir(temporary.path())
        .output()
        .expect("cargo check must run in remote verification")
}

fn assert_compile_fails(case_name: &str, source: &str, expected: &[&str]) {
    let output = cargo_check(case_name, source);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "{case_name} unexpectedly compiled"
    );
    assert!(
        expected.iter().any(|fragment| stderr.contains(fragment)),
        "{case_name} failed without an expected diagnostic {expected:?}:\n{stderr}"
    );
}

fn assert_compiles(case_name: &str, source: &str) {
    let output = cargo_check(case_name, source);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{case_name} did not compile:\n{stderr}"
    );
}

const PRELUDE: &str = r#"
use bolt_v2::{
    bolt_v3_polymarket_redemption::{
        AttemptKind, RedemptionPreparationConfig, RedemptionRequestInput,
        RedemptionPreparationPermit,
        prepare_redemption_request,
    },
    bolt_v3_application_resource_ledger::{
        RiskClosureWorkspaceLease, RiskClosureWorkspaceReservation,
    },
    bolt_v3_secrets::ResolvedEvmSigningKey,
};
"#;

#[test]
fn compile_fail_harness_positive_control() {
    assert_compiles(
        "redemption_compile_positive_control",
        &format!(
            "{PRELUDE}\nfn accepts(_: RedemptionPreparationPermit, _: &mut RiskClosureWorkspaceLease, _: &RedemptionPreparationConfig, _: &ResolvedEvmSigningKey, _: RedemptionRequestInput, _: AttemptKind) {{}}\nfn main() {{}}\n"
        ),
    );
}

#[test]
fn external_code_cannot_construct_preparation_permit() {
    assert_compile_fails(
        "redemption_permit_cannot_construct",
        &format!(
            r#"{PRELUDE}
fn main() {{
    let _permit = RedemptionPreparationPermit {{ private: () }};
}}
"#,
        ),
        &["field `private`", "private field"],
    );
}

#[test]
fn preparation_permit_cannot_be_cloned() {
    assert_compile_fails(
        "redemption_permit_cannot_clone",
        &format!(
            r#"{PRELUDE}
fn forbidden(permit: RedemptionPreparationPermit) {{
    let _duplicate = permit.clone();
}}
fn main() {{}}
"#,
        ),
        &["no method named `clone`", "method `clone` not found"],
    );
}

#[test]
fn new_risk_reservation_cannot_call_request_preparation() {
    assert_compile_fails(
        "new_risk_cannot_prepare_redemption",
        &format!(
            r#"{PRELUDE}
fn forbidden(
    permit: RedemptionPreparationPermit,
    reservation: &mut RiskClosureWorkspaceReservation,
    config: &RedemptionPreparationConfig,
    credentials: &ResolvedEvmSigningKey,
    input: RedemptionRequestInput,
) {{
    prepare_redemption_request(
        permit,
        reservation,
        config,
        credentials,
        input,
        AttemptKind::Original,
        |_| {{}},
    ).unwrap();
}}
fn main() {{}}
"#,
        ),
        &[
            "expected `&mut RiskClosureWorkspaceLease`",
            "mismatched types",
        ],
    );
}

#[test]
fn prepared_request_borrow_cannot_escape_callback() {
    assert_compile_fails(
        "prepared_request_cannot_escape",
        &format!(
            r#"{PRELUDE}
fn forbidden<'a>(
    permit: RedemptionPreparationPermit,
    lease: &'a mut RiskClosureWorkspaceLease,
    config: &RedemptionPreparationConfig,
    credentials: &ResolvedEvmSigningKey,
    input: RedemptionRequestInput,
) -> &'a [u8] {{
    let mut escaped: Option<&'a [u8]> = None;
    prepare_redemption_request(
        permit,
        lease,
        config,
        credentials,
        input,
        AttemptKind::Original,
        |prepared| escaped = Some(prepared.as_bytes()),
    ).unwrap();
    escaped.unwrap()
}}
fn main() {{}}
"#,
        ),
        &[
            "borrowed data escapes outside of closure",
            "lifetime may not live long enough",
        ],
    );
}

#[test]
fn resolved_signing_key_cannot_be_serialized() {
    assert_compile_fails(
        "resolved_credentials_cannot_serialize",
        &format!(
            r#"{PRELUDE}
fn forbidden(credentials: &ResolvedEvmSigningKey) {{
    let _ = serde_json::to_string(credentials).unwrap();
}}
fn main() {{}}
"#,
        ),
        &["the trait bound `ResolvedEvmSigningKey: serde::Serialize` is not satisfied"],
    );
}
