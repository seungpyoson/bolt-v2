use std::fs;

#[test]
fn production_readiness_contract_links_issue_369_speckit_artifacts() {
    let contract = fs::read_to_string("docs/bolt-v3/2026-05-18-production-readiness-contract.md")
        .expect("production readiness contract should exist");

    for required in [
        "Tiny-canary ready",
        "Staged live ready",
        "Production live ready",
        "Repeated-live operation",
        "Abort",
        "Restart recovery",
        "Post-run hygiene",
        "monitoring/alerting proof",
        "deploy provenance",
        "approval replay-resistance proof",
        "single-runner protection proof",
    ] {
        assert!(
            contract.contains(required),
            "production readiness contract must mention {required}"
        );
    }

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
        assert!(
            fs::metadata(artifact).is_ok(),
            "Issue #369 SpecKit artifact missing: {artifact}"
        );
    }
}
