use backtesting_vertical_slice::artifact_store_secrets::{
    ArtifactStoreSecretResolver, ArtifactStoreSsmResolver,
};

#[test]
fn artifact_store_ssm_resolver_constructs_without_aws_calls_and_starts_empty() {
    let resolver = ArtifactStoreSsmResolver::new()
        .expect("constructing the resolver must not call AWS or require credentials");

    assert_eq!(resolver.cached_region_count(), 0);
}

#[test]
fn artifact_store_secret_resolver_trait_shape_accepts_region_and_path() {
    fn _assert_signature<T: ArtifactStoreSecretResolver>(_resolver: &mut T) {}

    let mut resolver = ArtifactStoreSsmResolver::new().expect("resolver");
    _assert_signature(&mut resolver);
}

#[test]
fn production_artifact_store_resolver_uses_aws_sdk_not_aws_cli() {
    let source = include_str!("../src/artifact_store_secrets.rs");

    assert!(!source.contains("std::process::Command"));
    assert!(!source.contains("\"aws\""));
    assert!(source.contains("aws_sdk_ssm::"));
}
