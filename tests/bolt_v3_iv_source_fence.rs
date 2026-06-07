use std::path::Path;

#[test]
fn iv_source_fence_entrypoint_is_wired_to_the_iv_module_boundary() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    assert!(manifest_dir.join("src/bolt_v3_iv").is_dir());
    assert!(manifest_dir.join("src/bolt_v3_iv/mod.rs").is_file());
}
