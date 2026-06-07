use std::{
    fs,
    path::{Path, PathBuf},
};

fn repo_text(relative_path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path))
        .unwrap_or_else(|error| panic!("{relative_path} should read: {error}"))
}

fn term(value: &str) -> String {
    value.to_string()
}

fn retired(parts: &[&str]) -> String {
    parts.concat()
}

fn assert_absent(relative_path: &str, forbidden: &[String]) {
    let text = repo_text(relative_path);
    let present: Vec<&String> = forbidden
        .iter()
        .filter(|token| text.contains(token.as_str()))
        .collect();
    assert!(
        present.is_empty(),
        "{relative_path} still contains retired evidence-gate tokens: {present:?}"
    );
}

fn collect_scan_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }

    let entries = fs::read_dir(path)
        .unwrap_or_else(|error| panic!("{} should list: {error}", path.display()));
    for entry in entries {
        let entry =
            entry.unwrap_or_else(|error| panic!("{} entry should read: {error}", path.display()));
        let child = entry.path();
        let file_name = child.file_name().and_then(|value| value.to_str());
        if child.is_dir() && matches!(file_name, Some(".git" | "target")) {
            continue;
        }
        collect_scan_files(&child, out);
    }
}

fn assert_repo_absent(forbidden: &[String]) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for relative_root in [
        "src",
        "tests",
        "config",
        "Cargo.toml",
        "scripts",
        "docs/bolt-v3",
        "specs/023-nt-order-intent-layer",
    ] {
        collect_scan_files(&manifest_dir.join(relative_root), &mut files);
    }

    let mut hits = Vec::new();
    for path in files {
        if path.file_name().and_then(|value| value.to_str()) == Some("bolt_v3_dead_gate_removal.rs")
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for token in forbidden {
            if text.contains(token) {
                let relative = path.strip_prefix(manifest_dir).unwrap_or(&path);
                hits.push(format!("{} contains `{token}`", relative.display()));
            }
        }
    }

    assert!(
        hits.is_empty(),
        "repo still contains retired evidence-gate terms:\n{}",
        hits.join("\n")
    );
}

#[test]
fn retired_evidence_gate_runtime_surface_is_deleted() {
    assert_absent(
        "src/lib.rs",
        &[
            retired(&["bolt_v3", "_canary", "_proof", "_executor"]),
            retired(&["bolt_v3", "_canary", "_proof", "_policy"]),
            retired(&["bolt_v3", "_live", "_canary", "_gate"]),
            retired(&["bolt_v3", "_no", "_submit", "_readiness"]),
            retired(&["bolt_v3", "_no", "_submit", "_readiness", "_schema"]),
            retired(&["bolt_v3", "_tiny", "_canary", "_evidence"]),
            retired(&["bolt_v3", "_price", "_to", "_beat"]),
        ],
    );
    assert_absent(
        "src/main.rs",
        &[
            retired(&["No", "Submit", "Readiness"]),
            retired(&["Collect", "Canary", "Proof", "Artifacts"]),
            retired(&["Generate", "Operator", "Evidence", "Json"]),
            retired(&["Update", "Operator", "Evidence", "Toml"]),
            retired(&[
                "Write",
                "Live",
                "Canary",
                "Post",
                "Run",
                "Proof",
                "Artifacts",
            ]),
            retired(&["CANARY", "_PROOF", "_"]),
        ],
    );
    assert_absent(
        "src/bolt_v3_live_node.rs",
        &[
            retired(&["register", "_canary", "_proof", "_executor", "_on_node"]),
            retired(&["canary", "_proof", "_executor", "_enabled"]),
            term("run_blocked_before_submit"),
            retired(&[
                "write",
                "_bolt_v3",
                "_blocked",
                "_before_submit",
                "_canary",
                "_evidence",
            ]),
            term("consume_bolt_v3_live_runner_approval"),
        ],
    );
    assert_absent(
        "src/bolt_v3_submit_admission.rs",
        &[
            retired(&["bolt_v3", "_live", "_canary", "_gate"]),
            term("gate_report"),
            term("pub fn arm("),
            retired(&["canary", "_proof", "_claim"]),
            retired(&["Rejected", "Invalid", "Canary", "Proof", "Claim"]),
        ],
    );
}

#[test]
fn retired_readiness_gate_identity_is_not_part_of_strategy_evidence() {
    assert_absent(
        "src/bolt_v3_decision_evidence.rs",
        &[
            retired(&["BoltV3", "Readiness", "Gate", "Evidence", "Snapshot"]),
            retired(&["BoltV3", "Gate", "Evidence", "Identity"]),
            retired(&["validate", "_strategy_input", "_readiness", "_evidence"]),
            retired(&["validate", "_readiness", "_gate", "_evidence", "_snapshot"]),
            retired(&["from", "_entry", "_readiness", "_gate", "_session"]),
            retired(&["gate", "_session", "_hash"]),
            retired(&["gate", "_evidence"]),
            retired(&["canary", "_proof", "_claim"]),
        ],
    );
    assert_absent(
        "src/strategies/registry.rs",
        &[
            retired(&["BoltV3", "Readiness", "Gate", "Evidence", "Snapshot"]),
            retired(&["readiness", "_evidence"]),
            retired(&["with", "_readiness", "_evidence"]),
        ],
    );
    assert_absent(
        "src/strategies/binary_oracle_edge_taker/mod.rs",
        &[
            retired(&["readiness", "_evidence"]),
            retired(&["with", "_readiness", "_evidence"]),
            retired(&["gate", "_session", "_hash"]),
            retired(&["gate", "_evidence"]),
            retired(&["canary", "_proof", "_claim"]),
        ],
    );
}

#[test]
fn shipped_config_has_no_retired_live_gate_blocks() {
    assert_absent(
        "config/root.toml",
        &[
            retired(&["[", "live", "_canary", "]"]),
            retired(&["[", "live", "_canary", ".", "operator", "_evidence", "]"]),
        ],
    );
}

#[test]
fn nextest_config_has_no_deleted_evidence_gate_binaries() {
    assert_absent(
        ".config/nextest.toml",
        &[
            retired(&["bolt_v3", "_live", "_canary", "_gate"]),
            retired(&["bolt_v3", "_tiny", "_canary", "_operator"]),
        ],
    );
}

#[test]
fn repo_has_no_retired_evidence_gate_terms() {
    let forbidden = vec![
        ["canary", "_proof"].concat(),
        ["live", "_canary"].concat(),
        ["no", "_submit_readiness"].concat(),
        ["no", "_submit"].concat(),
        ["No", "Submit", "Readiness"].concat(),
        ["No", "Submit"].concat(),
        ["no", "-", "submit"].concat(),
        ["operator", "_evidence"].concat(),
        ["Operator", "Evidence"].concat(),
        ["proof", "_policy"].concat(),
        ["tiny", "_canary"].concat(),
        ["Rejected", "NotArmed"].concat(),
        ["gate", "_session_hash"].concat(),
        ["gate", "_evidence"].concat(),
        "GateEvidence".to_string(),
        "GateSatisfaction".to_string(),
        "EntryReadinessGateSession".to_string(),
    ];

    assert_repo_absent(&forbidden);
}
