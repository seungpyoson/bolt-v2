use bolt_v2::{
    bolt_v3_config::{ReferenceStreamBlock, load_bolt_v3_config},
    bolt_v3_reference_producer::BoltV3ReferenceActorPlan,
    config::ReferenceVenueKind,
};

mod support;

fn orderbook_stream_fixture() -> ReferenceStreamBlock {
    let toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3_reference_producer/orderbook_stream.toml",
    ))
    .expect("reference producer fixture should be readable");
    toml::from_str(&toml).expect("reference producer fixture should parse")
}

#[test]
fn reference_actor_plan_uses_configured_data_client_id() {
    let loaded = load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("bolt-v3 root should load");
    let stream = orderbook_stream_fixture();

    let plan = BoltV3ReferenceActorPlan::from_stream(&loaded.root, "eth_usd", &stream)
        .expect("orderbook stream should build reference actor plan");

    assert_eq!(plan.config.publish_topic, "reference.eth_usd");
    assert_eq!(plan.config.min_publish_interval_ms, 100);
    assert_eq!(plan.config.venue_subscriptions.len(), 1);
    assert_eq!(
        plan.config.venue_subscriptions[0].venue_name,
        "eth_usd_orderbook_anchor"
    );
    assert_eq!(
        plan.config.venue_subscriptions[0].client_id.to_string(),
        "binance_reference"
    );
    assert_eq!(plan.venue_cfgs.len(), 1);
    assert_eq!(plan.venue_cfgs[0].name, "eth_usd_orderbook_anchor");
    assert_eq!(plan.venue_cfgs[0].kind, ReferenceVenueKind::Binance);
    assert_eq!(plan.venue_cfgs[0].instrument_id, "ETHUSDT.BINANCE");
}

#[test]
fn existing_eth_stream_builds_chainlink_reference_actor_plan_from_toml() {
    let loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing strategy root should load");
    let stream = loaded
        .root
        .reference_streams
        .get("eth_usd")
        .expect("existing root should define eth_usd stream");

    let plan = BoltV3ReferenceActorPlan::from_stream(&loaded.root, "eth_usd", stream)
        .expect("existing eth stream should build Chainlink reference actor plan");

    assert_eq!(plan.config.publish_topic, "reference.eth_usd");
    assert_eq!(
        plan.config.venue_subscriptions[0].client_id.to_string(),
        "chainlink_reference"
    );
    assert_eq!(plan.venue_cfgs[0].kind, ReferenceVenueKind::Chainlink);
    let chainlink = plan.venue_cfgs[0]
        .chainlink
        .as_ref()
        .expect("Chainlink producer input should carry feed config");
    assert_eq!(
        chainlink.feed_id,
        "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
    );
    assert_eq!(chainlink.price_scale, 8);
}
