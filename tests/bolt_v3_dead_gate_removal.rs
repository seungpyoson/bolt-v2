use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const SCAN_ROOTS: &[&str] = &[
    ".config",
    ".github",
    "src",
    "tests",
    "config",
    "Cargo.toml",
    "justfile",
    "scripts",
];

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

fn repo_scan_files(repo_root: &Path, relative_roots: &[&str]) -> Vec<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("ls-files")
        .arg("--")
        .args(relative_roots)
        .output()
        .unwrap_or_else(|error| panic!("git ls-files should run: {error}"));
    assert!(
        output.status.success(),
        "git ls-files should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|relative_path| repo_root.join(relative_path))
        .collect()
}

fn assert_repo_absent(forbidden: &[String]) {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = repo_scan_files(manifest_dir, SCAN_ROOTS);

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

fn run_git(repo_root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?} should run: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn retired_term_scan_covers_ci_surfaces() {
    for expected_root in [".config", ".github", "justfile"] {
        assert!(
            SCAN_ROOTS.contains(&expected_root),
            "retired-term scan must include {expected_root}"
        );
    }
}

#[test]
fn repo_scan_files_respects_gitignored_operator_config() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let repo_root = env::temp_dir().join(format!(
        "bolt-v3-dead-gate-removal-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(repo_root.join("config"))
        .unwrap_or_else(|error| panic!("{} should create: {error}", repo_root.display()));
    fs::write(repo_root.join(".gitignore"), "config/live.local.toml\n")
        .expect("gitignore fixture should write");
    fs::write(repo_root.join("config/root.toml"), "# tracked root\n")
        .expect("tracked config fixture should write");
    run_git(&repo_root, &["init"]);
    run_git(&repo_root, &["add", ".gitignore", "config/root.toml"]);
    let ignored_operator_config = repo_root.join("config/live.local.toml");
    fs::write(
        &ignored_operator_config,
        format!("[{}]\n", retired(&["live", "_canary"])),
    )
    .expect("ignored operator config fixture should write");

    let scanned = repo_scan_files(&repo_root, &["config"]);
    let tracked_config = repo_root.join("config/root.toml");

    fs::remove_dir_all(&repo_root)
        .unwrap_or_else(|error| panic!("{} should remove: {error}", repo_root.display()));
    assert!(
        scanned.iter().any(|path| path == &tracked_config),
        "retired-term scan should inspect tracked config"
    );
    assert!(
        !scanned.iter().any(|path| path == &ignored_operator_config),
        "retired-term scan must not inspect gitignored operator config"
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
            retired(&["proof", " ", "executor"]),
            retired(&["canary", "_proof", "_claim"]),
            retired(&["Rejected", "Invalid", "Canary", "Proof", "Claim"]),
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
        ["No", "-", "submit"].concat(),
        ["No", "-", "Submit"].concat(),
        ["no", "-", "submit"].concat(),
        ["operator", "_evidence"].concat(),
        ["Operator", "Evidence"].concat(),
        ["proof", "_policy"].concat(),
        ["tiny", "_canary"].concat(),
        ["tiny", "-", "canary"].concat(),
        ["Tiny", "-", "canary"].concat(),
        ["tiny", " ", "canary"].concat(),
        ["Rejected", "NotArmed"].concat(),
        ["new", "_unarmed"].concat(),
        "unarmed".to_string(),
        ["gate", "_session_hash"].concat(),
        ["gate", "_evidence"].concat(),
        "GateEvidence".to_string(),
        "GateSatisfaction".to_string(),
        "EntryReadinessGateSession".to_string(),
    ];

    assert_repo_absent(&forbidden);
}
