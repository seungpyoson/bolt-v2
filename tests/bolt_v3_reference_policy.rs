use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::{ReferenceStreamBlock, load_bolt_v3_config},
    bolt_v3_reference_policy::BoltV3ReferenceStreamPolicy,
    platform::reference::{ReferenceObservation, VenueHealth, VenueKind},
};

mod support;

fn policy_fixture() -> ReferenceStreamBlock {
    let toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3_reference_policy/eth_usd_stream.toml",
    ))
    .expect("reference policy fixture should be readable");
    toml::from_str(&toml).expect("reference policy fixture should parse")
}

#[test]
fn v3_reference_stream_policy_fuses_weighted_fresh_sources() {
    let stream = policy_fixture();
    let policy = BoltV3ReferenceStreamPolicy::from_stream("eth_usd", &stream)
        .expect("reference stream policy should build from valid fixture");
    let latest = BTreeMap::from([
        (
            "eth_usd_oracle_anchor".to_string(),
            ReferenceObservation::Oracle {
                venue_name: "eth_usd_oracle_anchor".to_string(),
                instrument_id: "ETHUSD.CHAINLINK".to_string(),
                price: 100.0,
                ts_ms: 1_000,
                observed_ts_ms: 1_005,
            },
        ),
        (
            "eth_usd_fast_orderbook".to_string(),
            ReferenceObservation::Orderbook {
                venue_name: "eth_usd_fast_orderbook".to_string(),
                instrument_id: "ETHUSDT.POLYREFERENCE".to_string(),
                bid: 119.0,
                ask: 121.0,
                ts_ms: 1_000,
                observed_ts_ms: 1_006,
            },
        ),
    ]);

    let snapshot = policy.fuse_snapshot(1_100, &latest, &BTreeMap::new());

    assert_eq!(snapshot.topic, "reference.eth_usd");
    assert_eq!(snapshot.fair_value, Some(106.66666666666667));
    assert_eq!(snapshot.confidence, 1.0);
    assert_eq!(snapshot.venues.len(), 2);
    assert_eq!(snapshot.venues[0].venue_kind, VenueKind::Oracle);
    assert_eq!(snapshot.venues[1].venue_kind, VenueKind::Orderbook);
}

#[test]
fn v3_reference_stream_policy_excludes_stale_or_disabled_sources() {
    let stream = policy_fixture();
    let policy = BoltV3ReferenceStreamPolicy::from_stream("eth_usd", &stream)
        .expect("reference stream policy should build from valid fixture");
    let latest = BTreeMap::from([
        (
            "eth_usd_oracle_anchor".to_string(),
            ReferenceObservation::Oracle {
                venue_name: "eth_usd_oracle_anchor".to_string(),
                instrument_id: "ETHUSD.CHAINLINK".to_string(),
                price: 100.0,
                ts_ms: 1_000,
                observed_ts_ms: 1_005,
            },
        ),
        (
            "eth_usd_fast_orderbook".to_string(),
            ReferenceObservation::Orderbook {
                venue_name: "eth_usd_fast_orderbook".to_string(),
                instrument_id: "ETHUSDT.POLYREFERENCE".to_string(),
                bid: 119.0,
                ask: 121.0,
                ts_ms: 1_000,
                observed_ts_ms: 1_006,
            },
        ),
    ]);
    let disabled = BTreeMap::from([(
        "eth_usd_fast_orderbook".to_string(),
        "manual disable".to_string(),
    )]);

    let snapshot = policy.fuse_snapshot(1_100, &latest, &disabled);

    assert_eq!(snapshot.fair_value, Some(100.0));
    assert_eq!(snapshot.confidence, 2.0 / 3.0);
    assert_eq!(
        snapshot.venues[1].health,
        VenueHealth::Disabled {
            reason: "manual disable".to_string()
        }
    );
    assert_eq!(snapshot.venues[1].effective_weight, 0.0);
}

#[test]
fn existing_strategy_root_stream_uses_v3_reference_policy() {
    let loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing strategy root should load");
    let stream = loaded
        .root
        .reference_streams
        .get("eth_usd")
        .expect("existing strategy root should define eth_usd stream");
    let observed_input = stream
        .inputs
        .iter()
        .find(|input| {
            matches!(
                input.source_type,
                bolt_v2::bolt_v3_config::ReferenceSourceType::Oracle
            )
        })
        .expect("existing strategy stream should define an oracle source");
    let observed_weight = observed_input.base_weight;
    let total_weight = stream
        .inputs
        .iter()
        .map(|input| input.base_weight)
        .sum::<f64>();
    let policy = BoltV3ReferenceStreamPolicy::from_stream("eth_usd", stream)
        .expect("existing strategy stream should build policy");
    let latest = BTreeMap::from([(
        observed_input.source_id.clone(),
        ReferenceObservation::Oracle {
            venue_name: observed_input.source_id.clone(),
            instrument_id: observed_input.instrument_id.clone(),
            price: 3_100.0,
            ts_ms: 1_000,
            observed_ts_ms: 1_004,
        },
    )]);

    let snapshot = policy.fuse_snapshot(1_100, &latest, &BTreeMap::new());

    assert_eq!(snapshot.topic, stream.publish_topic);
    assert_eq!(snapshot.fair_value, Some(3_100.0));
    assert_eq!(snapshot.confidence, observed_weight / total_weight);
    assert_eq!(snapshot.venues[0].venue_name, observed_input.source_id);
}
