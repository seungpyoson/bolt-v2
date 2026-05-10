use nautilus_common::actor::DataActorConfig;
use nautilus_model::identifiers::{ClientId, InstrumentId};

use crate::{
    bolt_v3_config::{BoltV3RootConfig, ReferenceSourceType, ReferenceStreamBlock},
    bolt_v3_providers::{binance, chainlink, polymarket},
    config::{ChainlinkReferenceConfig, ReferenceVenueEntry, ReferenceVenueKind},
    platform::reference_actor::{ReferenceActor, ReferenceActorConfig, ReferenceSubscription},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3ReferenceProducerError {
    reason: String,
}

impl BoltV3ReferenceProducerError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for BoltV3ReferenceProducerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for BoltV3ReferenceProducerError {}

#[derive(Debug)]
pub struct BoltV3ReferenceActorPlan {
    pub config: ReferenceActorConfig,
    pub venue_cfgs: Vec<ReferenceVenueEntry>,
}

impl BoltV3ReferenceActorPlan {
    pub fn from_stream(
        root: &BoltV3RootConfig,
        stream_id: &str,
        stream: &ReferenceStreamBlock,
    ) -> Result<Self, BoltV3ReferenceProducerError> {
        let mut venue_subscriptions = Vec::with_capacity(stream.inputs.len());
        let mut venue_cfgs = Vec::with_capacity(stream.inputs.len());

        for (index, input) in stream.inputs.iter().enumerate() {
            let context = format!("reference_streams.{stream_id}.inputs[{index}]");
            let data_client_id = input.data_client_id.as_deref().ok_or_else(|| {
                BoltV3ReferenceProducerError::new(format!("{context}.data_client_id is required"))
            })?;
            let client = root.clients.get(data_client_id).ok_or_else(|| {
                BoltV3ReferenceProducerError::new(format!(
                    "{context}.data_client_id `{data_client_id}` does not match any [clients.<id>] block"
                ))
            })?;
            if client.data.is_none() {
                return Err(BoltV3ReferenceProducerError::new(format!(
                    "{context}.data_client_id `{data_client_id}` must reference a data-capable client"
                )));
            }

            let kind =
                reference_kind_for_source(&context, input.source_type, client.venue.as_str())?;
            venue_subscriptions.push(ReferenceSubscription {
                venue_name: input.source_id.clone(),
                instrument_id: InstrumentId::from(input.instrument_id.as_str()),
                client_id: ClientId::from(data_client_id),
            });
            let chainlink = if kind == ReferenceVenueKind::Chainlink {
                let chainlink = input.chainlink.as_ref().ok_or_else(|| {
                    BoltV3ReferenceProducerError::new(format!(
                        "{context}.chainlink feed config is required for Chainlink producer inputs"
                    ))
                })?;
                Some(ChainlinkReferenceConfig {
                    feed_id: chainlink.feed_id.clone(),
                    price_scale: chainlink.price_scale,
                })
            } else {
                None
            };
            venue_cfgs.push(ReferenceVenueEntry {
                name: input.source_id.clone(),
                kind,
                instrument_id: input.instrument_id.clone(),
                base_weight: input.base_weight,
                stale_after_ms: input.stale_after_milliseconds,
                disable_after_ms: input.disable_after_milliseconds,
                chainlink,
            });
        }

        Ok(Self {
            config: ReferenceActorConfig {
                base: DataActorConfig::default(),
                publish_topic: stream.publish_topic.clone(),
                min_publish_interval_ms: stream.min_publish_interval_milliseconds,
                venue_subscriptions,
            },
            venue_cfgs,
        })
    }

    pub fn into_actor(self) -> ReferenceActor {
        ReferenceActor::new(self.config, self.venue_cfgs)
    }
}

fn reference_kind_for_source(
    context: &str,
    source_type: ReferenceSourceType,
    venue: &str,
) -> Result<ReferenceVenueKind, BoltV3ReferenceProducerError> {
    match (source_type, venue) {
        (ReferenceSourceType::Orderbook, binance::KEY) => Ok(ReferenceVenueKind::Binance),
        (ReferenceSourceType::Orderbook, polymarket::KEY) => Ok(ReferenceVenueKind::Polymarket),
        (ReferenceSourceType::Oracle, chainlink::KEY) => Ok(ReferenceVenueKind::Chainlink),
        (ReferenceSourceType::Oracle, venue) => Err(BoltV3ReferenceProducerError::new(format!(
            "{context}.source_type `oracle` is not supported for client venue `{venue}`"
        ))),
        (ReferenceSourceType::Orderbook, venue) => Err(BoltV3ReferenceProducerError::new(format!(
            "{context}.source_type `orderbook` is not supported for client venue `{venue}`"
        ))),
    }
}
