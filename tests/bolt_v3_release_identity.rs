mod support;

use std::fs;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_release_identity::{
        bolt_v3_compiled_nautilus_trader_revision, bolt_v3_config_hash,
        load_bolt_v3_release_identity,
    },
};
use tempfile::TempDir;

fn existing_strategy_root_fixture() -> std::path::PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml")
}

#[test]
fn release_identity_loads_from_configured_manifest_and_verifies_config_hash_and_nt_pin() {
    let temp_dir = TempDir::new().unwrap();
    let root_path = temp_root_with_manifest_path(temp_dir.path());
    let loaded = load_bolt_v3_config(&root_path).expect("v3 TOML fixture should load");
    let config_hash = bolt_v3_config_hash(&loaded).expect("config hash should compute");
    let nt_revision =
        bolt_v3_compiled_nautilus_trader_revision().expect("NT revision should come from Cargo");
    write_release_manifest(temp_dir.path(), &config_hash, &nt_revision);

    let identity = load_bolt_v3_release_identity(&loaded).expect("release identity should load");

    assert_eq!(identity.release_id, "release-sha");
    assert_eq!(identity.config_hash, config_hash);
    assert_eq!(identity.nautilus_trader_revision, nt_revision);
}

#[test]
fn release_identity_rejects_manifest_config_hash_mismatch() {
    let temp_dir = TempDir::new().unwrap();
    let root_path = temp_root_with_manifest_path(temp_dir.path());
    let loaded = load_bolt_v3_config(&root_path).expect("v3 TOML fixture should load");
    let nt_revision =
        bolt_v3_compiled_nautilus_trader_revision().expect("NT revision should come from Cargo");
    write_release_manifest(temp_dir.path(), &"0".repeat(64), &nt_revision);

    let error = load_bolt_v3_release_identity(&loaded).unwrap_err();

    assert!(error.to_string().contains("config_hash mismatch"));
}

#[test]
fn release_identity_rejects_manifest_nt_revision_mismatch() {
    let temp_dir = TempDir::new().unwrap();
    let root_path = temp_root_with_manifest_path(temp_dir.path());
    let loaded = load_bolt_v3_config(&root_path).expect("v3 TOML fixture should load");
    let config_hash = bolt_v3_config_hash(&loaded).expect("config hash should compute");
    write_release_manifest(temp_dir.path(), &config_hash, &"1".repeat(40));

    let error = load_bolt_v3_release_identity(&loaded).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("nautilus_trader_revision mismatch")
    );
}

#[test]
fn release_identity_wiring_does_not_use_env_git_or_cwd_identity_sources() {
    let path = support::repo_path("src/bolt_v3_release_identity.rs");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", path.display()));

    for forbidden in [
        "std::env::var",
        "current_dir",
        "git rev-parse",
        "Command::new",
        "release-sha",
        "config-hash",
        "38b912a8b0fe14e4046773973ff46a3b798b1e3e",
    ] {
        assert!(
            !source.contains(forbidden),
            "release identity source must not use `{forbidden}`"
        );
    }
}

fn temp_root_with_manifest_path(temp_dir: &std::path::Path) -> std::path::PathBuf {
    let root_template = fs::read_to_string(existing_strategy_root_fixture()).unwrap();
    let root_text = root_template.replace(
        "/var/lib/bolt/releases/current/release-identity.toml",
        temp_dir
            .join("release-identity.toml")
            .to_str()
            .expect("temp path should be UTF-8"),
    );
    let root_path = temp_dir.join("root.toml");
    let strategy_dir = temp_dir.join("strategies");
    fs::create_dir_all(&strategy_dir).unwrap();
    fs::write(&root_path, root_text).unwrap();
    fs::copy(
        support::repo_path(
            "tests/fixtures/bolt_v3_existing_strategy/strategies/eth_chainlink_taker.toml",
        ),
        strategy_dir.join("eth_chainlink_taker.toml"),
    )
    .unwrap();
    root_path
}

fn write_release_manifest(temp_dir: &std::path::Path, config_hash: &str, nt_revision: &str) {
    fs::write(
        temp_dir.join("release-identity.toml"),
        format!(
            r#"
release_id = "release-sha"
git_commit_sha = "release-sha"
nautilus_trader_revision = "{nt_revision}"
binary_sha256 = "{binary_sha256}"
cargo_lock_sha256 = "{cargo_lock_sha256}"
config_hash = "{config_hash}"
build_profile = "release"
artifact_sha256 = {{ "bolt-v2" = "{artifact_sha256}" }}
"#,
            binary_sha256 = "a".repeat(64),
            cargo_lock_sha256 = "b".repeat(64),
            artifact_sha256 = "c".repeat(64),
        ),
    )
    .unwrap();
}
