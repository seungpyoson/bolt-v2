use std::fs;
use std::path::PathBuf;

#[test]
fn production_readiness_contract_links_issue_369_speckit_artifacts() {
    let contract = read_repo_file("docs/bolt-v3/2026-05-18-production-readiness-contract.md");
    let contract_words = compact_words(&contract);

    for required in [
        "tiny-canary ready",
        "staged live ready",
        "production live ready",
        "no-submit readiness, tiny-canary readiness, staged live readiness, and production live readiness",
        "Repeated-live operation",
        "Abort",
        "Restart recovery",
        "Post-run hygiene",
        "monitoring/alerting proof",
        "deploy provenance",
        "approval replay-resistance proof",
        "single-runner protection proof",
        "raw secrets, private keys, raw approval ids, or account balances",
    ] {
        assert!(
            contract_words.contains(required),
            "production readiness contract must mention {required}"
        );
    }

    let ledger = read_repo_file("docs/bolt-v3/2026-04-25-bolt-v3-contract-ledger.md");
    assert!(ledger.contains("## 17. Bolt-v3 production readiness levels"));
    assert!(ledger.contains("docs/bolt-v3/2026-05-18-production-readiness-contract.md"));

    let status_map = read_repo_file("docs/bolt-v3/2026-04-28-source-grounded-status-map.md");
    assert!(status_map.contains("| 48 | Production live trading | Missing |"));
    assert!(status_map.contains("docs/bolt-v3/2026-05-18-production-readiness-contract.md"));

    for artifact in [
        "specs/013-production-live-readiness/spec.md",
        "specs/013-production-live-readiness/plan.md",
        "specs/013-production-live-readiness/research.md",
        "specs/013-production-live-readiness/data-model.md",
        "specs/013-production-live-readiness/contracts/production-readiness.md",
        "specs/013-production-live-readiness/quickstart.md",
        "specs/013-production-live-readiness/tasks.md",
        "specs/013-production-live-readiness/checklists/requirements.md",
    ] {
        let contents = read_repo_file(artifact);
        assert!(
            !contents.trim().is_empty(),
            "Issue #369 SpecKit artifact must not be empty: {artifact}"
        );
        assert!(
            repo_path(artifact).metadata().is_ok(),
            "Issue #369 SpecKit artifact missing: {artifact}"
        );
    }
}

fn read_repo_file(path: &str) -> String {
    fs::read_to_string(repo_path(path)).unwrap_or_else(|error| {
        panic!("failed to read {path}: {error}");
    })
}

fn repo_path(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn compact_words(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}
