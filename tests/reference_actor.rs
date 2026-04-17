use std::{any::Any, cell::RefCell, rc::Rc, sync::Arc};

use bolt_v2::{
    config::{ReferenceVenueEntry, ReferenceVenueKind},
    platform::{
        reference::{ReferenceSnapshot, VenueHealth},
        reference_actor::{ReferenceActor, ReferenceActorConfig, ReferenceSubscription},
    },
};
use nautilus_common::{
    actor::{
        Component, DataActor,
        registry::{get_actor_unchecked, register_actor},
    },
    cache::Cache,
    clock::TestClock,
    msgbus::{self, MessageBus, ShareableMessageHandler, switchboard::get_quotes_topic},
    runner::{SyncDataCommandSender, replace_data_cmd_sender},
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    identifiers::{ActorId, ClientId, InstrumentId, TraderId},
    stubs::TestDefault,
    types::{Price, Quantity},
};

fn reference_venue(
    name: &str,
    instrument_id: &str,
    kind: ReferenceVenueKind,
    base_weight: f64,
) -> ReferenceVenueEntry {
    ReferenceVenueEntry {
        name: name.into(),
        kind,
        instrument_id: instrument_id.into(),
        base_weight,
        stale_after_ms: 1_000,
        disable_after_ms: 5_000,
    }
}

fn subscription(venue_name: &str, instrument_id: &str, client_id: &str) -> ReferenceSubscription {
    ReferenceSubscription {
        venue_name: venue_name.into(),
        instrument_id: InstrumentId::from(instrument_id),
        client_id: ClientId::from(client_id),
    }
}

fn quote(instrument_id: &str, bid: &str, ask: &str, ts_ms: u64) -> nautilus_model::data::QuoteTick {
    let ts = UnixNanos::from(ts_ms * 1_000_000);
    nautilus_model::data::QuoteTick::new(
        InstrumentId::from(instrument_id),
        Price::from(bid),
        Price::from(ask),
        Quantity::from("1"),
        Quantity::from("1"),
        ts,
        ts,
    )
}

fn register_reference_actor(
    config: ReferenceActorConfig,
    venue_cfgs: Vec<ReferenceVenueEntry>,
) -> nautilus_model::identifiers::ActorId {
    replace_data_cmd_sender(Arc::new(SyncDataCommandSender));
    *msgbus::get_message_bus().borrow_mut() = MessageBus::default();

    let clock = Rc::new(RefCell::new(TestClock::new()));
    let cache = Rc::new(RefCell::new(Cache::new(None, None)));
    let trader_id = TraderId::test_default();

    let mut actor = ReferenceActor::new(config, venue_cfgs);
    actor.register(trader_id, clock, cache).unwrap();
    let actor_id = actor.actor_id();
    register_actor(actor);
    actor_id
}

fn collect_snapshots(topic: &str) -> Rc<RefCell<Vec<ReferenceSnapshot>>> {
    let snapshots = Rc::new(RefCell::new(Vec::new()));
    let captured = Rc::clone(&snapshots);
    let handler = ShareableMessageHandler::from_any(move |message: &dyn Any| {
        if let Some(snapshot) = message.downcast_ref::<ReferenceSnapshot>() {
            captured.borrow_mut().push(snapshot.clone());
        }
    });
    msgbus::subscribe_any(topic.into(), handler, None);
    snapshots
}

fn set_actor_clock_ms(actor: &mut ReferenceActor, ts_ms: u64) {
    let mut clock = actor.clock();
    let test_clock = (&mut *clock as &mut dyn Any)
        .downcast_mut::<TestClock>()
        .expect("reference actor should be registered with TestClock");
    test_clock.set_time(UnixNanos::from(ts_ms * 1_000_000));
}

fn advance_actor_clock_to(actor_id: ActorId, ts_ms: u64) {
    let clock = {
        let actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
        actor.clock_rc()
    };

    let handlers = {
        let mut clock = clock.borrow_mut();
        let test_clock = (&mut *clock as &mut dyn Any)
            .downcast_mut::<TestClock>()
            .expect("reference actor should be registered with TestClock");
        let events = test_clock.advance_time(UnixNanos::from(ts_ms * 1_000_000), true);
        test_clock.match_handlers(events)
    };

    for handler in handlers {
        handler.callback.call(handler.event);
    }
}

fn actor_config(
    publish_topic: &str,
    min_publish_interval_ms: u64,
    venue_subscriptions: Vec<ReferenceSubscription>,
) -> ReferenceActorConfig {
    ReferenceActorConfig {
        base: nautilus_common::actor::DataActorConfig {
            actor_id: Some(ActorId::from(
                format!("REFERENCE-ACTOR-{}", UUID4::new()).as_str(),
            )),
            ..Default::default()
        },
        publish_topic: publish_topic.into(),
        min_publish_interval_ms,
        venue_subscriptions,
    }
}

#[test]
fn reference_actor_subscribes_to_quotes_for_configured_venues() {
    let publish_topic = "platform.reference.test.start";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            0,
            vec![
                subscription("BINANCE", "BTCUSDT.BINANCE", "BINANCE"),
                subscription("BYBIT", "BTCUSDT.BYBIT", "BYBIT"),
            ],
        ),
        vec![
            reference_venue(
                "BINANCE",
                "BTCUSDT.BINANCE",
                ReferenceVenueKind::Binance,
                0.5,
            ),
            reference_venue("BYBIT", "BTCUSDT.BYBIT", ReferenceVenueKind::Bybit, 0.5),
        ],
    );
    let snapshots = collect_snapshots(publish_topic);

    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BINANCE")),
        &quote("BTCUSDT.BINANCE", "99.0", "101.0", 900),
    );
    assert!(
        snapshots.borrow().is_empty(),
        "actor should not receive quotes before start"
    );

    let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
    actor.start().unwrap();

    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BINANCE")),
        &quote("BTCUSDT.BINANCE", "99.0", "101.0", 1_000),
    );
    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BYBIT")),
        &quote("BTCUSDT.BYBIT", "100.0", "102.0", 1_100),
    );
    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.KRAKEN")),
        &quote("BTCUSDT.KRAKEN", "1.0", "2.0", 1_200),
    );

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 2);
    assert!(snapshots.iter().all(|snapshot| {
        snapshot
            .venues
            .iter()
            .any(|venue| venue.venue_name == "BINANCE")
    }));
    assert!(snapshots.iter().all(|snapshot| {
        snapshot
            .venues
            .iter()
            .any(|venue| venue.venue_name == "BYBIT")
    }));
}

#[test]
fn quote_events_update_latest_observation_and_publish_snapshot() {
    let publish_topic = "platform.reference.test.quote";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            0,
            vec![subscription("BINANCE", "BTCUSDT.BINANCE", "BINANCE")],
        ),
        vec![reference_venue(
            "BINANCE",
            "BTCUSDT.BINANCE",
            ReferenceVenueKind::Binance,
            1.0,
        )],
    );
    let snapshots = collect_snapshots(publish_topic);

    let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
    actor.start().unwrap();

    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BINANCE")),
        &quote("BTCUSDT.BINANCE", "99.5", "100.5", 1_234),
    );

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 1);

    let snapshot = &snapshots[0];
    assert_eq!(snapshot.topic, publish_topic);
    assert_eq!(snapshot.fair_value, Some(100.0));
    assert_eq!(snapshot.confidence, 1.0);
    assert_eq!(snapshot.venues.len(), 1);
    assert_eq!(snapshot.venues[0].venue_name, "BINANCE");
    assert_eq!(snapshot.venues[0].observed_price, Some(100.0));
    assert_eq!(snapshot.venues[0].effective_weight, 1.0);
}

#[test]
fn min_publish_interval_throttles_snapshot_emission() {
    let publish_topic = "platform.reference.test.throttle";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            100,
            vec![subscription("BINANCE", "BTCUSDT.BINANCE", "BINANCE")],
        ),
        vec![reference_venue(
            "BINANCE",
            "BTCUSDT.BINANCE",
            ReferenceVenueKind::Binance,
            1.0,
        )],
    );
    let snapshots = collect_snapshots(publish_topic);

    let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
    actor.start().unwrap();

    set_actor_clock_ms(&mut actor, 1_000);
    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BINANCE")),
        &quote("BTCUSDT.BINANCE", "99.5", "100.5", 1_000),
    );
    set_actor_clock_ms(&mut actor, 1_050);
    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BINANCE")),
        &quote("BTCUSDT.BINANCE", "109.5", "110.5", 1_050),
    );
    set_actor_clock_ms(&mut actor, 1_100);
    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BINANCE")),
        &quote("BTCUSDT.BINANCE", "119.5", "120.5", 1_100),
    );

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].fair_value, Some(100.0));
    assert_eq!(snapshots[1].fair_value, Some(120.0));
}

#[test]
fn publish_throttle_uses_actor_clock_not_quote_timestamp() {
    let publish_topic = "platform.reference.test.clock-throttle";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            100,
            vec![subscription("BINANCE", "BTCUSDT.BINANCE", "BINANCE")],
        ),
        vec![ReferenceVenueEntry {
            name: "BINANCE".into(),
            kind: ReferenceVenueKind::Binance,
            instrument_id: "BTCUSDT.BINANCE".into(),
            base_weight: 1.0,
            stale_after_ms: 20_000,
            disable_after_ms: 25_000,
        }],
    );
    let snapshots = collect_snapshots(publish_topic);

    let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
    actor.start().unwrap();

    set_actor_clock_ms(&mut actor, 10_000);
    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BINANCE")),
        &quote("BTCUSDT.BINANCE", "99.5", "100.5", 1_000),
    );

    set_actor_clock_ms(&mut actor, 10_050);
    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BINANCE")),
        &quote("BTCUSDT.BINANCE", "109.5", "110.5", 1_500),
    );

    set_actor_clock_ms(&mut actor, 10_100);
    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BINANCE")),
        &quote("BTCUSDT.BINANCE", "119.5", "120.5", 1_600),
    );

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].fair_value, Some(100.0));
    assert_eq!(snapshots[1].fair_value, Some(120.0));
}

#[test]
fn staleness_uses_actor_clock_while_observation_timestamp_uses_quote_timestamp() {
    let publish_topic = "platform.reference.test.clock-stale";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            0,
            vec![subscription("BINANCE", "BTCUSDT.BINANCE", "BINANCE")],
        ),
        vec![reference_venue(
            "BINANCE",
            "BTCUSDT.BINANCE",
            ReferenceVenueKind::Binance,
            1.0,
        )],
    );
    let snapshots = collect_snapshots(publish_topic);

    let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
    actor.start().unwrap();
    set_actor_clock_ms(&mut actor, 2_500);

    actor
        .on_quote(&quote("BTCUSDT.BINANCE", "99.5", "100.5", 1_000))
        .unwrap();

    let stored = actor
        .latest
        .get("BINANCE")
        .expect("latest observation should be stored");
    match stored {
        bolt_v2::platform::reference::ReferenceObservation::Orderbook { ts_ms, .. } => {
            assert_eq!(*ts_ms, 1_000);
        }
        other => panic!("expected orderbook observation, got {other:?}"),
    }

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].fair_value, None);
    assert_eq!(snapshots[0].confidence, 0.0);
    assert!(snapshots[0].venues[0].stale);
}

#[test]
fn unmapped_instrument_returns_error_instead_of_succeeding() {
    let publish_topic = "platform.reference.test.unmapped";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            0,
            vec![subscription("BINANCE", "BTCUSDT.BINANCE", "BINANCE")],
        ),
        vec![reference_venue(
            "BINANCE",
            "BTCUSDT.BINANCE",
            ReferenceVenueKind::Binance,
            1.0,
        )],
    );

    let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
    actor.start().unwrap();

    let err = actor
        .on_quote(&quote("BTCUSDT.KRAKEN", "1.0", "2.0", 1_000))
        .expect_err("unmapped instrument should fail closed");
    assert!(
        err.to_string().contains("BTCUSDT.KRAKEN"),
        "error should identify the unmapped instrument: {err:#}"
    );
}

#[test]
fn out_of_order_quotes_do_not_replace_latest_or_publish() {
    let publish_topic = "platform.reference.test.out-of-order";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            0,
            vec![subscription("BINANCE", "BTCUSDT.BINANCE", "BINANCE")],
        ),
        vec![ReferenceVenueEntry {
            name: "BINANCE".into(),
            kind: ReferenceVenueKind::Binance,
            instrument_id: "BTCUSDT.BINANCE".into(),
            base_weight: 1.0,
            stale_after_ms: 20_000,
            disable_after_ms: 25_000,
        }],
    );
    let snapshots = collect_snapshots(publish_topic);

    let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
    actor.start().unwrap();

    set_actor_clock_ms(&mut actor, 5_000);
    actor
        .on_quote(&quote("BTCUSDT.BINANCE", "99.5", "100.5", 2_000))
        .unwrap();
    set_actor_clock_ms(&mut actor, 5_100);
    actor
        .on_quote(&quote("BTCUSDT.BINANCE", "89.5", "90.5", 1_999))
        .unwrap();

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].fair_value, Some(100.0));

    let stored = actor
        .latest
        .get("BINANCE")
        .expect("latest observation should be retained");
    match stored {
        bolt_v2::platform::reference::ReferenceObservation::Orderbook {
            bid, ask, ts_ms, ..
        } => {
            assert_eq!(*bid, 99.5);
            assert_eq!(*ask, 100.5);
            assert_eq!(*ts_ms, 2_000);
        }
        other => panic!("expected orderbook observation, got {other:?}"),
    }
}

#[test]
fn disabled_venue_still_appears_in_snapshot_with_zero_weight() {
    let publish_topic = "platform.reference.test.disabled";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            0,
            vec![
                subscription("BINANCE", "BTCUSDT.BINANCE", "BINANCE"),
                subscription("BYBIT", "BTCUSDT.BYBIT", "BYBIT"),
            ],
        ),
        vec![
            reference_venue(
                "BINANCE",
                "BTCUSDT.BINANCE",
                ReferenceVenueKind::Binance,
                0.6,
            ),
            reference_venue("BYBIT", "BTCUSDT.BYBIT", ReferenceVenueKind::Bybit, 0.4),
        ],
    );
    let snapshots = collect_snapshots(publish_topic);

    let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
    actor
        .disabled_mut()
        .insert("BYBIT".into(), "venue disabled".into());
    actor.start().unwrap();

    msgbus::publish_quote(
        get_quotes_topic(InstrumentId::from("BTCUSDT.BINANCE")),
        &quote("BTCUSDT.BINANCE", "100.0", "102.0", 5_000),
    );

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 1);

    let snapshot = &snapshots[0];
    assert_eq!(snapshot.venues.len(), 2);
    assert_eq!(snapshot.fair_value, Some(101.0));

    let bybit = snapshot
        .venues
        .iter()
        .find(|venue| venue.venue_name == "BYBIT")
        .expect("disabled venue should still be present");
    assert_eq!(bybit.base_weight, 0.4);
    assert_eq!(bybit.effective_weight, 0.0);
    assert_eq!(
        bybit.health,
        VenueHealth::Disabled {
            reason: "venue disabled".into(),
        }
    );
}

#[test]
fn start_without_quotes_publishes_one_no_data_snapshot_after_quiet_timer() {
    let publish_topic = "platform.reference.test.no-data";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            10,
            vec![subscription("BINANCE", "BTCUSDT.BINANCE", "BINANCE")],
        ),
        vec![ReferenceVenueEntry {
            name: "BINANCE".into(),
            kind: ReferenceVenueKind::Binance,
            instrument_id: "BTCUSDT.BINANCE".into(),
            base_weight: 1.0,
            stale_after_ms: 1_000,
            disable_after_ms: 2_000,
        }],
    );
    let snapshots = collect_snapshots(publish_topic);

    {
        let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
        set_actor_clock_ms(&mut actor, 1_000);
        actor.start().unwrap();
    }

    advance_actor_clock_to(actor_id, 1_010);
    advance_actor_clock_to(actor_id, 2_000);

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].fair_value, None);
    assert_eq!(snapshots[0].confidence, 0.0);
    assert!(snapshots[0].venues[0].stale);
    assert_eq!(snapshots[0].venues[0].effective_weight, 0.0);
    assert_eq!(
        snapshots[0].venues[0].health,
        VenueHealth::Disabled {
            reason: "no reference update received yet".into(),
        }
    );
}

#[test]
fn quiet_period_timer_publishes_stale_snapshot_without_new_quote() {
    let publish_topic = "platform.reference.test.timer-stale";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            0,
            vec![subscription("BINANCE", "BTCUSDT.BINANCE", "BINANCE")],
        ),
        vec![ReferenceVenueEntry {
            name: "BINANCE".into(),
            kind: ReferenceVenueKind::Binance,
            instrument_id: "BTCUSDT.BINANCE".into(),
            base_weight: 1.0,
            stale_after_ms: 1_000,
            disable_after_ms: 2_000,
        }],
    );
    let snapshots = collect_snapshots(publish_topic);

    {
        let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
        actor.start().unwrap();
        set_actor_clock_ms(&mut actor, 1_000);
        actor
            .on_quote(&quote("BTCUSDT.BINANCE", "99.5", "100.5", 1_000))
            .unwrap();
    }

    advance_actor_clock_to(actor_id, 2_001);

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].fair_value, Some(100.0));
    assert_eq!(snapshots[0].venues[0].health, VenueHealth::Healthy);
    assert!(!snapshots[0].venues[0].stale);

    assert_eq!(snapshots[1].fair_value, None);
    assert_eq!(snapshots[1].confidence, 0.0);
    assert!(snapshots[1].venues[0].stale);
    assert_eq!(snapshots[1].venues[0].health, VenueHealth::Healthy);
    assert_eq!(snapshots[1].venues[0].effective_weight, 0.0);
}

#[test]
fn quiet_period_timer_publishes_disabled_snapshot_after_disable_threshold() {
    let publish_topic = "platform.reference.test.timer-disabled";
    let actor_id = register_reference_actor(
        actor_config(
            publish_topic,
            0,
            vec![subscription("BYBIT", "BTCUSDT.BYBIT", "BYBIT")],
        ),
        vec![ReferenceVenueEntry {
            name: "BYBIT".into(),
            kind: ReferenceVenueKind::Bybit,
            instrument_id: "BTCUSDT.BYBIT".into(),
            base_weight: 1.0,
            stale_after_ms: 1_000,
            disable_after_ms: 2_000,
        }],
    );
    let snapshots = collect_snapshots(publish_topic);

    {
        let mut actor = get_actor_unchecked::<ReferenceActor>(&actor_id.inner());
        actor.start().unwrap();
        set_actor_clock_ms(&mut actor, 1_000);
        actor
            .on_quote(&quote("BTCUSDT.BYBIT", "109.5", "110.5", 1_000))
            .unwrap();
    }

    advance_actor_clock_to(actor_id, 2_001);
    advance_actor_clock_to(actor_id, 3_001);

    let snapshots = snapshots.borrow();
    assert_eq!(snapshots.len(), 3);
    assert_eq!(snapshots[2].fair_value, None);
    assert_eq!(snapshots[2].confidence, 0.0);
    assert!(snapshots[2].venues[0].stale);
    assert_eq!(snapshots[2].venues[0].effective_weight, 0.0);
    assert_eq!(
        snapshots[2].venues[0].health,
        VenueHealth::Disabled {
            reason: "auto-disabled after 2001ms without a fresh reference update".into(),
        }
    );
}
