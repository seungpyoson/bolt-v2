use std::{fs, path::Path};

#[test]
fn iv_source_fence_entrypoint_is_wired_to_the_iv_module_boundary() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    assert!(manifest_dir.join("src/bolt_v3_iv").is_dir());
    assert!(manifest_dir.join("src/bolt_v3_iv/mod.rs").is_file());
}

#[test]
fn source_fence_accepts_public_strategy_query_imports() {
    let source = r#"
use crate::bolt_v3_iv::query::{IvQuery, IvQueryHandle};

fn strategy_consumes_iv(handle: &IvQueryHandle, query: &IvQuery) {
    let _ = handle.query(query);
}
"#;

    assert!(iv_strategy_source_fence_violations(source).is_empty());
}

#[test]
fn source_fence_rejects_strategy_local_nt_iv_subscriptions() {
    let source = r#"
use nautilus_live::node::LiveNode;

fn strategy_owned_subscription(node: &mut LiveNode) {
    node.subscribe_option_greeks();
}
"#;

    assert_strategy_source_fence_rejects(source, "strategy-owned NT IV subscription");
}

#[test]
fn source_fence_rejects_strategy_local_nt_helper_derivation() {
    let source = r#"
use nautilus_model::data::imply_vol_and_greeks;

fn strategy_owned_derived_iv() {
    let _ = imply_vol_and_greeks;
}
"#;

    assert_strategy_source_fence_rejects(source, "strategy-local NT helper derivation");
}

#[test]
fn source_fence_rejects_raw_audit_reader_and_raw_dto_strategy_imports() {
    let source = r#"
use crate::bolt_v3_iv::raw_access::read_raw_event;
use crate::bolt_v3_iv::ingest::{IvRawEvent, IvRawPayload};
"#;

    assert_strategy_source_fence_rejects(source, "raw IV payload bypass");
}

#[test]
fn source_fence_rejects_strategy_state_handle_escape_hatch() {
    let source = r#"
use crate::bolt_v3_iv::query::IvQueryHandle;

fn strategy_escapes_query_authz(handle: &IvQueryHandle) {
    let _ = handle.derived_outputs();
    let _ = handle.derived_inputs();
    let _ = handle.query_rejections();
    let _ = handle.source_health_for("configured-profile", "configured-source");
}
"#;

    assert_strategy_source_fence_rejects(source, "IV query state escape hatch");
}

#[test]
fn source_fence_rejects_forbidden_iv_bypass_in_strategy_tree() {
    let temp = tempfile::tempdir().unwrap();
    let strategy_dir = temp.path().join("src/strategies/configured_strategy");
    fs::create_dir_all(&strategy_dir).unwrap();
    fs::write(
        strategy_dir.join("mod.rs"),
        "use crate::bolt_v3_iv::raw_access::read_raw_event;\n",
    )
    .unwrap();

    let violations = iv_strategy_tree_source_fence_violations(&temp.path().join("src/strategies"));

    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("raw IV payload bypass")),
        "expected fake strategy tree to reject raw IV bypass, got {violations:?}"
    );
}

#[test]
fn source_fence_accepts_current_strategy_tree_without_iv_bypasses() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let violations = iv_strategy_tree_source_fence_violations(&manifest_dir.join("src/strategies"));

    assert!(
        violations.is_empty(),
        "current strategy tree had IV source-fence violations: {violations:?}"
    );
}

fn assert_strategy_source_fence_rejects(source: &str, expected_reason: &str) {
    let violations = iv_strategy_source_fence_violations(source);
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains(expected_reason)),
        "expected source-fence violation containing {expected_reason:?}, got {violations:?}"
    );
}

fn iv_strategy_source_fence_violations(_source: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let checks = [
        (
            "subscribe_option_greeks",
            "strategy-owned NT IV subscription",
        ),
        (
            "subscribe_option_chain",
            "strategy-owned NT IV subscription",
        ),
        (
            "subscribe_aggregate_greeks",
            "strategy-owned NT IV subscription",
        ),
        ("subscribe_custom_data", "strategy-owned NT IV subscription"),
        (
            "imply_vol_and_greeks",
            "strategy-local NT helper derivation",
        ),
        (
            "black_scholes_greeks",
            "strategy-local NT helper derivation",
        ),
        ("read_raw_event", "raw IV payload bypass"),
        ("IvRawAuditRequest", "raw IV payload bypass"),
        ("IvRawEvent", "raw IV payload bypass"),
        ("IvRawPayload", "raw IV payload bypass"),
        ("derived_outputs", "IV query state escape hatch"),
        ("derived_inputs", "IV query state escape hatch"),
        ("query_rejections", "IV query state escape hatch"),
        ("source_health_for", "IV query state escape hatch"),
    ];

    for (needle, reason) in checks {
        if _source.contains(needle) {
            violations.push(format!("{reason}: {needle}"));
        }
    }

    violations
}

fn iv_strategy_tree_source_fence_violations(strategy_root: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    collect_strategy_source_fence_violations(strategy_root, &mut violations);
    violations
}

fn collect_strategy_source_fence_violations(path: &Path, violations: &mut Vec<String>) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_strategy_source_fence_violations(&path, violations);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }

        let source = fs::read_to_string(&path).unwrap();
        violations.extend(
            iv_strategy_source_fence_violations(&source)
                .into_iter()
                .map(|violation| format!("{}: {violation}", path.display())),
        );
    }
}
