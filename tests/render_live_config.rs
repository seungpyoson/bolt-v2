mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use bolt_v2::{MaterializationOutcome, materialize_live_config};
use support::{TempCaseDir, repo_path};

#[test]
fn materialize_live_config_creates_read_only_output() {
    let tempdir = TempCaseDir::new("create-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should create the runtime config");

    assert_eq!(outcome, MaterializationOutcome::Created);
    assert!(output_path.exists());
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn materialize_live_config_creates_missing_parent_directories() {
    let tempdir = TempCaseDir::new("nested-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("nested/runtime/live.toml");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should create nested output directories");

    assert_eq!(outcome, MaterializationOutcome::Created);
    assert!(output_path.exists());
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn materialize_live_config_updates_drifted_contents() {
    let tempdir = TempCaseDir::new("updated-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    materialize_live_config(&input_path, &output_path).expect("first render should succeed");

    #[cfg(unix)]
    set_mode(&output_path, 0o600);
    #[cfg(not(unix))]
    make_writable_if_needed(&output_path);
    fs::write(&output_path, "drifted = true\n").expect("drifted output should be writable");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should repair drifted contents");

    assert_eq!(outcome, MaterializationOutcome::Updated);
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
    #[cfg(unix)]
    assert_mode(&output_path, 0o400);
}

#[test]
fn materialize_live_config_repairs_permissions_without_rewriting_contents() {
    let tempdir = TempCaseDir::new("permissions-repaired");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    materialize_live_config(&input_path, &output_path).expect("first render should succeed");

    let modified_before = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");
    thread::sleep(Duration::from_millis(20));

    #[cfg(unix)]
    set_mode(&output_path, 0o600);
    #[cfg(not(unix))]
    make_writable_if_needed(&output_path);

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("materializer should repair writable drift");

    let modified_after = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");

    assert_eq!(outcome, MaterializationOutcome::PermissionsRepaired);
    assert_eq!(
        modified_before, modified_after,
        "permission repair should not rewrite contents"
    );
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
    #[cfg(unix)]
    assert_mode(&output_path, 0o400);
}

#[test]
fn materialize_live_config_leaves_matching_read_only_output_unchanged() {
    let tempdir = TempCaseDir::new("unchanged-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let output_path = tempdir.path().join("live.toml");

    materialize_live_config(&input_path, &output_path).expect("first render should succeed");

    let modified_before = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");
    thread::sleep(Duration::from_millis(20));

    let outcome =
        materialize_live_config(&input_path, &output_path).expect("second render should succeed");

    let modified_after = fs::metadata(&output_path)
        .expect("output metadata should exist")
        .modified()
        .expect("output mtime should exist");

    assert_eq!(outcome, MaterializationOutcome::Unchanged);
    assert_eq!(
        modified_before, modified_after,
        "unchanged output should not be rewritten"
    );
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn render_live_config_binary_supports_relative_paths() {
    let tempdir = TempCaseDir::new("relative-output");
    let input_path = write_input(&tempdir, "live.local.toml");
    let _ = input_path;

    let output = Command::new(env!("CARGO_BIN_EXE_render_live_config"))
        .current_dir(tempdir.path())
        .args(["--input", "live.local.toml", "--output", "live.toml"])
        .output()
        .expect("renderer binary should run");

    assert!(
        output.status.success(),
        "binary failed: stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let output_path = tempdir.path().join("live.toml");
    assert!(output_path.exists());
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
}

#[test]
fn legacy_operator_config_without_phase1_sections_still_materializes() {
    let tempdir = TempCaseDir::new("legacy-phase1-compat");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let input = r#"
[node]
name = "BOLT-V2-TEST"
trader_id = "BOLT-TEST"

[polymarket]
event_slug = "btc-updown-5m"
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"
"#;

    fs::write(&input_path, input).expect("input config should be written");

    let outcome = materialize_live_config(&input_path, &output_path)
        .expect("legacy operator config should still materialize");

    assert_eq!(outcome, MaterializationOutcome::Created);

    let rendered = fs::read_to_string(&output_path).expect("rendered config should be readable");
    assert!(!rendered.contains("\n[reference]\n"));
    assert!(!rendered.contains("\n[[rulesets]]\n"));
    assert!(!rendered.contains("\n[audit]\n"));
    assert_generated_output(&output_path);
}

#[cfg(unix)]
#[test]
fn render_live_config_binary_respects_restrictive_umask_on_create() {
    let tempdir = TempCaseDir::new("umask-create");
    let _input_path = write_input(&tempdir, "live.local.toml");

    let output = Command::new("/bin/sh")
        .current_dir(tempdir.path())
        .env("BIN", env!("CARGO_BIN_EXE_render_live_config"))
        .env("INPUT", "live.local.toml")
        .env("OUTPUT", "live.toml")
        .arg("-c")
        .arg("umask 077 && \"$BIN\" --input \"$INPUT\" --output \"$OUTPUT\"")
        .output()
        .expect("renderer binary should run with restrictive umask");

    assert!(
        output.status.success(),
        "binary failed: stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let output_path = tempdir.path().join("live.toml");
    assert_generated_output(&output_path);
    assert_read_only(&output_path);
    assert_mode(&output_path, 0o400);
}

fn write_input(tempdir: &TempCaseDir, file_name: &str) -> PathBuf {
    let input_path = tempdir.path().join(file_name);
    fs::write(&input_path, tracked_live_local_example()).expect("input config should be written");
    input_path
}

fn tracked_live_local_example() -> String {
    fs::read_to_string(repo_path("config/live.local.example.toml"))
        .expect("tracked operator template should be readable")
}

fn assert_generated_output(path: &Path) {
    let rendered = fs::read_to_string(path).expect("generated output should be readable");
    assert!(rendered.contains("# GENERATED FILE - DO NOT EDIT."));
    assert!(rendered.contains("# Source of truth:"));
    assert!(rendered.contains("[[data_clients]]"));
    assert!(rendered.contains("[[exec_clients]]"));
    assert!(rendered.contains("[[strategies]]"));
}

fn assert_read_only(path: &Path) {
    let metadata = fs::metadata(path).expect("output metadata should exist");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(metadata.permissions().mode() & 0o222, 0);
    }

    #[cfg(not(unix))]
    {
        assert!(metadata.permissions().readonly());
    }
}

#[cfg(not(unix))]
fn make_writable_if_needed(path: &Path) {
    let mut permissions = fs::metadata(path)
        .expect("output metadata should exist")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).expect("output should become writable");
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .expect("output metadata should exist")
        .permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).expect("output mode should be updated");
}

#[cfg(unix)]
fn assert_mode(path: &Path, expected_mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    let actual_mode = fs::metadata(path)
        .expect("output metadata should exist")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(actual_mode, expected_mode);
}
