use std::collections::BTreeMap;

use bolt_v2::venue_contract::{
    Capability, Policy, Provenance, StreamContract, VenueContract,
};
use tempfile::tempdir;

fn base_polymarket_streams() -> BTreeMap<String, StreamContract> {
    let supported = |policy: Policy| StreamContract {
        capability: Capability::Supported,
        policy: Some(policy),
        provenance: Provenance::Native,
        reason: None,
        derived_from: None,
    };
    let unsupported = || StreamContract {
        capability: Capability::Unsupported,
        policy: None,
        provenance: Provenance::Native,
        reason: Some("n/a".to_string()),
        derived_from: None,
    };

    BTreeMap::from([
        ("quotes".to_string(), supported(Policy::Required)),
        ("trades".to_string(), supported(Policy::Required)),
        (
            "order_book_deltas".to_string(),
            supported(Policy::Required),
        ),
        ("order_book_depths".to_string(), unsupported()),
        ("index_prices".to_string(), unsupported()),
        ("mark_prices".to_string(), unsupported()),
        ("instrument_closes".to_string(), unsupported()),
    ])
}

fn make_contract(streams: BTreeMap<String, StreamContract>) -> VenueContract {
    VenueContract {
        schema_version: 1,
        venue: "test".to_string(),
        adapter_version: "bolt-v2".to_string(),
        streams,
    }
}

#[test]
fn loads_polymarket_contract() {
    let contract =
        VenueContract::load_and_validate(std::path::Path::new("contracts/polymarket.toml"))
            .expect("polymarket contract should load");

    assert_eq!(contract.venue, "polymarket");
    assert_eq!(contract.schema_version, 1);
    assert_eq!(contract.streams.len(), 7);
}

#[test]
fn rejects_contract_missing_stream_class() {
    let mut streams = base_polymarket_streams();
    streams.remove("quotes");
    let contract = make_contract(streams);
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("contract missing required stream class"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_unsupported_with_required_policy() {
    let mut streams = base_polymarket_streams();
    streams.get_mut("mark_prices").unwrap().policy = Some(Policy::Required);
    let contract = make_contract(streams);
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported capability cannot have policy"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_derived_without_derived_from() {
    let mut streams = base_polymarket_streams();
    streams.get_mut("quotes").unwrap().provenance = Provenance::Derived;
    let contract = make_contract(streams);
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("derived provenance requires non-empty derived_from"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_wrong_schema_version() {
    let mut contract = make_contract(base_polymarket_streams());
    contract.schema_version = 99;
    let err = contract.validate().unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported contract schema_version"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_malformed_toml() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.toml");
    std::fs::write(&path, "this is not valid toml [[[").unwrap();
    let err = VenueContract::load_and_validate(&path).unwrap_err();
    assert!(
        err.to_string().contains("failed to parse contract"),
        "unexpected error: {err}"
    );
}
