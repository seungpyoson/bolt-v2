use std::path::PathBuf;

use bolt_v2::nt_pointer_probe::control::{
    ExpectedBranchProtection, LoadedControlPlane, compare_branch_protection_response,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(path: &str) -> PathBuf {
    repo_root()
        .join("tests/fixtures/nt_pointer_probe")
        .join(path)
}

#[test]
fn repo_control_plane_loads_and_validates() {
    let loaded = LoadedControlPlane::load_from_repo_root(&repo_root())
        .expect("repo control plane should load and validate");

    assert_eq!(loaded.control.schema_version, 1);
    assert_eq!(loaded.control.default_branch, "main");
    assert!(
        loaded
            .registry
            .seams
            .iter()
            .any(|seam| seam.name == "subscription_custom_data_semantics"),
        "expected initial seam registry to include subscription semantics seam"
    );
}

#[test]
fn shared_crate_prefix_safe_list_fails_closed() {
    let err = LoadedControlPlane::load_from_repo_root(&fixture("bad_shared_crate_prefix"))
        .expect_err("shared NT crate prefix safe-list should fail validation");

    assert!(
        err.to_string()
            .contains("shared NT crate safe-list entries must use exact match"),
        "unexpected error: {err}"
    );
}

#[test]
fn shared_crate_root_prefix_safe_list_fails_closed() {
    let err = LoadedControlPlane::load_from_repo_root(&fixture("bad_shared_crate_root_prefix"))
        .expect_err("shared NT crate root safe-list should fail validation");

    assert!(
        err.to_string()
            .contains("shared NT crate safe-list entries must use exact match"),
        "unexpected error: {err}"
    );
}

#[test]
fn branch_protection_comparison_accepts_matching_fixture() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual = std::fs::read_to_string(fixture("branch_protection/matching_actual.json"))
        .expect("matching actual fixture should load");

    compare_branch_protection_response(&expected, &actual)
        .expect("matching branch protection fixture should compare cleanly");
}

#[test]
fn branch_protection_comparison_rejects_unprotected_branch() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual = std::fs::read_to_string(fixture("branch_protection/unprotected_actual.json"))
        .expect("unprotected actual fixture should load");

    let err = compare_branch_protection_response(&expected, &actual)
        .expect_err("unprotected branch should fail drift comparison");

    assert!(
        err.to_string()
            .contains("branch protection drift: expected protected branch"),
        "unexpected error: {err}"
    );
}
