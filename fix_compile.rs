use std::fs;

fn main() {
    let mut decision_evidence_test = fs::read_to_string("tests/bolt_v3_decision_evidence.rs").unwrap();
    let s1 = "        rv_snapshot_blockers: vec![BoltV3ExitRvSnapshotBlocker::QuorumNotReady],\n        rv_source_diagnostics: Vec::new(),";
    let r1 = "        rv_snapshot_blockers: Some(vec![BoltV3ExitRvSnapshotBlocker::QuorumNotReady]),\n        rv_source_diagnostics: Some(Vec::new()),";

    if let Some(pos) = decision_evidence_test.find(s1) {
        decision_evidence_test.replace_range(pos..pos+s1.len(), r1);
        fs::write("tests/bolt_v3_decision_evidence.rs", decision_evidence_test).unwrap();
        println!("Fixed bolt_v3_decision_evidence.rs tests");
    } else {
        println!("Could not find target in bolt_v3_decision_evidence.rs test");
    }
}
