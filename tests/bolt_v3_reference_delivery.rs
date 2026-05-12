//! Mock-only proof that a bolt-v3 registered ReferenceActor can publish
//! ReferenceSnapshot through NT lifecycle start.
//!
//! This does not use real providers, does not register strategies, does not
//! submit orders, and does not claim venue/live readiness.

mod support;

use std::{
    any::Any,
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};

use bolt_v2::{
    bolt_v3_config::{
        LoadedBoltV3Config, REFERENCE_STREAM_ID_PARAMETER, ReferenceStreamBlock,
        ReferenceStreamInputBlock, load_bolt_v3_config,
    },
    bolt_v3_live_node::make_bolt_v3_live_node_builder,
    bolt_v3_reference_actor_registration::register_bolt_v3_reference_actors,
    clients::chainlink::{ChainlinkOracleUpdate, chainlink_topic_for_venue},
    platform::reference::ReferenceSnapshot,
};
use nautilus_common::msgbus::{self, MessageBus, ShareableMessageHandler};
use nautilus_core::UnixNanos;
use nautilus_live::node::NodeState;
use nautilus_model::data::CustomData;
use serde::Deserialize;
use support::{MockDataClientConfig, MockDataClientFactory};

static LIVE_NODE_TEST_LOCK: Mutex<()> = Mutex::new(());
static REFERENCE_DELIVERY_OBSERVATION: OnceLock<ReferenceDeliveryObservationFixture> =
    OnceLock::new();

#[derive(Debug, Clone, Deserialize)]
struct ReferenceDeliveryObservationFixture {
    price: f64,
}

fn live_node_test_guard() -> MutexGuard<'static, ()> {
    LIVE_NODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn existing_strategy_root_fixture() -> std::path::PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml")
}

fn selected_reference_stream<'a>(
    loaded: &'a LoadedBoltV3Config,
) -> (&'a str, &'a ReferenceStreamBlock) {
    let strategy = loaded
        .strategies
        .first()
        .expect("fixture should define one strategy");
    let parameters = strategy
        .config
        .parameters
        .as_table()
        .expect("strategy parameters should be a table");
    let stream_id = parameters
        .get(REFERENCE_STREAM_ID_PARAMETER)
        .and_then(toml::Value::as_str)
        .expect("strategy should select reference stream from TOML");
    let stream = loaded
        .root
        .reference_streams
        .get(stream_id)
        .expect("selected stream should exist in root TOML");
    (stream_id, stream)
}

fn single_chainlink_input(stream: &ReferenceStreamBlock) -> &ReferenceStreamInputBlock {
    let input = stream
        .inputs
        .first()
        .expect("fixture stream should define one input");
    input
        .provider_config
        .as_ref()
        .expect("fixture input should carry provider-owned feed config");
    input
}

fn collect_snapshots(topic: &str) -> Rc<RefCell<Vec<ReferenceSnapshot>>> {
    let snapshots: Rc<RefCell<Vec<ReferenceSnapshot>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&snapshots);
    let handler = ShareableMessageHandler::from_any(move |message: &dyn Any| {
        if let Some(snapshot) = message.downcast_ref::<ReferenceSnapshot>() {
            sink.borrow_mut().push(snapshot.clone());
        }
    });
    msgbus::subscribe_any(topic.into(), handler, None);
    snapshots
}

fn current_time_milliseconds() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after UNIX epoch");
    u64::try_from(now.as_millis()).expect("current time millis should fit in u64")
}

fn reference_delivery_observation_fixture() -> &'static ReferenceDeliveryObservationFixture {
    REFERENCE_DELIVERY_OBSERVATION.get_or_init(|| {
        let fixture_path =
            support::repo_path("tests/fixtures/bolt_v3_reference_delivery/observation.toml");
        let toml =
            std::fs::read_to_string(fixture_path).expect("reference delivery fixture should read");
        toml::from_str(&toml).expect("reference delivery fixture should parse")
    })
}

#[test]
fn live_node_start_drives_registered_reference_actor_to_publish_snapshot() {
    let _guard = live_node_test_guard();
    *msgbus::get_message_bus().borrow_mut() = MessageBus::default();

    let mut loaded =
        load_bolt_v3_config(&existing_strategy_root_fixture()).expect("v3 fixture should load");
    loaded.root.nautilus.delay_post_stop_seconds = 0;
    loaded.root.nautilus.timeout_disconnection_seconds = 1;
    let (stream_id, stream) = selected_reference_stream(&loaded);
    let input = single_chainlink_input(stream);
    let data_client_id = input
        .data_client_id
        .as_deref()
        .expect("fixture input should select configured data client");
    let data_client_venue = loaded
        .root
        .clients
        .get(data_client_id)
        .expect("selected data client should exist in root TOML")
        .venue
        .as_str();

    let mut node = make_bolt_v3_live_node_builder(&loaded)
        .expect("v3 builder should construct from fixture")
        .add_data_client(
            Some(data_client_id.to_string()),
            Box::new(MockDataClientFactory),
            Box::new(MockDataClientConfig::new(data_client_id, data_client_venue)),
        )
        .expect("mock Chainlink data client should register on builder")
        .build()
        .expect("LiveNode should build with mock Chainlink data client");

    let registered = register_bolt_v3_reference_actors(&mut node, &loaded)
        .expect("selected reference actor should register on mock LiveNode");
    assert_eq!(registered, vec![stream_id.to_string()]);

    let snapshots = collect_snapshots(stream.publish_topic.as_str());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for LiveNode start proof");

    runtime.block_on(async {
        node.start()
            .await
            .expect("mock-only LiveNode start should succeed");
        assert_eq!(node.state(), NodeState::Running);

        let observed_ms = current_time_milliseconds();
        let observation = reference_delivery_observation_fixture();
        let custom = CustomData::new(
            Arc::new(ChainlinkOracleUpdate {
                venue_name: input.source_id.clone(),
                instrument_id: input.instrument_id.clone(),
                price: observation.price,
                round_id: observed_ms.to_string(),
                updated_at_ms: observed_ms,
                ts_init: UnixNanos::from(observed_ms.saturating_mul(1_000_000)),
            }),
            bolt_v2::clients::chainlink::chainlink_data_type_for_venue(
                input.source_id.as_str(),
                input.instrument_id.as_str(),
            ),
        );
        msgbus::publish_any(
            chainlink_topic_for_venue(input.source_id.as_str(), input.instrument_id.as_str()),
            &custom,
        );

        node.stop()
            .await
            .expect("mock-only LiveNode stop should succeed");
    });

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].topic, stream.publish_topic);
    assert_eq!(
        snapshots[0].fair_value,
        Some(reference_delivery_observation_fixture().price)
    );
}
