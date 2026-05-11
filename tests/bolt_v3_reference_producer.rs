use bolt_v2::{
    bolt_v3_config::{REFERENCE_STREAM_ID_PARAMETER, ReferenceStreamBlock, load_bolt_v3_config},
    bolt_v3_reference_producer::BoltV3ReferenceActorPlan,
    config::ReferenceVenueKind,
};

mod support;

struct StreamFixture {
    stream_id: String,
    stream: ReferenceStreamBlock,
}

fn orderbook_stream_fixture() -> StreamFixture {
    let fixture_path =
        support::repo_path("tests/fixtures/bolt_v3_reference_producer/orderbook_stream.toml");
    let stream_id = fixture_path
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .and_then(|file_name| file_name.strip_suffix("_stream.toml"))
        .expect("reference producer fixture filename should encode stream id")
        .to_string();
    let toml = std::fs::read_to_string(fixture_path)
        .expect("reference producer fixture should be readable");
    let stream = toml::from_str(&toml).expect("reference producer fixture should parse");

    StreamFixture { stream_id, stream }
}

#[test]
fn reference_actor_plan_uses_configured_data_client_id() {
    let loaded = load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("bolt-v3 root should load");
    let fixture = orderbook_stream_fixture();
    let stream = &fixture.stream;
    let input = stream
        .inputs
        .first()
        .expect("fixture stream should define one input");
    let expected_data_client_id = input
        .data_client_id
        .as_deref()
        .expect("fixture input should configure data_client_id");

    let plan = BoltV3ReferenceActorPlan::from_stream(&loaded.root, &fixture.stream_id, stream)
        .expect("orderbook stream should build reference actor plan");

    assert_eq!(plan.config.publish_topic, stream.publish_topic);
    assert_eq!(
        plan.config.min_publish_interval_ms,
        stream.min_publish_interval_milliseconds
    );
    assert_eq!(plan.config.venue_subscriptions.len(), 1);
    assert_eq!(
        plan.config.venue_subscriptions[0].venue_name,
        input.source_id
    );
    assert_eq!(
        plan.config.venue_subscriptions[0].client_id.to_string(),
        expected_data_client_id
    );
    assert_eq!(plan.venue_cfgs.len(), 1);
    assert_eq!(plan.venue_cfgs[0].name, input.source_id);
    assert_eq!(plan.venue_cfgs[0].kind, ReferenceVenueKind::Binance);
    assert_eq!(plan.venue_cfgs[0].instrument_id, input.instrument_id);
}

#[test]
fn existing_eth_stream_builds_chainlink_reference_actor_plan_from_toml() {
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
        .expect("existing root should define selected reference stream");
    let input = stream
        .inputs
        .first()
        .expect("existing stream should define one input");
    let expected_data_client_id = input
        .data_client_id
        .as_deref()
        .expect("existing eth stream should configure primary data_client_id");

    let plan = BoltV3ReferenceActorPlan::from_stream(&loaded.root, stream_id, stream)
        .expect("existing eth stream should build Chainlink reference actor plan");

    assert_eq!(plan.config.publish_topic, stream.publish_topic);
    assert_eq!(
        plan.config.venue_subscriptions[0].client_id.to_string(),
        expected_data_client_id
    );
    assert_eq!(plan.venue_cfgs[0].kind, ReferenceVenueKind::Chainlink);
    let chainlink = plan.venue_cfgs[0]
        .chainlink
        .as_ref()
        .expect("Chainlink producer input should carry feed config");
    let provider_config = input
        .provider_config
        .as_ref()
        .expect("Chainlink fixture input should carry provider config");
    let expected_feed_id = provider_config
        .get("feed_id")
        .and_then(toml::Value::as_str)
        .expect("Chainlink provider config should define feed_id");
    let expected_price_scale = provider_config
        .get("price_scale")
        .and_then(toml::Value::as_integer)
        .and_then(|value| u8::try_from(value).ok())
        .expect("Chainlink provider config should define u8 price_scale");
    assert_eq!(chainlink.feed_id, expected_feed_id);
    assert_eq!(chainlink.price_scale, expected_price_scale);
}
