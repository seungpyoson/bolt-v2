use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::{
        REFERENCE_STREAM_ID_PARAMETER, ReferenceSourceType, ReferenceStreamBlock,
        ReferenceStreamInputBlock, load_bolt_v3_config,
    },
    bolt_v3_reference_policy::BoltV3ReferenceStreamPolicy,
    platform::reference::{
        ReferenceObservation, VenueHealth, VenueKind, reference_auto_disable_reason,
    },
};
use serde::Deserialize;

mod support;

struct PolicyFixture {
    stream_id: String,
    stream: ReferenceStreamBlock,
    scenarios: PolicyScenariosFixture,
}

#[derive(Debug, Deserialize)]
struct PolicyScenariosFixture {
    weighted_fresh_sources: PolicyObservationScenario,
    stale_or_disabled_sources: PolicyObservationScenario,
    derived_disable_map: PolicyObservationScenario,
    manual_disable: ManualDisableScenario,
    existing_strategy_observed_input: ObservedInputScenario,
}

#[derive(Debug, Deserialize)]
struct PolicyObservationScenario {
    oracle_price: f64,
    orderbook_bid: f64,
    orderbook_ask: f64,
    observation_ts_ms: u64,
}

#[derive(Debug, Deserialize)]
struct ManualDisableScenario {
    reason: String,
}

#[derive(Debug, Deserialize)]
struct ObservedInputScenario {
    observed_price: f64,
    observation_ts_ms: u64,
}

fn policy_fixture() -> PolicyFixture {
    let fixture_path =
        support::repo_path("tests/fixtures/bolt_v3_reference_policy/eth_usd_stream.toml");
    let scenarios_path =
        support::repo_path("tests/fixtures/bolt_v3_reference_policy/scenarios.toml");
    let stream_id = fixture_path
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .and_then(|file_name| file_name.strip_suffix("_stream.toml"))
        .expect("reference policy fixture filename should encode stream id")
        .to_string();
    let toml =
        std::fs::read_to_string(fixture_path).expect("reference policy fixture should be readable");
    let stream = toml::from_str(&toml).expect("reference policy fixture should parse");
    let scenarios_toml = std::fs::read_to_string(scenarios_path)
        .expect("reference policy scenarios fixture should be readable");
    let scenarios =
        toml::from_str(&scenarios_toml).expect("reference policy scenarios fixture should parse");

    PolicyFixture {
        stream_id,
        stream,
        scenarios,
    }
}

fn input_by_type(
    stream: &ReferenceStreamBlock,
    source_type: ReferenceSourceType,
) -> &ReferenceStreamInputBlock {
    stream
        .inputs
        .iter()
        .find(|input| input.source_type == source_type)
        .expect("reference stream fixture should include requested source type")
}

fn observations_from_stream(
    stream: &ReferenceStreamBlock,
    oracle_price: f64,
    orderbook_bid: f64,
    orderbook_ask: f64,
    ts_ms: u64,
) -> BTreeMap<String, ReferenceObservation> {
    stream
        .inputs
        .iter()
        .map(|input| {
            (
                input.source_id.clone(),
                observation_from_input(input, oracle_price, orderbook_bid, orderbook_ask, ts_ms),
            )
        })
        .collect()
}

fn observation_from_input(
    input: &ReferenceStreamInputBlock,
    oracle_price: f64,
    orderbook_bid: f64,
    orderbook_ask: f64,
    ts_ms: u64,
) -> ReferenceObservation {
    match input.source_type {
        ReferenceSourceType::Oracle => ReferenceObservation::Oracle {
            venue_name: input.source_id.clone(),
            instrument_id: input.instrument_id.clone(),
            price: oracle_price,
            ts_ms,
            observed_ts_ms: ts_ms + 1,
        },
        ReferenceSourceType::Orderbook => ReferenceObservation::Orderbook {
            venue_name: input.source_id.clone(),
            instrument_id: input.instrument_id.clone(),
            bid: orderbook_bid,
            ask: orderbook_ask,
            ts_ms,
            observed_ts_ms: ts_ms + 1,
        },
    }
}

#[test]
fn v3_reference_stream_policy_fuses_weighted_fresh_sources() {
    let fixture = policy_fixture();
    let stream = &fixture.stream;
    let scenario = &fixture.scenarios.weighted_fresh_sources;
    let oracle_input = input_by_type(stream, ReferenceSourceType::Oracle);
    let orderbook_input = input_by_type(stream, ReferenceSourceType::Orderbook);
    let oracle_price = scenario.oracle_price;
    let orderbook_bid = scenario.orderbook_bid;
    let orderbook_ask = scenario.orderbook_ask;
    let orderbook_mid = (orderbook_bid + orderbook_ask) / 2.0;
    let total_weight = stream
        .inputs
        .iter()
        .map(|input| input.base_weight)
        .sum::<f64>();
    let expected_fair_value = (oracle_price * oracle_input.base_weight
        + orderbook_mid * orderbook_input.base_weight)
        / total_weight;
    let policy = BoltV3ReferenceStreamPolicy::from_stream(&fixture.stream_id, stream)
        .expect("reference stream policy should build from valid fixture");
    let observation_ts_ms = scenario.observation_ts_ms;
    let latest = observations_from_stream(
        stream,
        oracle_price,
        orderbook_bid,
        orderbook_ask,
        observation_ts_ms,
    );

    let snapshot = policy.fuse_snapshot(
        observation_ts_ms + stream.min_publish_interval_milliseconds,
        &latest,
        &BTreeMap::new(),
    );

    assert_eq!(snapshot.topic, stream.publish_topic);
    assert_eq!(snapshot.fair_value, Some(expected_fair_value));
    assert_eq!(snapshot.confidence, 1.0);
    assert_eq!(snapshot.venues.len(), 2);
    assert_eq!(snapshot.venues[0].venue_kind, VenueKind::Oracle);
    assert_eq!(snapshot.venues[1].venue_kind, VenueKind::Orderbook);
}

#[test]
fn v3_reference_stream_policy_excludes_stale_or_disabled_sources() {
    let fixture = policy_fixture();
    let stream = &fixture.stream;
    let scenario = &fixture.scenarios.stale_or_disabled_sources;
    let oracle_input = input_by_type(stream, ReferenceSourceType::Oracle);
    let orderbook_input = input_by_type(stream, ReferenceSourceType::Orderbook);
    let oracle_price = scenario.oracle_price;
    let orderbook_bid = scenario.orderbook_bid;
    let orderbook_ask = scenario.orderbook_ask;
    let total_weight = stream
        .inputs
        .iter()
        .map(|input| input.base_weight)
        .sum::<f64>();
    let policy = BoltV3ReferenceStreamPolicy::from_stream(&fixture.stream_id, stream)
        .expect("reference stream policy should build from valid fixture");
    let observation_ts_ms = scenario.observation_ts_ms;
    let latest = observations_from_stream(
        stream,
        oracle_price,
        orderbook_bid,
        orderbook_ask,
        observation_ts_ms,
    );
    let manual_disable_reason = fixture.scenarios.manual_disable.reason.clone();
    let disabled = BTreeMap::from([(
        orderbook_input.source_id.clone(),
        manual_disable_reason.clone(),
    )]);

    let snapshot = policy.fuse_snapshot(
        observation_ts_ms + stream.min_publish_interval_milliseconds,
        &latest,
        &disabled,
    );

    assert_eq!(snapshot.fair_value, Some(oracle_price));
    assert_eq!(snapshot.confidence, oracle_input.base_weight / total_weight);
    assert_eq!(
        snapshot.venues[1].health,
        VenueHealth::Disabled {
            reason: manual_disable_reason
        }
    );
    assert_eq!(snapshot.venues[1].effective_weight, 0.0);
}

#[test]
fn v3_reference_stream_policy_derives_disable_map_before_fusion() {
    let fixture = policy_fixture();
    let stream = &fixture.stream;
    let scenario = &fixture.scenarios.derived_disable_map;
    let oracle_input = input_by_type(stream, ReferenceSourceType::Oracle);
    let orderbook_input = input_by_type(stream, ReferenceSourceType::Orderbook);
    let oracle_price = scenario.oracle_price;
    let orderbook_bid = scenario.orderbook_bid;
    let orderbook_ask = scenario.orderbook_ask;
    let total_weight = stream
        .inputs
        .iter()
        .map(|input| input.base_weight)
        .sum::<f64>();
    let policy = BoltV3ReferenceStreamPolicy::from_stream(&fixture.stream_id, stream)
        .expect("reference stream policy should build from valid fixture");
    let orderbook_ts_ms = scenario.observation_ts_ms;
    let disabled_age_ms =
        orderbook_input.disable_after_milliseconds + stream.min_publish_interval_milliseconds;
    let now_ms = orderbook_ts_ms + disabled_age_ms;
    let oracle_ts_ms = now_ms - stream.min_publish_interval_milliseconds;
    let latest = BTreeMap::from([
        (
            oracle_input.source_id.clone(),
            observation_from_input(
                oracle_input,
                oracle_price,
                orderbook_bid,
                orderbook_ask,
                oracle_ts_ms,
            ),
        ),
        (
            orderbook_input.source_id.clone(),
            observation_from_input(
                orderbook_input,
                oracle_price,
                orderbook_bid,
                orderbook_ask,
                orderbook_ts_ms,
            ),
        ),
    ]);

    let disabled = policy.disabled_sources(now_ms, &latest);
    let snapshot = policy.fuse_snapshot_with_source_health(now_ms, &latest);
    let expected_disable_reason = reference_auto_disable_reason(disabled_age_ms);

    assert_eq!(
        disabled.get(&orderbook_input.source_id).map(String::as_str),
        Some(expected_disable_reason.as_str())
    );
    assert!(!disabled.contains_key(&oracle_input.source_id));
    assert_eq!(snapshot.fair_value, Some(oracle_price));
    assert_eq!(snapshot.confidence, oracle_input.base_weight / total_weight);
    assert_eq!(
        snapshot.venues[1].health,
        VenueHealth::Disabled {
            reason: expected_disable_reason
        }
    );
}

#[test]
fn existing_strategy_root_stream_uses_v3_reference_policy() {
    let fixture = policy_fixture();
    let scenario = &fixture.scenarios.existing_strategy_observed_input;
    let loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing strategy root should load");
    let stream_id = loaded
        .strategies
        .first()
        .and_then(|strategy| {
            strategy
                .config
                .parameters
                .get(REFERENCE_STREAM_ID_PARAMETER)
        })
        .and_then(toml::Value::as_str)
        .expect("existing strategy should select reference stream from TOML");
    let stream = loaded
        .root
        .reference_streams
        .get(stream_id)
        .expect("existing strategy root should define selected stream");
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
    let policy = BoltV3ReferenceStreamPolicy::from_stream(stream_id, stream)
        .expect("existing strategy stream should build policy");
    let observed_price = scenario.observed_price;
    let observation_ts_ms = scenario.observation_ts_ms;
    let latest = BTreeMap::from([(
        observed_input.source_id.clone(),
        ReferenceObservation::Oracle {
            venue_name: observed_input.source_id.clone(),
            instrument_id: observed_input.instrument_id.clone(),
            price: observed_price,
            ts_ms: observation_ts_ms,
            observed_ts_ms: observation_ts_ms + 1,
        },
    )]);

    let snapshot = policy.fuse_snapshot(
        observation_ts_ms + stream.min_publish_interval_milliseconds,
        &latest,
        &BTreeMap::new(),
    );

    assert_eq!(snapshot.topic, stream.publish_topic);
    assert_eq!(snapshot.fair_value, Some(observed_price));
    assert_eq!(snapshot.confidence, observed_weight / total_weight);
    assert_eq!(snapshot.venues[0].venue_name, observed_input.source_id);
}
