use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use bolt_v2::bolt_v3_iv::capability::{
    CapabilityClassification, IvCapabilityCandidate, IvCapabilityEngineMapping,
    IvCapabilityEngineMappingKind, IvCapabilityEngineMappingRule, IvCapabilityError,
    IvCapabilityLedger, REQUIRED_CANDIDATE_SWEEP_TERMS, SeedFamily, load_capability_ledger_fixture,
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
            &format!("/// discovered {term} surface\npub struct OptionCandidate{index};\n"),
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
fn whole_checkout_scan_surfaces_microstructure_terms_without_iv_anchor_words() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    for (index, term) in [
        "strike",
        "expiry",
        "expiration",
        "tenor",
        "moneyness",
        "skew",
        "premium",
    ]
    .iter()
    .enumerate()
    {
        write_source(
            root,
            &format!("crates/model/src/generated/microstructure_{index}.rs"),
            &format!("/// discovered {term}\npub struct Candidate{index};\n"),
        );
    }

    let candidates = scan_whole_checkout_candidates(root).unwrap();
    let matched_terms = candidates
        .iter()
        .flat_map(|candidate| candidate.matched_terms.iter().cloned())
        .collect::<BTreeSet<_>>();

    for term in [
        "strike",
        "expiry",
        "expiration",
        "tenor",
        "moneyness",
        "skew",
        "premium",
    ] {
        assert!(matched_terms.contains(term), "missing sweep term {term}");
    }
}

#[test]
fn option_chain_manager_scan_handles_public_const_functions_as_named_surfaces() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    write_source(
        root,
        "crates/data/src/option_chains/manager.rs",
        "/// Returns whether the option chain is bootstrapped.\npub const fn is_bootstrapped(&self) -> bool { true }\n",
    );

    let candidates = scan_whole_checkout_candidates(root).unwrap();

    assert!(
        candidates.iter().any(|candidate| candidate.surface_id
            == "nt.crates.data.src.option_chains.manager.is_bootstrapped"),
        "option-chain pub const fn should surface by its real function name"
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.surface_id != "nt.crates.data.src.option_chains.manager.fn"),
        "option-chain pub const fn must not produce a synthetic `fn` surface"
    );
}

#[test]
fn ledger_rejects_unclassified_candidates_and_loads_fixture() {
    let candidate = IvCapabilityCandidate {
        surface_id: "nt.model.unclassified_candidate".to_string(),
        evidence_path: "crates/model/src/data/option_chain.rs".to_string(),
        symbol: "UnclassifiedCandidate".to_string(),
        matched_terms: BTreeSet::from(["option".to_string()]),
        seed_family: None,
        engine_mapping: None,
    };
    let empty = IvCapabilityLedger::empty();

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

#[test]
fn fixture_maps_seed_surfaces_to_specific_engine_owners() {
    let fixture = load_capability_ledger_fixture(&repo_path(
        "tests/fixtures/bolt_v3_iv/capability-ledger.toml",
    ))
    .unwrap();

    let greeks_helper = fixture
        .engine_mapping_for("nt.crates.model.src.data.greeks.imply_vol_and_greeks")
        .unwrap();
    assert_eq!(
        greeks_helper.mapping_kind,
        IvCapabilityEngineMappingKind::Helper
    );
    assert_eq!(greeks_helper.target, "iv_derived_helper");
    let refine_helper = fixture
        .engine_mapping_for("nt.crates.model.src.data.greeks.refine_vol_and_greeks")
        .unwrap();
    assert_eq!(
        refine_helper.mapping_kind,
        IvCapabilityEngineMappingKind::Helper
    );
    assert_eq!(refine_helper.target, "iv_derived_helper");

    let option_chain = fixture
        .engine_mapping_for("nt.crates.model.src.data.option_chain.option_chain_slice")
        .unwrap();
    assert_eq!(
        option_chain.mapping_kind,
        IvCapabilityEngineMappingKind::ProductKind
    );
    assert_eq!(option_chain.target, "iv_option_chain_slice");

    let subscription = fixture
        .engine_mapping_for("nt.crates.common.src.actor.data_actor.subscribe_option_greeks")
        .unwrap();
    assert_eq!(
        subscription.mapping_kind,
        IvCapabilityEngineMappingKind::RuntimeOperation
    );
    assert_eq!(subscription.target, "iv_subscription_lifecycle");
}

#[test]
fn ledger_requires_exact_review_for_supported_candidates() {
    let candidate = IvCapabilityCandidate {
        surface_id: "nt.crates.model.src.data.new_option_surface".to_string(),
        evidence_path: "crates/model/src/data/new_option_surface.rs".to_string(),
        symbol: "NewOptionSurface".to_string(),
        matched_terms: BTreeSet::from(["option".to_string(), "surface".to_string()]),
        seed_family: None,
        engine_mapping: None,
    };
    let mut ledger = IvCapabilityLedger::empty();
    ledger.classification_rules.push(
        bolt_v2::bolt_v3_iv::capability::IvCapabilityClassificationRule {
            surface_id_prefix: "nt.crates.model.src.data.".to_string(),
            classification: CapabilityClassification::Supported,
        },
    );

    assert!(matches!(
        ledger.validate_candidates(std::slice::from_ref(&candidate)),
        Err(IvCapabilityError::UnclassifiedCandidate { surface_id })
            if surface_id == candidate.surface_id
    ));

    ledger.classifications.insert(
        candidate.surface_id.clone(),
        CapabilityClassification::Supported,
    );
    ledger
        .engine_mapping_rules
        .push(IvCapabilityEngineMappingRule {
            surface_id_prefix: candidate.surface_id.clone(),
            engine_mapping: IvCapabilityEngineMapping {
                mapping_kind: IvCapabilityEngineMappingKind::ProductKind,
                target: "iv_option_surface".to_string(),
            },
        });
    ledger.validate_candidates(&[candidate]).unwrap();
}

#[test]
fn ledger_rejects_new_candidates_matched_only_by_broad_crate_prefix() {
    let candidate = IvCapabilityCandidate {
        surface_id: "nt.crates.adapters.deribit.src.data.new_option_surface".to_string(),
        evidence_path: "crates/adapters/deribit/src/data.rs".to_string(),
        symbol: "NewOptionSurface".to_string(),
        matched_terms: BTreeSet::from(["option".to_string(), "surface".to_string()]),
        seed_family: None,
        engine_mapping: None,
    };
    let mut ledger = IvCapabilityLedger::empty();
    ledger.classification_rules.push(
        bolt_v2::bolt_v3_iv::capability::IvCapabilityClassificationRule {
            surface_id_prefix: "nt.crates.adapters.".to_string(),
            classification: CapabilityClassification::Excluded,
        },
    );

    assert!(matches!(
        ledger.validate_candidates(std::slice::from_ref(&candidate)),
        Err(IvCapabilityError::UnclassifiedCandidate { surface_id })
            if surface_id == candidate.surface_id
    ));
}

#[test]
fn ledger_rejects_review_candidates_covered_only_by_deep_module_prefix() {
    let candidate = IvCapabilityCandidate {
        surface_id: "nt.crates.indicators.src.volatility.new_iv_band".to_string(),
        evidence_path: "crates/indicators/src/volatility/new_iv_band.rs".to_string(),
        symbol: "NewIvBand".to_string(),
        matched_terms: BTreeSet::from(["volatility".to_string()]),
        seed_family: None,
        engine_mapping: None,
    };
    let mut ledger = IvCapabilityLedger::empty();
    ledger.classification_rules.push(
        bolt_v2::bolt_v3_iv::capability::IvCapabilityClassificationRule {
            surface_id_prefix: "nt.crates.indicators.src.volatility.".to_string(),
            classification: CapabilityClassification::Excluded,
        },
    );

    assert!(matches!(
        ledger.validate_candidates(std::slice::from_ref(&candidate)),
        Err(IvCapabilityError::UnclassifiedCandidate { surface_id })
            if surface_id == candidate.surface_id
    ));
}

#[test]
fn broad_adapter_exclusion_can_cover_non_iv_options_false_positives() {
    let candidate = IvCapabilityCandidate {
        surface_id: "nt.crates.adapters.example.src.websocket.client.with_options".to_string(),
        evidence_path: "crates/adapters/example/src/websocket/client.rs".to_string(),
        symbol: "with_options".to_string(),
        matched_terms: BTreeSet::from(["options".to_string()]),
        seed_family: Some(SeedFamily::Adapter),
        engine_mapping: None,
    };
    let mut ledger = IvCapabilityLedger::empty();
    ledger.classification_rules.push(
        bolt_v2::bolt_v3_iv::capability::IvCapabilityClassificationRule {
            surface_id_prefix: "nt.crates.adapters.".to_string(),
            classification: CapabilityClassification::Excluded,
        },
    );

    ledger.validate_candidates(&[candidate]).unwrap();
}

#[test]
fn ledger_requires_engine_mapping_for_supported_candidates() {
    let candidate = IvCapabilityCandidate {
        surface_id: "nt.crates.model.src.data.greeks.imply_vol_and_greeks".to_string(),
        evidence_path: "crates/model/src/data/greeks.rs".to_string(),
        symbol: "imply_vol_and_greeks".to_string(),
        matched_terms: BTreeSet::from(["greeks".to_string(), "implied".to_string()]),
        seed_family: Some(SeedFamily::GreeksHelper),
        engine_mapping: None,
    };
    let mut ledger = IvCapabilityLedger::empty();
    ledger.classifications.insert(
        candidate.surface_id.clone(),
        CapabilityClassification::Supported,
    );

    assert!(matches!(
        ledger.validate_candidates(std::slice::from_ref(&candidate)),
        Err(IvCapabilityError::MissingEngineMapping { surface_id })
            if surface_id == candidate.surface_id
    ));
}

#[test]
fn capability_ledger_classifies_whole_cargo_resolved_nt_checkout() {
    let metadata = cargo_metadata_json();
    let lock_text = fs::read_to_string(repo_path("Cargo.lock")).unwrap();
    let evidence = resolve_nt_cargo_evidence(&metadata, &lock_text).unwrap();
    let candidates = scan_whole_checkout_candidates(&evidence.resolved_checkout_path).unwrap();
    assert!(
        !candidates.is_empty(),
        "whole-checkout capability scan must discover NT IV/options candidates"
    );
    let discovered_families = scan_seed_families(&evidence.resolved_checkout_path)
        .unwrap()
        .into_iter()
        .filter_map(|candidate| candidate.seed_family)
        .collect::<BTreeSet<_>>();
    for required_family in SeedFamily::required() {
        assert!(
            discovered_families.contains(required_family),
            "whole-checkout capability scan missed required seed family {required_family:?}"
        );
    }
    let fixture = load_capability_ledger_fixture(&repo_path(
        "tests/fixtures/bolt_v3_iv/capability-ledger.toml",
    ))
    .unwrap();

    let validation_errors = candidates
        .iter()
        .filter_map(|candidate| {
            fixture
                .validate_candidates(std::slice::from_ref(candidate))
                .err()
                .map(|error| {
                    (
                        candidate.surface_id.clone(),
                        candidate.matched_terms.clone(),
                        candidate.seed_family,
                        error,
                    )
                })
        })
        .collect::<Vec<_>>();
    assert!(
        validation_errors.is_empty(),
        "whole-checkout capability ledger has unresolved candidates: {:#?}",
        validation_errors.iter().take(100).collect::<Vec<_>>()
    );
}
