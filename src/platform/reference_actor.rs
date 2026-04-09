use std::{collections::BTreeMap, fmt::Debug};

use nautilus_common::{
    actor::{DataActor, DataActorConfig, DataActorCore},
    msgbus::publish_any,
    nautilus_actor,
};
use nautilus_model::{
    data::QuoteTick,
    identifiers::{ClientId, InstrumentId},
};

use crate::{
    config::ReferenceVenueEntry,
    platform::reference::{ReferenceObservation, fuse_reference_snapshot},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceSubscription {
    pub venue_name: String,
    pub instrument_id: InstrumentId,
    pub client_id: ClientId,
}

#[derive(Debug, Clone)]
pub struct ReferenceActorConfig {
    pub base: DataActorConfig,
    pub publish_topic: String,
    pub min_publish_interval_ms: u64,
    pub venue_subscriptions: Vec<ReferenceSubscription>,
}

#[derive(Debug)]
pub struct ReferenceActor {
    pub core: DataActorCore,
    pub config: ReferenceActorConfig,
    pub latest: BTreeMap<String, ReferenceObservation>,
    pub disabled: BTreeMap<String, String>,
    pub venue_cfgs: Vec<ReferenceVenueEntry>,
    pub instrument_to_venue_name: BTreeMap<InstrumentId, String>,
    pub last_publish_ms: Option<u64>,
}

impl ReferenceActor {
    pub fn new(config: ReferenceActorConfig, venue_cfgs: Vec<ReferenceVenueEntry>) -> Self {
        let instrument_to_venue_name = config
            .venue_subscriptions
            .iter()
            .map(|subscription| {
                (
                    subscription.instrument_id,
                    subscription.venue_name.clone(),
                )
            })
            .collect();

        Self {
            core: DataActorCore::new(config.base.clone()),
            config,
            latest: BTreeMap::new(),
            disabled: BTreeMap::new(),
            venue_cfgs,
            instrument_to_venue_name,
            last_publish_ms: None,
        }
    }

    pub fn disabled_mut(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.disabled
    }

    fn should_publish(&self, ts_ms: u64) -> bool {
        match self.last_publish_ms {
            None => true,
            Some(last_publish_ms) => {
                ts_ms.saturating_sub(last_publish_ms) >= self.config.min_publish_interval_ms
            }
        }
    }

    fn publish_snapshot(&mut self, ts_ms: u64) {
        if !self.should_publish(ts_ms) {
            return;
        }

        let snapshot = fuse_reference_snapshot(
            &self.config.publish_topic,
            ts_ms,
            &self.venue_cfgs,
            &self.latest,
            &self.disabled,
        );
        publish_any(self.config.publish_topic.as_str().into(), &snapshot);
        self.last_publish_ms = Some(ts_ms);
    }
}

nautilus_actor!(ReferenceActor);

impl DataActor for ReferenceActor {
    fn on_start(&mut self) -> anyhow::Result<()> {
        let subscriptions = self.config.venue_subscriptions.clone();
        for subscription in subscriptions {
            self.subscribe_quotes(
                subscription.instrument_id,
                Some(subscription.client_id),
                None,
            );
        }

        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> anyhow::Result<()> {
        let Some(venue_name) = self
            .instrument_to_venue_name
            .get(&quote.instrument_id)
            .cloned()
        else {
            return Ok(());
        };

        let ts_ms = u64::from(quote.ts_init) / 1_000_000;
        self.latest.insert(
            venue_name.clone(),
            ReferenceObservation::Orderbook {
                venue_name,
                instrument_id: quote.instrument_id.to_string(),
                bid: f64::from(&quote.bid_price),
                ask: f64::from(&quote.ask_price),
                ts_ms,
            },
        );
        self.publish_snapshot(ts_ms);

        Ok(())
    }
}
