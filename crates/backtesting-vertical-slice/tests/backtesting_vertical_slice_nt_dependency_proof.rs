use backtesting_vertical_slice::nt_dependency_proof::nt_dependency_proof_from_embedded_manifests;

#[test]
fn nt_dependency_proof_binds_revision_and_required_features() {
    let proof = nt_dependency_proof_from_embedded_manifests().expect("NT dependency proof");

    assert_eq!(proof.nautilus_revision.len(), 40);
    assert!(
        proof
            .nt_dependency_names
            .contains(&"nautilus-backtest".to_string())
    );
    assert!(
        proof
            .nt_dependency_names
            .contains(&"nautilus-persistence".to_string())
    );
    assert_eq!(
        proof.nautilus_backtest_features,
        vec!["examples".to_string(), "streaming".to_string()]
    );
    assert_eq!(
        proof.nautilus_persistence_features,
        vec!["cloud".to_string()]
    );
    assert!(proof.lock_sources_all_resolve_to_revision);
}
