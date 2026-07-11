use backtesting_vertical_slice::nt_dependency_proof::{
    nt_dependency_proof_from_embedded_manifests, verified_nt_revision_from_manifests,
};

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

#[test]
fn verified_revision_rejects_lockfile_skew() {
    let manifest = include_str!("../Cargo.toml");
    let lock = include_str!("../Cargo.lock");
    let declared_revision = nt_dependency_proof_from_embedded_manifests()
        .expect("embedded dependency proof")
        .nautilus_revision;
    let divergent_lock = lock.replace(
        &format!("#{declared_revision}"),
        "#aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );

    let error = verified_nt_revision_from_manifests(manifest, &divergent_lock)
        .expect_err("lockfile skew must fail closed");
    assert!(error.to_string().contains("does not resolve"));
}
