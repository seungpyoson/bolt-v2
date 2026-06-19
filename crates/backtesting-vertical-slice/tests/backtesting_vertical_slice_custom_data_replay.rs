use std::{
    fmt::Debug,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use nautilus_backtest::{config::BacktestEngineConfig, engine::BacktestEngine};
use nautilus_common::actor::data_actor::DataActor;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{CustomData, Data, DataType},
    identifiers::{InstrumentId, StrategyId},
};
use nautilus_persistence_macros::custom_data;
use nautilus_serialization::ensure_custom_data_registered;
use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};

const CUSTOM_REPLAY_INSTRUMENT: &str = "REFERENCE.SOURCE";
const CUSTOM_REPLAY_KIND: &str = "entry_evaluation";
const CUSTOM_REPLAY_STRATEGY_ID: &str = "CUSTOM-REPLAY-001";
const CUSTOM_REPLAY_ORDER_TAG: &str = "001";

#[custom_data]
struct ReplayDecisionEvent {
    instrument_id: InstrumentId,
    event_kind: String,
    ts_event: UnixNanos,
    ts_init: UnixNanos,
}

struct CustomReplayObserver {
    core: StrategyCore,
    data_type: DataType,
    received_count: Arc<AtomicUsize>,
    received_kinds: Arc<Mutex<Vec<String>>>,
}

impl CustomReplayObserver {
    fn new(
        data_type: DataType,
        received_count: Arc<AtomicUsize>,
        received_kinds: Arc<Mutex<Vec<String>>>,
    ) -> Self {
        let config = StrategyConfig {
            strategy_id: Some(StrategyId::from(CUSTOM_REPLAY_STRATEGY_ID)),
            order_id_tag: Some(CUSTOM_REPLAY_ORDER_TAG.to_string()),
            ..Default::default()
        };

        Self {
            core: StrategyCore::new(config),
            data_type,
            received_count,
            received_kinds,
        }
    }
}

nautilus_strategy!(CustomReplayObserver);

impl Debug for CustomReplayObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(CustomReplayObserver)).finish()
    }
}

impl DataActor for CustomReplayObserver {
    fn on_start(&mut self) -> anyhow::Result<()> {
        self.subscribe_data(self.data_type.clone(), None, None);
        Ok(())
    }

    fn on_data(&mut self, data: &CustomData) -> anyhow::Result<()> {
        anyhow::ensure!(
            data.data_type == self.data_type,
            "received unexpected custom data type"
        );
        let event = data
            .data
            .as_any()
            .downcast_ref::<ReplayDecisionEvent>()
            .ok_or_else(|| anyhow::anyhow!("custom replay payload was not ReplayDecisionEvent"))?;

        self.received_count.fetch_add(1, Ordering::SeqCst);
        self.received_kinds
            .lock()
            .expect("received kinds lock poisoned")
            .push(event.event_kind.clone());
        Ok(())
    }
}

#[test]
fn backtest_engine_add_data_replays_custom_data_to_strategy_on_data() -> anyhow::Result<()> {
    ensure_custom_data_registered::<ReplayDecisionEvent>();

    let instrument_id = InstrumentId::from(CUSTOM_REPLAY_INSTRUMENT);
    let data_type = DataType::new("ReplayDecisionEvent", None, Some(instrument_id.to_string()));
    let event = ReplayDecisionEvent {
        instrument_id,
        event_kind: CUSTOM_REPLAY_KIND.to_string(),
        ts_event: UnixNanos::from(100),
        ts_init: UnixNanos::from(100),
    };
    let custom_data = CustomData::new(Arc::new(event), data_type.clone());

    let received_count = Arc::new(AtomicUsize::new(0));
    let received_kinds = Arc::new(Mutex::new(Vec::new()));
    let mut engine = BacktestEngine::new(BacktestEngineConfig::default())?;
    engine.add_strategy(CustomReplayObserver::new(
        data_type,
        Arc::clone(&received_count),
        Arc::clone(&received_kinds),
    ))?;

    engine.add_data(vec![Data::Custom(custom_data)], None, false, true)?;
    engine.run(None, None, None, false)?;

    assert_eq!(received_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        received_kinds
            .lock()
            .expect("received kinds lock poisoned")
            .as_slice(),
        [CUSTOM_REPLAY_KIND]
    );

    Ok(())
}
