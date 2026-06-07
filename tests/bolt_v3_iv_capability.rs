use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use bolt_v2::bolt_v3_iv::capability::{
    CapabilityClassification, IvCapabilityCandidate, IvCapabilityError, IvCapabilityLedger,
    REQUIRED_CANDIDATE_SWEEP_TERMS, SeedFamily, load_capability_ledger_fixture,
    resolve_nt_cargo_evidence, scan_seed_families, scan_whole_checkout_candidates,
};

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn cargo_metadata_json() -> String {
    let output = Command::new("cargo")
        .args(["metadata", "--locked", "--format-version", "1"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata should start");

    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).expect("cargo metadata should emit UTF-8 JSON")
}

fn write_source(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("fixture path should have parent")).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn resolves_nt_checkout_from_cargo_metadata_and_lock() {
    let metadata_json = cargo_metadata_json();
    let lock_text = fs::read_to_string(repo_path("Cargo.lock")).unwrap();

    let evidence = resolve_nt_cargo_evidence(&metadata_json, &lock_text).unwrap();

    assert!(evidence.resolved_checkout_path.is_dir());
    assert!(evidence.nt_revision.len() >= 40);
    assert!(evidence.lock_revisions.contains_key("nautilus-model"));
    assert!(
        evidence
            .lock_revisions
            .values()
            .all(|revision| revision == &evidence.nt_revision)
    );
    assert!(evidence.metadata_packages.contains("nautilus-model"));
}

#[test]
fn seed_family_scan_finds_required_iv_options_surfaces() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    write_source(
        root,
        "crates/model/src/data/option_chain.rs",
        "pub struct OptionGreeks;\npub struct OptionChainSlice;\n",
    );
    write_source(
        root,
        "crates/data/src/client.rs",
        "pub fn subscribe_option_greeks() {}\npub fn unsubscribe_option_greeks() {}\n",
    );
    write_source(
        root,
        "crates/data/src/engine/option_publications.rs",
        "pub fn publish_option_chain_slice() {}\n",
    );
    write_source(
        root,
        "crates/common/src/msgbus/option_topics.rs",
        "pub fn option_greeks_topic() {}\n",
    );
    write_source(
        root,
        "crates/data/src/option_chains/mod.rs",
        "pub struct OptionChainAggregator;\n",
    );
    write_source(
        root,
        "crates/model/src/data/greeks.rs",
        "pub struct BlackScholesGreeksResult;\n",
    );
    write_source(
        root,
        "crates/adapters/generic/src/data.rs",
        "pub fn subscribe_option_greeks_for_adapter() {}\n",
    );
    write_source(
        root,
        "crates/model/src/data/custom.rs",
        "/// custom data registration\npub fn register_custom_data() {}\n",
    );

    let surfaces = scan_seed_families(root).unwrap();
    let families = surfaces
        .iter()
        .map(|candidate| {
            candidate
                .seed_family
                .expect("seed candidate should have family")
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(families, SeedFamily::required().iter().copied().collect());
}

#[test]
fn whole_checkout_sweep_includes_all_fr054_terms() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    assert_eq!(
        REQUIRED_CANDIDATE_SWEEP_TERMS,
        [
            "option",
            "options",
            "greeks",
            "implied",
            "iv",
            "volatility",
            "smile",
            "surface",
            "chain",
            "custom data",
            "strike",
            "expiry",
            "expiration",
            "tenor",
            "moneyness",
            "skew",
            "premium",
            "vol",
        ]
    );

    for (index, term) in REQUIRED_CANDIDATE_SWEEP_TERMS.iter().enumerate() {
        write_source(
            root,
            &format!("crates/model/src/generated/candidate_{index}.rs"),
            &format!("/// discovered {term} surface\npub struct Candidate{index};\n"),
        );
    }

    let candidates = scan_whole_checkout_candidates(root).unwrap();
    let matched_terms = candidates
        .iter()
        .flat_map(|candidate| candidate.matched_terms.iter().cloned())
        .collect::<BTreeSet<_>>();

    for term in REQUIRED_CANDIDATE_SWEEP_TERMS {
        assert!(matched_terms.contains(term), "missing sweep term {term}");
    }
}

#[test]
fn ledger_rejects_unclassified_candidates_and_loads_fixture() {
    let candidate = IvCapabilityCandidate {
        surface_id: "nt.model.unclassified_candidate".to_string(),
        evidence_path: "crates/model/src/data/option_chain.rs".to_string(),
        symbol: "UnclassifiedCandidate".to_string(),
        matched_terms: BTreeSet::from(["option".to_string()]),
        seed_family: None,
    };
    let empty = IvCapabilityLedger::default();

    assert!(matches!(
        empty.validate_candidates(std::slice::from_ref(&candidate)),
        Err(IvCapabilityError::UnclassifiedCandidate { surface_id })
            if surface_id == candidate.surface_id
    ));

    let fixture = load_capability_ledger_fixture(&repo_path(
        "tests/fixtures/bolt_v3_iv/capability-ledger.toml",
    ))
    .unwrap();

    assert!(
        fixture
            .classification_for("nt.model.option_greeks")
            .is_some_and(|classification| classification == CapabilityClassification::Supported)
    );
    fixture.validate_candidates(&fixture.surfaces).unwrap();
}
