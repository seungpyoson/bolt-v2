mod support;

use std::{fs, path::Path};

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_canary_gate::check_bolt_v3_live_canary_gate,
};
use tempfile::TempDir;

fn loaded_with_canary_fixture(filename: &str) -> (TempDir, LoadedBoltV3Config) {
    let temp_dir = TempDir::new().expect("temp canary root dir should create");
    let root_template_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let root_template_dir = root_template_path
        .parent()
        .expect("root fixture should have parent dir");
    let mut root_text = fs::read_to_string(&root_template_path)
        .unwrap_or_else(|error| panic!("{} should read: {error}", root_template_path.display()));
    copy_root_strategy_files(&root_text, root_template_dir, temp_dir.path());

    let relative_path = format!("tests/fixtures/bolt_v3/live_canary/{filename}");
    let canary_path = support::repo_path(&relative_path);
    let canary_text = fs::read_to_string(&canary_path)
        .unwrap_or_else(|error| panic!("{} should read: {error}", canary_path.display()));
    let canary_value = toml::from_str::<toml::Table>(&canary_text)
        .unwrap_or_else(|error| panic!("{} should parse: {error}", canary_path.display()));
    let no_submit_report_path = canary_value
        .get("no_submit_readiness_report_path")
        .and_then(toml::Value::as_str)
        .expect("canary fixture should name no-submit report path");

    root_text.push_str("\n[live_canary]\n");
    root_text.push_str(&canary_text);

    let root_path = temp_dir.path().join("root.toml");
    fs::write(&root_path, root_text).expect("temp root should write");
    copy_canary_report(
        no_submit_report_path,
        canary_path.parent().unwrap(),
        temp_dir.path(),
    );

    let loaded = load_bolt_v3_config(&root_path).expect("temp v3 root TOML should load");
    (temp_dir, loaded)
}

fn copy_root_strategy_files(root_text: &str, source_root_dir: &Path, temp_root_dir: &Path) {
    let root_value =
        toml::from_str::<toml::Table>(root_text).expect("root fixture should parse as TOML value");
    let strategy_files = root_value
        .get("strategy_files")
        .and_then(toml::Value::as_array)
        .expect("root fixture should list strategy files");
    for strategy_file in strategy_files {
        let relative = strategy_file
            .as_str()
            .expect("strategy_files entries should be strings");
        let source = source_root_dir.join(relative);
        let destination = temp_root_dir.join(relative);
        let destination_dir = destination
            .parent()
            .expect("strategy fixture destination should have parent");
        fs::create_dir_all(destination_dir).expect("temp strategy parent should create");
        fs::copy(&source, &destination).unwrap_or_else(|error| {
            panic!(
                "strategy fixture {} should copy to {}: {error}",
                source.display(),
                destination.display()
            )
        });
    }
}

fn copy_canary_report(report_path: &str, source_canary_dir: &Path, temp_root_dir: &Path) {
    let source = source_canary_dir.join(
        Path::new(report_path)
            .file_name()
            .expect("canary report path should have file name"),
    );
    let destination = temp_root_dir.join(report_path);
    let destination_dir = destination
        .parent()
        .expect("canary report destination should have parent");
    fs::create_dir_all(destination_dir).expect("temp canary report parent should create");
    fs::copy(&source, &destination).unwrap_or_else(|error| {
        panic!(
            "canary report fixture {} should copy to {}: {error}",
            source.display(),
            destination.display()
        )
    });
}

#[test]
fn live_canary_block_loads_from_root_toml() {
    let (_temp_dir, loaded) = loaded_with_canary_fixture("accepted.toml");

    assert!(
        loaded.root.live_canary.is_some(),
        "live canary config must come from root TOML load"
    );
}

#[test]
fn live_canary_gate_accepts_satisfied_no_submit_readiness_report() {
    let (_temp_dir, loaded) = loaded_with_canary_fixture("accepted.toml");
    let canary = loaded
        .root
        .live_canary
        .as_ref()
        .expect("fixture should include live canary config");

    let report = check_bolt_v3_live_canary_gate(&loaded)
        .expect("satisfied no-submit readiness report should pass live canary gate");

    assert_eq!(report.approval_id, canary.approval_id);
    assert_eq!(report.max_live_order_count, canary.max_live_order_count);
    assert_eq!(report.max_notional_per_order, canary.max_notional_per_order);
}

#[test]
fn live_canary_gate_rejects_failed_no_submit_readiness_report() {
    let (_temp_dir, loaded) = loaded_with_canary_fixture("failed.toml");

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .expect_err("failed no-submit readiness report should block live canary");

    assert!(
        error
            .to_string()
            .contains("no-submit readiness report contains failed or skipped facts"),
        "expected no-submit readiness failure, got: {error}"
    );
}

#[test]
fn live_canary_gate_rejects_incomplete_no_submit_readiness_report() {
    let (_temp_dir, loaded) = loaded_with_canary_fixture("incomplete.toml");

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .expect_err("incomplete no-submit readiness report should block live canary");

    assert!(
        error.to_string().contains("does not contain satisfied"),
        "expected missing no-submit stage failure, got: {error}"
    );
}

#[test]
fn live_canary_gate_rejects_canary_notional_above_root_risk_cap() {
    let (_temp_dir, loaded) = loaded_with_canary_fixture("exceeds_root_risk.toml");

    let error = check_bolt_v3_live_canary_gate(&loaded)
        .expect_err("canary notional above configured root risk cap should block live canary");

    assert!(
        error.to_string().contains(
            "live_canary.max_notional_per_order exceeds root risk.default_max_notional_per_order"
        ),
        "expected root-risk cap failure, got: {error}"
    );
}

#[test]
fn live_canary_gate_source_does_not_submit_or_connect() {
    let source = include_str!("../src/bolt_v3_live_canary_gate.rs");
    for forbidden in source_forbidden_terms() {
        assert!(
            !source.contains(&forbidden),
            "live canary gate must stay approval/readiness-only; source references `{forbidden}`"
        );
    }
}

fn source_forbidden_terms() -> Vec<String> {
    let path = support::repo_path("tests/fixtures/bolt_v3/live_canary/forbidden_core_terms.toml");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} should read: {error}", path.display()));
    let value = toml::from_str::<toml::Table>(&text)
        .unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()));
    value
        .get("terms")
        .and_then(toml::Value::as_array)
        .expect("forbidden core term fixture should define terms array")
        .iter()
        .map(|term| {
            term.as_str()
                .expect("forbidden core terms should be strings")
                .to_string()
        })
        .chain(
            [
                ".run(",
                ".start(",
                "connect_bolt_v3_clients",
                "disconnect_bolt_v3_clients",
                "submit_order",
                "submit_order_list",
                "modify_order",
                "cancel_order",
                "OrderBuilder",
                "OrderSubmitter",
                "SsmResolverSession",
                "resolve_bolt_v3_secrets",
                "python",
                "PyO3",
                "maturin",
                "Command::new",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .collect()
}
