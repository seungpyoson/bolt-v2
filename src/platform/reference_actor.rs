use std::{collections::BTreeMap, fmt::Debug};

use anyhow::anyhow;
use nautilus_common::{
    actor::{DataActor, DataActorConfig, DataActorCore},
    msgbus::publish_any,
    nautilus_actor,
    timer::TimeEvent,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{CustomData, QuoteTick},
    identifiers::{ClientId, InstrumentId},
};

use crate::{
    clients::chainlink::{ChainlinkOracleUpdate, chainlink_data_type_for_venue},
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
    pub latest_oracle_ordering: BTreeMap<String, OracleOrdering>,
    pub disabled: BTreeMap<String, String>,
    pub venue_cfgs: Vec<ReferenceVenueEntry>,
    pub instrument_to_venue_name: BTreeMap<InstrumentId, String>,
    pub last_publish_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct OracleOrdering {
    pub updated_at_ms: u64,
    pub round_id: u128,
}

impl ReferenceActor {
    const QUIET_TIMER_NAME_SUFFIX: &str = "reference-quiet-transition";

    pub fn new(config: ReferenceActorConfig, venue_cfgs: Vec<ReferenceVenueEntry>) -> Self {
        let instrument_to_venue_name = config
            .venue_subscriptions
            .iter()
            .map(|subscription| (subscription.instrument_id, subscription.venue_name.clone()))
            .collect();

        Self {
            core: DataActorCore::new(config.base.clone()),
            config,
            latest: BTreeMap::new(),
            latest_oracle_ordering: BTreeMap::new(),
            disabled: BTreeMap::new(),
            venue_cfgs,
            instrument_to_venue_name,
            last_publish_ms: None,
        }
    }

    pub fn disabled_mut(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.disabled
    }

    fn now_ms(&mut self) -> u64 {
        self.clock().timestamp_ns().as_u64() / 1_000_000
    }

    fn latest_ts_ms(&self, venue_name: &str) -> Option<u64> {
        match self.latest.get(venue_name) {
            Some(ReferenceObservation::Orderbook { ts_ms, .. })
            | Some(ReferenceObservation::Oracle { ts_ms, .. }) => Some(*ts_ms),
            None => None,
        }
    }

    fn should_publish(&self, ts_ms: u64) -> bool {
        match self.last_publish_ms {
            None => true,
            Some(last_publish_ms) => {
                ts_ms.saturating_sub(last_publish_ms) >= self.config.min_publish_interval_ms
            }
        }
    }

    fn quiet_timer_name(&self) -> String {
        format!(
            "{}-{}",
            self.actor_id().inner(),
            Self::QUIET_TIMER_NAME_SUFFIX
        )
    }

    fn next_transition_ms(&self, now_ms: u64) -> Option<u64> {
        if self.last_publish_ms.is_none()
            && self
                .venue_cfgs
                .iter()
                .any(|venue| !self.latest.contains_key(&venue.name))
        {
            return Some(now_ms.saturating_add(self.config.min_publish_interval_ms.max(1)));
        }

        self.venue_cfgs
            .iter()
            .filter_map(|venue| {
                let observation = self.latest.get(&venue.name)?;
                let observation_ts_ms = match observation {
                    ReferenceObservation::Orderbook { ts_ms, .. }
                    | ReferenceObservation::Oracle { ts_ms, .. } => *ts_ms,
                };
                let age_ms = now_ms.saturating_sub(observation_ts_ms);

                let stale_transition_ms = (age_ms <= venue.stale_after_ms).then(|| {
                    observation_ts_ms
                        .saturating_add(venue.stale_after_ms)
                        .saturating_add(1)
                });
                let disable_transition_ms = (!self.disabled.contains_key(&venue.name)
                    && age_ms <= venue.disable_after_ms)
                    .then(|| {
                        observation_ts_ms
                            .saturating_add(venue.disable_after_ms)
                            .saturating_add(1)
                    });

                stale_transition_ms
                    .into_iter()
                    .chain(disable_transition_ms)
                    .filter(|transition_ms| *transition_ms > now_ms)
                    .min()
            })
            .min()
    }

    fn reschedule_quiet_timer(&mut self) -> anyhow::Result<()> {
        let now_ms = self.now_ms();
        let timer_name = self.quiet_timer_name();
        let next_fire_ms = self.next_transition_ms(now_ms).map(|transition_ms| {
            let earliest_publish_ms = self
                .last_publish_ms
                .map(|last_publish_ms| {
                    last_publish_ms.saturating_add(self.config.min_publish_interval_ms)
                })
                .unwrap_or(now_ms);
            transition_ms.max(earliest_publish_ms)
        });

        let mut clock = self.clock();
        clock.cancel_timer(&timer_name);
        if let Some(next_fire_ms) = next_fire_ms {
            clock.set_time_alert_ns(
                &timer_name,
                UnixNanos::from(next_fire_ms.saturating_mul(1_000_000)),
                None,
                None,
            )?;
        }

        Ok(())
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

    fn venue_config(&self, venue_name: &str) -> Option<&ReferenceVenueEntry> {
        self.venue_cfgs
            .iter()
            .find(|venue| venue.name == venue_name)
    }

    fn parse_oracle_ordering(
        &self,
        update: &ChainlinkOracleUpdate,
    ) -> anyhow::Result<OracleOrdering> {
        let round_id = update.round_id.parse::<u128>().map_err(|error| {
            anyhow!(
                "reference_actor received non-numeric oracle round_id {} for venue {}: {error}",
                update.round_id,
                update.venue_name
            )
        })?;
        Ok(OracleOrdering {
            updated_at_ms: update.updated_at_ms,
            round_id,
        })
    }
}

nautilus_actor!(ReferenceActor);

impl DataActor for ReferenceActor {
    fn on_start(&mut self) -> anyhow::Result<()> {
        let subscriptions = self.config.venue_subscriptions.clone();
        for subscription in subscriptions {
            let is_chainlink = self
                .venue_config(&subscription.venue_name)
                .map(|venue| venue.kind == crate::config::ReferenceVenueKind::Chainlink)
                .ok_or_else(|| {
                    anyhow!(
                        "reference_actor missing config for venue {}",
                        subscription.venue_name
                    )
                })?;

            if is_chainlink {
                let instrument_id = subscription.instrument_id.to_string();
                self.subscribe_data(
                    chainlink_data_type_for_venue(&subscription.venue_name, &instrument_id),
                    Some(subscription.client_id),
                    None,
                );
            } else {
                self.subscribe_quotes(
                    subscription.instrument_id,
                    Some(subscription.client_id),
                    None,
                );
            }
        }

        self.reschedule_quiet_timer()?;

        Ok(())
    }

    fn on_stop(&mut self) -> anyhow::Result<()> {
        let timer_name = self.quiet_timer_name();
        self.clock().cancel_timer(&timer_name);
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> anyhow::Result<()> {
        let venue_name = self
            .instrument_to_venue_name
            .get(&quote.instrument_id)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "reference_actor received quote for unmapped instrument {}",
                    quote.instrument_id
                )
            })?;

        let ts_ms = u64::from(quote.ts_init) / 1_000_000;
        if self
            .latest_ts_ms(&venue_name)
            .is_some_and(|latest_ts_ms| latest_ts_ms >= ts_ms)
        {
            return Ok(());
        }

        self.latest.insert(
            venue_name.clone(),
            ReferenceObservation::Orderbook {
                venue_name,
                instrument_id: quote.instrument_id.to_string(),
                bid: f64::from(&quote.bid_price),
                ask: f64::from(&quote.ask_price),
                ts_ms,
                observed_ts_ms: ts_ms,
            },
        );
        let now_ms = self.now_ms();
        self.publish_snapshot(now_ms);
        self.reschedule_quiet_timer()?;

        Ok(())
    }

    fn on_data(&mut self, data: &CustomData) -> anyhow::Result<()> {
        let update = data
            .data
            .as_any()
            .downcast_ref::<ChainlinkOracleUpdate>()
            .ok_or_else(|| anyhow!("reference_actor received unsupported custom data type"))?;

        let (is_chainlink, expected_instrument_id) = self
            .venue_config(&update.venue_name)
            .map(|venue| {
                (
                    venue.kind == crate::config::ReferenceVenueKind::Chainlink,
                    venue.instrument_id.clone(),
                )
            })
            .ok_or_else(|| {
                anyhow!(
                    "reference_actor received oracle update for unmapped venue {}",
                    update.venue_name
                )
            })?;
        if !is_chainlink {
            return Err(anyhow!(
                "reference_actor received oracle update for non-chainlink venue {}",
                update.venue_name
            ));
        }
        if expected_instrument_id != update.instrument_id {
            return Err(anyhow!(
                "reference_actor received oracle update with mismatched instrument {} for venue {}",
                update.instrument_id,
                update.venue_name
            ));
        }
        let incoming_ordering = self.parse_oracle_ordering(update)?;
        if self
            .latest_oracle_ordering
            .get(&update.venue_name)
            .is_some_and(|latest| *latest >= incoming_ordering)
        {
            return Ok(());
        }

        self.latest.insert(
            update.venue_name.clone(),
            ReferenceObservation::Oracle {
                venue_name: update.venue_name.clone(),
                instrument_id: update.instrument_id.clone(),
                price: update.price,
                ts_ms: update.updated_at_ms,
                observed_ts_ms: update.ts_init.as_u64() / 1_000_000,
            },
        );
        self.latest_oracle_ordering
            .insert(update.venue_name.clone(), incoming_ordering);
        let now_ms = self.now_ms();
        self.publish_snapshot(now_ms);
        self.reschedule_quiet_timer()?;

        Ok(())
    }

    fn on_time_event(&mut self, event: &TimeEvent) -> anyhow::Result<()> {
        if event.name.as_str() != self.quiet_timer_name() {
            return Ok(());
        }

        let now_ms = self.now_ms();
        self.publish_snapshot(now_ms);
        self.reschedule_quiet_timer()?;

        Ok(())
    }
}
